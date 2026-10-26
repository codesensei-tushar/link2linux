//! The clipboard through X11 selections, on X11 sessions and through
//! Xwayland on Wayland ones.
//!
//! Two connections to the X server, each with a window of its own:
//!
//! - the owner, on a thread of its own, holds `CLIPBOARD` after a write and
//!   hands the text to each app that pastes it, in parts (the `INCR`
//!   protocol) when it is too big for one request. It also hears of every
//!   clipboard change through XFixes;
//! - the reader, on a worker thread, asks the app holding `CLIPBOARD` for
//!   its content and waits for it, up to a timeout.

use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::ops::Range;
use std::os::unix::net::UnixStream;
use std::sync::Arc;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use rustix::event::{PollFd, PollFlags, Timespec};
use tokio::sync::watch;
use x11rb::connection::{Connection, RequestConnection};
use x11rb::errors::{ConnectionError, ReplyError, ReplyOrIdError};
use x11rb::protocol::Event;
use x11rb::protocol::xfixes::{ConnectionExt as _, SelectionEventMask};
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ChangeWindowAttributesAux, ConnectionExt as _, CreateWindowAux, EventMask,
    PropMode, Property, PropertyNotifyEvent, SELECTION_NOTIFY_EVENT, SelectionNotifyEvent,
    SelectionRequestEvent, Timestamp, Window, WindowClass,
};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;
use x11rb::{CURRENT_TIME, NONE};

use super::{
    MAX_READ_BYTES, OPEN_TIMEOUT, PlatformError, TRANSFER_TIMEOUT, Worker, read_text, too_long,
    unavailable,
};
use crate::ClipboardText;

x11rb::atom_manager! {
    Atoms: AtomsCookie {
        CLIPBOARD,
        TARGETS,
        TIMESTAMP,
        INCR,
        UTF8_STRING,
        TEXT_PLAIN_UTF8: b"text/plain;charset=utf-8",
        TEXT_PLAIN: b"text/plain",
        PASSWORD_HINT: b"x-kde-passwordManagerHint",
        // Properties on our own windows: where the clipboard is delivered,
        // and one written to learn the server's time.
        TRANSFER: b"LINK2LINUX_CLIPBOARD",
        CLOCK: b"LINK2LINUX_CLOCK",
    }
}

/// The most targets read from a clipboard owner.
const MAX_TARGETS: usize = 1024;
/// Text over this size is sent in parts, at most this big each.
const MAX_PART_BYTES: usize = 1 << 20;

pub(super) struct X11Clipboard {
    worker: Worker<Session>,
}

/// What the worker thread uses: the reader's connection, and a way to tell
/// the owner to take the clipboard.
struct Session {
    reader: Reader,
    owner: OwnerHandle,
}

impl X11Clipboard {
    /// The clipboard of the X server in `DISPLAY`, or `None` when there is
    /// none or it lacks XFixes.
    pub(super) fn open(changes: watch::Sender<()>) -> Option<Self> {
        Self::open_with(changes, None)
    }

    /// With `part_bytes`, the owner sends text over that size in parts of
    /// that size, so tests can make it.
    fn open_with(changes: watch::Sender<()>, part_bytes: Option<usize>) -> Option<Self> {
        let (ready, started) = mpsc::sync_channel(1);
        let opening = thread::Builder::new()
            .name("x11-clipboard-open".into())
            .spawn(move || {
                let _ = ready.send(Self::start(changes, part_bytes));
            });
        if let Err(error) = opening {
            tracing::warn!(%error, "cannot open the X11 clipboard");
            return None;
        }
        match started.recv_timeout(OPEN_TIMEOUT) {
            Ok(Ok(clipboard)) => Some(clipboard),
            Ok(Err(error)) => {
                tracing::debug!(%error, "no X11 clipboard");
                None
            }
            Err(_) => {
                tracing::warn!("the X server did not answer in time");
                None
            }
        }
    }

    fn start(changes: watch::Sender<()>, part_bytes: Option<usize>) -> Result<Self, PlatformError> {
        let mut owner = Owner::connect().map_err(unavailable)?;
        if let Some(part_bytes) = part_bytes {
            owner.part_bytes = part_bytes;
        }
        let reader = Reader::connect().map_err(unavailable)?;
        let (commands, received) = mpsc::channel();
        let (wake, woken) = UnixStream::pair()?;
        wake.set_nonblocking(true)?;
        thread::Builder::new()
            .name("x11-clipboard-owner".into())
            .spawn(move || owner.run(&received, &woken, &changes))?;
        let session = Session {
            reader,
            owner: OwnerHandle { commands, wake },
        };
        Ok(Self {
            worker: Worker::spawn("x11-clipboard", session)?,
        })
    }

    pub(super) async fn read(&self) -> Result<Option<ClipboardText>, PlatformError> {
        self.worker
            .run(|session: &mut Session| session.reader.read())
            .await
    }

    pub(super) async fn write(&self, text: ClipboardText) -> Result<(), PlatformError> {
        self.worker
            .run(move |session: &mut Session| {
                let time = session.reader.server_time()?;
                session.owner.take(text, time)
            })
            .await
    }
}

/// Makes a connection and a window that is never shown, which receives
/// property changes.
fn connect() -> Result<(RustConnection, Window, Atoms), ReplyOrIdError> {
    let (conn, screen) = x11rb::connect(None)
        .map_err(|e| ConnectionError::IoError(io::Error::new(io::ErrorKind::NotConnected, e)))?;
    let atoms = Atoms::new(&conn)?;
    let window = conn.generate_id()?;
    let root = conn.setup().roots[screen].root;
    conn.create_window(
        0,
        window,
        root,
        0,
        0,
        1,
        1,
        0,
        WindowClass::INPUT_ONLY,
        0,
        &CreateWindowAux::new().event_mask(EventMask::PROPERTY_CHANGE),
    )?;
    let atoms = atoms.reply()?;
    Ok((conn, window, atoms))
}

/// Waits until `conn` has data to read, until `timeout` if given. Also
/// returns when `wake` is readable. Interrupted waits return early too:
/// callers loop.
fn wait(
    conn: &RustConnection,
    wake: Option<&UnixStream>,
    timeout: Option<Duration>,
) -> io::Result<()> {
    let timeout = timeout
        .map(Timespec::try_from)
        .transpose()
        .map_err(io::Error::other)?;
    let mut fds = vec![PollFd::new(conn.stream(), PollFlags::IN)];
    if let Some(wake) = wake {
        fds.push(PollFd::new(wake, PollFlags::IN));
    }
    match rustix::event::poll(&mut fds, timeout.as_ref()) {
        Ok(_) | Err(rustix::io::Errno::INTR) => Ok(()),
        Err(error) => Err(error.into()),
    }
}

/// Reads the clipboard on its own connection, one conversion at a time.
struct Reader {
    conn: RustConnection,
    window: Window,
    atoms: Atoms,
    /// Atom names already asked for. The server never forgets an atom.
    names: HashMap<Atom, String>,
}

impl Reader {
    fn connect() -> Result<Self, ReplyOrIdError> {
        let (conn, window, atoms) = connect()?;
        conn.flush()?;
        Ok(Self {
            conn,
            window,
            atoms,
            names: HashMap::new(),
        })
    }

    fn read(&mut self) -> Result<Option<ClipboardText>, PlatformError> {
        let owner = self
            .conn
            .get_selection_owner(self.atoms.CLIPBOARD)
            .map_err(unavailable)?
            .reply()
            .map_err(unavailable)?
            .owner;
        if owner == NONE {
            return Ok(None);
        }
        let Some(list) = self.convert(self.atoms.TARGETS, MAX_TARGETS * 4)? else {
            return Ok(None);
        };
        let atoms: Vec<Atom> = list
            .as_chunks::<4>()
            .0
            .iter()
            .map(|atom| u32::from_ne_bytes(*atom))
            .collect();
        let targets = self.names(&atoms)?;
        let types: Vec<&str> = targets.iter().map(|(_, name)| name.as_str()).collect();
        read_text(&types, |name, limit| {
            let (atom, _) = targets
                .iter()
                .find(|(_, offered)| offered == name)
                .expect("one of the offered types");
            self.convert(*atom, limit)
        })
    }

    /// The names of `atoms`, skipping any the server does not know.
    fn names(&mut self, atoms: &[Atom]) -> Result<Vec<(Atom, String)>, PlatformError> {
        let mut unknown: Vec<Atom> = atoms
            .iter()
            .copied()
            .filter(|atom| *atom != NONE && !self.names.contains_key(atom))
            .collect();
        unknown.sort_unstable();
        unknown.dedup();
        let cookies = unknown
            .iter()
            .map(|atom| self.conn.get_atom_name(*atom))
            .collect::<Result<Vec<_>, _>>()
            .map_err(unavailable)?;
        for (atom, cookie) in unknown.into_iter().zip(cookies) {
            match cookie.reply() {
                Ok(reply) => {
                    self.names
                        .insert(atom, String::from_utf8_lossy(&reply.name).into_owned());
                }
                Err(ReplyError::X11Error(_)) => {}
                Err(error) => return Err(unavailable(error)),
            }
        }
        Ok(atoms
            .iter()
            .filter_map(|atom| Some((*atom, self.names.get(atom)?.clone())))
            .collect())
    }

    /// Asks the clipboard's owner for its content as `target`, and waits
    /// for it, up to `limit` bytes. `None` when the owner refuses.
    fn convert(&self, target: Atom, limit: usize) -> Result<Option<Vec<u8>>, PlatformError> {
        let (conn, atoms) = (&self.conn, &self.atoms);
        // Left over from a conversion that timed out.
        conn.delete_property(self.window, atoms.TRANSFER)
            .map_err(unavailable)?;
        conn.convert_selection(
            self.window,
            atoms.CLIPBOARD,
            target,
            atoms.TRANSFER,
            CURRENT_TIME,
        )
        .map_err(unavailable)?;
        conn.flush().map_err(unavailable)?;
        let notified = self.next_event(|event| {
            matches!(event, Event::SelectionNotify(notify)
                if notify.requestor == self.window
                    && notify.selection == atoms.CLIPBOARD
                    && notify.target == target)
        })?;
        let Some(Event::SelectionNotify(notify)) = notified else {
            return Err(unavailable("the app that copied did not answer"));
        };
        if notify.property == NONE {
            return Ok(None);
        }
        let first = self.take_property(limit)?;
        if first.type_ != atoms.INCR {
            return Ok(Some(first.value));
        }
        // Sent in parts: each comes once the last is deleted, and an empty
        // one ends it.
        let mut parts = Parts::new(limit);
        loop {
            let written = self.next_event(|event| {
                matches!(event, Event::PropertyNotify(change)
                    if change.window == self.window
                        && change.atom == atoms.TRANSFER
                        && change.state == Property::NEW_VALUE)
            })?;
            if written.is_none() {
                return Err(unavailable("the app that copied stopped sending"));
            }
            let part = self.take_property(limit)?;
            if parts.add(&part.value)? {
                return Ok(Some(parts.data));
            }
        }
    }

    /// Reads and deletes the transfer property, up to `limit` bytes.
    fn take_property(&self, limit: usize) -> Result<Received, PlatformError> {
        let long_length = u32::try_from(limit / 4 + 1).unwrap_or(u32::MAX);
        let reply = self
            .conn
            .get_property(
                true,
                self.window,
                self.atoms.TRANSFER,
                AtomEnum::ANY,
                0,
                long_length,
            )
            .map_err(unavailable)?
            .reply()
            .map_err(unavailable)?;
        if reply.bytes_after > 0 || reply.value.len() > limit {
            // Too big to have been deleted with the read.
            let _ = self.conn.delete_property(self.window, self.atoms.TRANSFER);
            return Err(too_long());
        }
        Ok(Received {
            type_: reply.type_,
            value: reply.value,
        })
    }

    /// The server's current time, which taking the clipboard needs: learnt
    /// from the time stamp of a change to one of our own properties.
    fn server_time(&self) -> Result<Timestamp, PlatformError> {
        self.conn
            .change_property8(
                PropMode::APPEND,
                self.window,
                self.atoms.CLOCK,
                AtomEnum::STRING,
                &[],
            )
            .map_err(unavailable)?;
        self.conn.flush().map_err(unavailable)?;
        let changed = self.next_event(|event| {
            matches!(event, Event::PropertyNotify(change)
                if change.window == self.window && change.atom == self.atoms.CLOCK)
        })?;
        match changed {
            Some(Event::PropertyNotify(change)) => Ok(change.time),
            _ => Err(unavailable("the X server did not answer")),
        }
    }

    /// The next event `wanted` accepts, dropping the others, or `None` after
    /// [`TRANSFER_TIMEOUT`]. Only this connection's own requests cause
    /// events on it.
    fn next_event(&self, wanted: impl Fn(&Event) -> bool) -> Result<Option<Event>, PlatformError> {
        let deadline = Instant::now() + TRANSFER_TIMEOUT;
        loop {
            while let Some(event) = self.conn.poll_for_event().map_err(unavailable)? {
                if wanted(&event) {
                    return Ok(Some(event));
                }
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Ok(None);
            }
            wait(&self.conn, None, Some(left))?;
        }
    }
}

/// A property read by [`Reader::take_property`].
struct Received {
    type_: Atom,
    value: Vec<u8>,
}

/// Text received in parts, up to a limit.
struct Parts {
    data: Vec<u8>,
    limit: usize,
}

impl Parts {
    fn new(limit: usize) -> Self {
        Self {
            data: Vec::new(),
            limit,
        }
    }

    /// Adds the next part; `true` once the empty part that ends the text
    /// arrived.
    fn add(&mut self, part: &[u8]) -> Result<bool, PlatformError> {
        if part.is_empty() {
            return Ok(true);
        }
        if self.data.len() + part.len() > self.limit.min(MAX_READ_BYTES) {
            return Err(too_long());
        }
        self.data.extend_from_slice(part);
        Ok(false)
    }
}

/// Tells the owner thread to take the clipboard.
struct OwnerHandle {
    commands: mpsc::Sender<Take>,
    /// Written to after each command, to wake the owner thread; closed
    /// when the clipboard is dropped, to stop it.
    wake: UnixStream,
}

/// Take the clipboard with this text, at this server time.
struct Take {
    text: ClipboardText,
    time: Timestamp,
    done: mpsc::SyncSender<Result<(), PlatformError>>,
}

impl OwnerHandle {
    fn take(&mut self, text: ClipboardText, time: Timestamp) -> Result<(), PlatformError> {
        let (done, result) = mpsc::sync_channel(1);
        let stopped = || unavailable("the X11 clipboard connection closed");
        self.commands
            .send(Take { text, time, done })
            .map_err(|_| stopped())?;
        match self.wake.write(&[0]) {
            Ok(_) => {}
            // Full of wake-ups not read yet: it wakes anyway.
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
            Err(_) => return Err(stopped()),
        }
        result
            .recv_timeout(TRANSFER_TIMEOUT)
            .map_err(|_| stopped())?
    }
}

/// Holds the clipboard after a write, and watches it.
struct Owner {
    conn: RustConnection,
    window: Window,
    atoms: Atoms,
    /// The text this app put on the clipboard, while it holds it.
    owned: Option<Owned>,
    /// Text being sent in parts.
    transfers: Vec<Transfer>,
    /// Text over this size is sent in parts this big.
    part_bytes: usize,
}

#[derive(Clone)]
struct Owned {
    text: Arc<[u8]>,
    sensitive: bool,
    /// When it was taken, as the `TIMESTAMP` target reports.
    time: Timestamp,
}

/// Text being sent to an app in parts: each part is written once the app
/// deleted the last, and an empty part ends it.
struct Transfer {
    requestor: Window,
    property: Atom,
    target: Atom,
    text: Arc<[u8]>,
    sent: usize,
    /// When the last part was written; an app that stops reading is given up.
    deadline: Instant,
    /// The empty part that ends the text was written.
    ended: bool,
}

impl Transfer {
    /// The range of the next part, or `None` once the text was sent in
    /// full and ended. The range is empty for the part that ends it.
    fn next_part(&mut self, size: usize) -> Option<Range<usize>> {
        if self.ended {
            return None;
        }
        let start = self.sent;
        self.sent = (start + size).min(self.text.len());
        self.ended = start == self.sent;
        Some(start..self.sent)
    }
}

impl Owner {
    fn connect() -> Result<Self, ReplyOrIdError> {
        let (conn, window, atoms) = connect()?;
        // Fails on servers without XFixes, whose clipboard cannot be watched.
        conn.xfixes_query_version(1, 0)?.reply()?;
        conn.xfixes_select_selection_input(
            window,
            atoms.CLIPBOARD,
            SelectionEventMask::SET_SELECTION_OWNER
                | SelectionEventMask::SELECTION_WINDOW_DESTROY
                | SelectionEventMask::SELECTION_CLIENT_CLOSE,
        )?;
        // With BIG-REQUESTS, a request may carry far more than 256 KiB.
        conn.prefetch_maximum_request_bytes();
        let part_bytes = conn
            .maximum_request_bytes()
            .saturating_sub(64)
            .clamp(4096, MAX_PART_BYTES);
        conn.flush()?;
        Ok(Self {
            conn,
            window,
            atoms,
            owned: None,
            transfers: Vec::new(),
            part_bytes,
        })
    }

    fn run(
        mut self,
        commands: &mpsc::Receiver<Take>,
        wake: &UnixStream,
        changes: &watch::Sender<()>,
    ) {
        if let Err(error) = self.serve(commands, wake, changes) {
            tracing::warn!(%error, "the X11 clipboard connection closed");
        }
    }

    /// Serves until the clipboard is dropped, which closes `wake`.
    fn serve(
        &mut self,
        commands: &mpsc::Receiver<Take>,
        mut wake: &UnixStream,
        changes: &watch::Sender<()>,
    ) -> Result<(), ReplyOrIdError> {
        wake.set_nonblocking(true)
            .map_err(ConnectionError::IoError)?;
        loop {
            while let Ok(take) = commands.try_recv() {
                let result = self.take(&take.text, take.time);
                let _ = take.done.send(result);
            }
            self.give_up_stalled(Instant::now())?;
            while let Some(event) = self.conn.poll_for_event()? {
                self.handle(event, changes)?;
            }
            self.conn.flush()?;
            let timeout = self
                .transfers
                .iter()
                .map(|transfer| transfer.deadline.saturating_duration_since(Instant::now()))
                .min();
            wait(&self.conn, Some(wake), timeout).map_err(ConnectionError::IoError)?;
            let mut buffer = [0; 64];
            loop {
                match wake.read(&mut buffer) {
                    Ok(0) => return Ok(()),
                    Ok(_) => {}
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                    Err(e) => return Err(ConnectionError::IoError(e).into()),
                }
            }
        }
    }

    fn take(&mut self, text: &ClipboardText, time: Timestamp) -> Result<(), PlatformError> {
        // Set first: requests for it may come as soon as it is taken.
        self.owned = Some(Owned {
            text: Arc::from(text.text.as_bytes()),
            sensitive: text.sensitive,
            time,
        });
        self.conn
            .set_selection_owner(self.window, self.atoms.CLIPBOARD, time)
            .map_err(unavailable)?;
        let owner = self
            .conn
            .get_selection_owner(self.atoms.CLIPBOARD)
            .map_err(unavailable)?
            .reply()
            .map_err(unavailable)?
            .owner;
        if owner != self.window {
            self.owned = None;
            return Err(unavailable(
                "another app took the clipboard at the same time",
            ));
        }
        tracing::debug!(
            bytes = text.text.len(),
            sensitive = text.sensitive,
            "wrote the clipboard"
        );
        Ok(())
    }

    fn handle(&mut self, event: Event, changes: &watch::Sender<()>) -> Result<(), ReplyOrIdError> {
        match event {
            Event::XfixesSelectionNotify(notify) if notify.selection == self.atoms.CLIPBOARD => {
                changes.send_replace(());
            }
            Event::SelectionRequest(request) => self.answer(&request)?,
            Event::SelectionClear(clear)
                if clear.selection == self.atoms.CLIPBOARD && clear.owner == self.window =>
            {
                self.owned = None;
            }
            Event::PropertyNotify(change) if change.state == Property::DELETE => {
                self.send_next_part(&change)?;
            }
            // From a request about an app's window, which it closed.
            Event::Error(error) => {
                tracing::debug!(kind = ?error.error_kind, "X11 request failed");
            }
            _ => {}
        }
        Ok(())
    }

    /// Answers an app that pastes: with the text, the types it is offered
    /// in, or a refusal.
    fn answer(&mut self, request: &SelectionRequestEvent) -> Result<(), ReplyOrIdError> {
        // Apps from before ICCCM 2 name no property: the target is used.
        let property = if request.property == NONE {
            request.target
        } else {
            request.property
        };
        let answered = match self.owned.clone() {
            Some(owned)
                if request.selection == self.atoms.CLIPBOARD && request.owner == self.window =>
            {
                self.serve_target(request.requestor, property, request.target, &owned)?
            }
            _ => false,
        };
        self.conn.send_event(
            false,
            request.requestor,
            EventMask::NO_EVENT,
            SelectionNotifyEvent {
                response_type: SELECTION_NOTIFY_EVENT,
                sequence: 0,
                time: request.time,
                requestor: request.requestor,
                selection: request.selection,
                target: request.target,
                property: if answered { property } else { NONE },
            },
        )?;
        Ok(())
    }

    /// Writes `owned` as `target` to the requestor's `property`; `false`
    /// for a target it is not offered as.
    fn serve_target(
        &mut self,
        requestor: Window,
        property: Atom,
        target: Atom,
        owned: &Owned,
    ) -> Result<bool, ReplyOrIdError> {
        let atoms = &self.atoms;
        let conn = &self.conn;
        if target == atoms.TARGETS {
            let mut targets = vec![
                atoms.TARGETS,
                atoms.TIMESTAMP,
                atoms.UTF8_STRING,
                atoms.TEXT_PLAIN_UTF8,
                atoms.TEXT_PLAIN,
            ];
            if owned.sensitive {
                targets.push(atoms.PASSWORD_HINT);
            }
            conn.change_property32(
                PropMode::REPLACE,
                requestor,
                property,
                AtomEnum::ATOM,
                &targets,
            )?;
        } else if target == atoms.TIMESTAMP {
            conn.change_property32(
                PropMode::REPLACE,
                requestor,
                property,
                AtomEnum::INTEGER,
                &[owned.time],
            )?;
        } else if owned.sensitive && target == atoms.PASSWORD_HINT {
            conn.change_property8(PropMode::REPLACE, requestor, property, target, b"secret")?;
        } else if [atoms.UTF8_STRING, atoms.TEXT_PLAIN_UTF8, atoms.TEXT_PLAIN].contains(&target) {
            if owned.text.len() > self.part_bytes {
                self.start_transfer(requestor, property, target, &owned.text)?;
            } else {
                conn.change_property8(PropMode::REPLACE, requestor, property, target, &owned.text)?;
            }
        } else {
            return Ok(false);
        }
        Ok(true)
    }

    /// Starts sending `text` in parts: announced with its size as `INCR`,
    /// after which the requestor deletes the property to ask for each part.
    fn start_transfer(
        &mut self,
        requestor: Window,
        property: Atom,
        target: Atom,
        text: &Arc<[u8]>,
    ) -> Result<(), ReplyOrIdError> {
        self.conn.change_window_attributes(
            requestor,
            &ChangeWindowAttributesAux::new().event_mask(EventMask::PROPERTY_CHANGE),
        )?;
        let size = u32::try_from(text.len()).unwrap_or(u32::MAX);
        self.conn.change_property32(
            PropMode::REPLACE,
            requestor,
            property,
            self.atoms.INCR,
            &[size],
        )?;
        self.transfers.push(Transfer {
            requestor,
            property,
            target,
            text: text.clone(),
            sent: 0,
            deadline: Instant::now() + TRANSFER_TIMEOUT,
            ended: false,
        });
        Ok(())
    }

    /// Sends the next part to an app that deleted the last.
    fn send_next_part(&mut self, change: &PropertyNotifyEvent) -> Result<(), ReplyOrIdError> {
        let Some(index) = self.transfers.iter().position(|transfer| {
            transfer.requestor == change.window && transfer.property == change.atom
        }) else {
            return Ok(());
        };
        let transfer = &mut self.transfers[index];
        match transfer.next_part(self.part_bytes) {
            Some(part) => {
                self.conn.change_property8(
                    PropMode::APPEND,
                    transfer.requestor,
                    transfer.property,
                    transfer.target,
                    &transfer.text[part],
                )?;
                transfer.deadline = Instant::now() + TRANSFER_TIMEOUT;
                if transfer.ended {
                    self.finish_transfer(index)?;
                }
            }
            None => self.finish_transfer(index)?,
        }
        Ok(())
    }

    fn give_up_stalled(&mut self, now: Instant) -> Result<(), ReplyOrIdError> {
        while let Some(index) = self
            .transfers
            .iter()
            .position(|transfer| transfer.deadline <= now)
        {
            tracing::debug!("an app stopped reading the clipboard");
            self.finish_transfer(index)?;
        }
        Ok(())
    }

    /// Forgets a transfer, and stops hearing of the requestor's property
    /// changes unless another transfer to it goes on.
    fn finish_transfer(&mut self, index: usize) -> Result<(), ReplyOrIdError> {
        let requestor = self.transfers.swap_remove(index).requestor;
        if requestor != self.window && !self.transfers.iter().any(|t| t.requestor == requestor) {
            self.conn.change_window_attributes(
                requestor,
                &ChangeWindowAttributesAux::new().event_mask(EventMask::NO_EVENT),
            )?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn transfer(text: &[u8]) -> Transfer {
        Transfer {
            requestor: 1,
            property: 2,
            target: 3,
            text: Arc::from(text),
            sent: 0,
            deadline: Instant::now(),
            ended: false,
        }
    }

    #[test]
    fn sends_text_in_parts_then_ends_it() {
        let mut parts = transfer(b"abcdefghij");
        assert_eq!(parts.next_part(4), Some(0..4));
        assert_eq!(parts.next_part(4), Some(4..8));
        assert_eq!(parts.next_part(4), Some(8..10));
        assert_eq!(parts.next_part(4), Some(10..10));
        assert!(parts.ended);
        assert_eq!(parts.next_part(4), None);

        let mut exact = transfer(b"abcd");
        assert_eq!(exact.next_part(4), Some(0..4));
        assert_eq!(exact.next_part(4), Some(4..4));
        assert_eq!(exact.next_part(4), None);
    }

    #[test]
    fn puts_received_parts_together() {
        let mut parts = Parts::new(8);
        assert!(!parts.add(b"abc").unwrap());
        assert!(!parts.add(b"defgh").unwrap());
        assert!(parts.add(b"").unwrap());
        assert_eq!(parts.data, b"abcdefgh");

        let mut over = Parts::new(4);
        assert!(!over.add(b"abc").unwrap());
        assert!(over.add(b"de").is_err());
    }

    /// Opens the clipboard of the X server in `DISPLAY`.
    fn open(part_bytes: Option<usize>) -> (X11Clipboard, watch::Receiver<()>) {
        let (changes, changed) = watch::channel(());
        let clipboard = X11Clipboard::open_with(changes, part_bytes).expect("an X server");
        (clipboard, changed)
    }

    async fn next_change(changed: &mut watch::Receiver<()>) -> Result<(), String> {
        tokio::time::timeout(TRANSFER_TIMEOUT, changed.changed())
            .await
            .map_err(|_| "no change reported in time")?
            .map_err(|e| e.to_string())
    }

    async fn expect_text(clipboard: &X11Clipboard, expected: &ClipboardText) -> Result<(), String> {
        let read = clipboard.read().await.map_err(|e| e.to_string())?;
        if read.as_ref() == Some(expected) {
            Ok(())
        } else {
            Err(format!("read {read:?}, expected {expected:?}"))
        }
    }

    /// Gives a clipboard manager, such as GNOME's, time to copy the text
    /// just written: when the app that wrote it exits, the manager keeps
    /// the last text it copied.
    async fn settle() {
        tokio::time::sleep(Duration::from_secs(1)).await;
    }

    /// Needs an X server or Xwayland, and replaces the clipboard while it
    /// runs (the text on it before is put back at the end):
    /// `DISPLAY=:0 cargo test -p link2linux-platform -- --ignored x11_clipboard`
    #[tokio::test]
    #[ignore = "needs an X server"]
    async fn x11_clipboard_round_trip() {
        // Another app, as far as the X server can tell: its own connections.
        let (reader, mut changed) = open(None);
        let saved = reader.read().await.unwrap();
        let outcome = async {
            // Sends anything over 4 KiB in parts.
            let (writer, _) = open(Some(4096));
            let secret = ClipboardText {
                text: "ü".repeat(128 << 10),
                sensitive: true,
            };
            changed.mark_unchanged();
            writer
                .write(secret.clone())
                .await
                .map_err(|e| e.to_string())?;
            next_change(&mut changed).await?;
            expect_text(&reader, &secret).await?;
            // Its own text too.
            expect_text(&writer, &secret).await?;

            let plain = ClipboardText {
                text: "plain".into(),
                sensitive: false,
            };
            writer
                .write(plain.clone())
                .await
                .map_err(|e| e.to_string())?;
            next_change(&mut changed).await?;
            expect_text(&reader, &plain).await?;

            // 256 KiB in one request, as BIG-REQUESTS allows.
            let (whole, _) = open(None);
            let big = ClipboardText {
                text: "x".repeat(256 << 10),
                sensitive: false,
            };
            whole.write(big.clone()).await.map_err(|e| e.to_string())?;
            expect_text(&reader, &big).await
        }
        .await;
        let restored = match saved {
            Some(saved) => reader.write(saved).await,
            None => Ok(()),
        };
        settle().await;
        outcome.unwrap();
        restored.unwrap();
    }
}
