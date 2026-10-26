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
//!   its content and waits for it, up to a timeout. It hears of clipboard
//!   changes too, to tell when a read spans two copies.

use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::ops::Range;
use std::os::unix::net::UnixStream;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

use rustix::event::{PollFd, PollFlags, Timespec};
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
    MAX_READ_BYTES, OPEN_TIMEOUT, PlatformError, READ_TIMEOUT, Reporter, Source, TRANSFER_TIMEOUT,
    Worker, too_long, unavailable,
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
        // A property of the reader's window, written to learn the server's
        // time.
        CLOCK: b"LINK2LINUX_CLOCK",
    }
}

/// The most targets read from a clipboard owner.
const MAX_TARGETS: usize = 1024;
/// Text over this size is sent in parts, at most this big each.
const MAX_PART_BYTES: usize = 1 << 20;
/// How many properties the reader takes turns with, one per conversion, so
/// that a late answer to one is never taken for the next one's.
const TRANSFER_PROPERTIES: usize = 8;

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
    /// The clipboard of the X server named by `display`, or by `DISPLAY`
    /// when `None`. Fails when there is no such server, it refuses the
    /// connection or lacks XFixes, or it does not answer in time; it is
    /// tried once.
    pub(super) fn open(changes: Reporter, display: Option<&str>) -> Result<Self, PlatformError> {
        Self::open_with(changes, display, None)
    }

    /// With `part_bytes`, the owner sends text over that size in parts of
    /// that size, so tests can make it.
    fn open_with(
        changes: Reporter,
        display: Option<&str>,
        part_bytes: Option<usize>,
    ) -> Result<Self, PlatformError> {
        let display = display.map(str::to_owned);
        let (ready, started) = mpsc::sync_channel(1);
        thread::Builder::new()
            .name("x11-clipboard-open".into())
            .spawn(move || {
                let _ = ready.send(Self::start(changes, display.as_deref(), part_bytes));
            })?;
        started
            .recv_timeout(OPEN_TIMEOUT)
            .map_err(|_| unavailable("the X server did not answer in time"))?
    }

    fn start(
        changes: Reporter,
        display: Option<&str>,
        part_bytes: Option<usize>,
    ) -> Result<Self, PlatformError> {
        let mut owner = Owner::connect(display).map_err(unavailable)?;
        if let Some(part_bytes) = part_bytes {
            owner.part_bytes = part_bytes;
        }
        let reader = Reader::connect(display).map_err(unavailable)?;
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
            .run(|session: &mut Session| {
                super::read(&mut session.reader, Instant::now() + READ_TIMEOUT)
            })
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

/// Connects to the X server, and returns its root window and the atoms.
/// Fails on servers without XFixes, whose clipboard cannot be watched.
fn connect(display: Option<&str>) -> Result<(RustConnection, Window, Atoms), ReplyOrIdError> {
    let (conn, screen) = x11rb::connect(display)
        .map_err(|e| ConnectionError::IoError(io::Error::new(io::ErrorKind::NotConnected, e)))?;
    let atoms = Atoms::new(&conn)?;
    let root = conn.setup().roots[screen].root;
    let atoms = atoms.reply()?;
    conn.xfixes_query_version(1, 0)?.reply()?;
    Ok((conn, root, atoms))
}

/// Creates a window that is never shown, which hears of changes to its
/// properties and to the clipboard's owner.
fn create_window(
    conn: &RustConnection,
    root: Window,
    atoms: &Atoms,
) -> Result<Window, ReplyOrIdError> {
    let window = conn.generate_id()?;
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
    conn.xfixes_select_selection_input(
        window,
        atoms.CLIPBOARD,
        SelectionEventMask::SET_SELECTION_OWNER
            | SelectionEventMask::SELECTION_WINDOW_DESTROY
            | SelectionEventMask::SELECTION_CLIENT_CLOSE,
    )?;
    Ok(window)
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
    root: Window,
    /// Replaced after a conversion that was given up on, so that whatever
    /// is still on its way for that one never reaches the next.
    window: Window,
    atoms: Atoms,
    /// Where conversions are delivered, taking turns.
    properties: [Atom; TRANSFER_PROPERTIES],
    next_property: usize,
    /// Atom names already asked for. The server never forgets an atom.
    names: HashMap<Atom, String>,
    /// The read in progress: the types offered, by atom and name, and the
    /// clipboard's owner when it began.
    offered: Vec<(Atom, String)>,
    owner: Window,
    /// The clipboard changed since the read began, as XFixes reported.
    changed: bool,
    deadline: Instant,
}

impl Reader {
    fn connect(display: Option<&str>) -> Result<Self, ReplyOrIdError> {
        let (conn, root, atoms) = connect(display)?;
        let window = create_window(&conn, root, &atoms)?;
        let cookies = (0..TRANSFER_PROPERTIES)
            .map(|i| conn.intern_atom(false, format!("LINK2LINUX_CLIPBOARD_{i}").as_bytes()))
            .collect::<Result<Vec<_>, _>>()?;
        let mut properties = [NONE; TRANSFER_PROPERTIES];
        for (property, cookie) in properties.iter_mut().zip(cookies) {
            *property = cookie.reply()?.atom;
        }
        conn.flush()?;
        Ok(Self {
            conn,
            root,
            window,
            atoms,
            properties,
            next_property: 0,
            names: HashMap::new(),
            offered: Vec::new(),
            owner: NONE,
            changed: false,
            deadline: Instant::now(),
        })
    }

    /// Replaces the window after a conversion that was given up on: an
    /// answer still on its way, or an app still sending its text in parts,
    /// then finds no window and stops.
    fn replace_window(&mut self) -> Result<(), PlatformError> {
        self.conn.destroy_window(self.window).map_err(unavailable)?;
        self.window = create_window(&self.conn, self.root, &self.atoms).map_err(unavailable)?;
        self.conn.flush().map_err(unavailable)?;
        Ok(())
    }

    fn selection_owner(&self) -> Result<Window, PlatformError> {
        Ok(self
            .conn
            .get_selection_owner(self.atoms.CLIPBOARD)
            .map_err(unavailable)?
            .reply()
            .map_err(unavailable)?
            .owner)
    }

    /// Goes through the events already received, noting clipboard changes.
    fn drain(&mut self) -> Result<(), PlatformError> {
        while let Some(event) = self.conn.poll_for_event().map_err(unavailable)? {
            self.note(&event);
        }
        Ok(())
    }

    fn note(&mut self, event: &Event) {
        if let Event::XfixesSelectionNotify(notify) = event
            && notify.selection == self.atoms.CLIPBOARD
        {
            self.changed = true;
        }
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
    fn convert(&mut self, target: Atom, limit: usize) -> Result<Option<Vec<u8>>, PlatformError> {
        let converted = self.try_convert(target, limit);
        if converted.is_err() {
            let _ = self.replace_window();
        }
        converted
    }

    fn try_convert(
        &mut self,
        target: Atom,
        limit: usize,
    ) -> Result<Option<Vec<u8>>, PlatformError> {
        let property = self.properties[self.next_property];
        self.next_property = (self.next_property + 1) % TRANSFER_PROPERTIES;
        let (window, clipboard) = (self.window, self.atoms.CLIPBOARD);
        self.conn
            .delete_property(window, property)
            .map_err(unavailable)?;
        self.conn
            .convert_selection(window, clipboard, target, property, CURRENT_TIME)
            .map_err(unavailable)?;
        self.conn.flush().map_err(unavailable)?;
        // Its answer names its property, or none for a refusal.
        let answer = self.next_event(|event| {
            matches!(event, Event::SelectionNotify(notify)
                if notify.requestor == window
                    && notify.selection == clipboard
                    && notify.target == target
                    && (notify.property == property || notify.property == NONE))
        })?;
        let Some(Event::SelectionNotify(notify)) = answer else {
            return Err(unavailable("the app that copied did not answer"));
        };
        if notify.property == NONE {
            return Ok(None);
        }
        let first = self.take_property(property, limit)?;
        // Answered without writing anything: a refusal in all but name.
        if first.type_ == NONE {
            return Ok(None);
        }
        if first.type_ != self.atoms.INCR {
            return Ok(Some(first.value));
        }
        // Sent in parts: each comes once the last is deleted, and an empty
        // one ends it.
        let mut parts = Parts::new(limit);
        loop {
            let written = self.next_event(|event| {
                matches!(event, Event::PropertyNotify(change)
                    if change.window == window
                        && change.atom == property
                        && change.state == Property::NEW_VALUE)
            })?;
            if written.is_none() {
                return Err(unavailable("the app that copied stopped sending"));
            }
            let part = self.take_property(property, limit)?;
            if parts.add(&part.value)? {
                return Ok(Some(parts.data));
            }
        }
    }

    /// Reads and deletes `property` of the window, up to `limit` bytes.
    fn take_property(&self, property: Atom, limit: usize) -> Result<Received, PlatformError> {
        let long_length = u32::try_from(limit / 4 + 1).unwrap_or(u32::MAX);
        let reply = self
            .conn
            .get_property(true, self.window, property, AtomEnum::ANY, 0, long_length)
            .map_err(unavailable)?
            .reply()
            .map_err(unavailable)?;
        if reply.bytes_after > 0 || reply.value.len() > limit {
            return Err(too_long());
        }
        Ok(Received {
            type_: reply.type_,
            value: reply.value,
        })
    }

    /// The server's current time, which taking the clipboard needs: learnt
    /// from the time stamp of a change to one of our own properties.
    fn server_time(&mut self) -> Result<Timestamp, PlatformError> {
        let (window, clock) = (self.window, self.atoms.CLOCK);
        self.deadline = Instant::now() + TRANSFER_TIMEOUT;
        self.conn
            .change_property8(PropMode::APPEND, window, clock, AtomEnum::STRING, &[])
            .map_err(unavailable)?;
        self.conn.flush().map_err(unavailable)?;
        let changed = self.next_event(|event| {
            matches!(event, Event::PropertyNotify(change)
                if change.window == window && change.atom == clock)
        })?;
        match changed {
            Some(Event::PropertyNotify(change)) => Ok(change.time),
            _ => Err(unavailable("the X server did not answer")),
        }
    }

    /// The next event `wanted` accepts, noting clipboard changes among the
    /// others and dropping them, or `None` after [`TRANSFER_TIMEOUT`] or at
    /// the read's deadline, whichever comes first.
    fn next_event(
        &mut self,
        wanted: impl Fn(&Event) -> bool,
    ) -> Result<Option<Event>, PlatformError> {
        let deadline = (Instant::now() + TRANSFER_TIMEOUT).min(self.deadline);
        loop {
            while let Some(event) = self.conn.poll_for_event().map_err(unavailable)? {
                if wanted(&event) {
                    return Ok(Some(event));
                }
                self.note(&event);
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Ok(None);
            }
            wait(&self.conn, None, Some(left))?;
        }
    }
}

impl Source for Reader {
    fn begin(&mut self, deadline: Instant) -> Result<Option<Vec<String>>, PlatformError> {
        self.deadline = deadline;
        self.offered.clear();
        // The owner's reply comes after every event sent before it, so the
        // changes it brings in are older than this read.
        self.owner = self.selection_owner()?;
        self.drain()?;
        self.changed = false;
        if self.owner == NONE {
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
        self.offered = self.names(&atoms)?;
        Ok(Some(
            self.offered.iter().map(|(_, name)| name.clone()).collect(),
        ))
    }

    fn fetch(&mut self, mime_type: &str, limit: usize) -> Result<Option<Vec<u8>>, PlatformError> {
        let Some(&(atom, _)) = self.offered.iter().find(|(_, name)| name == mime_type) else {
            return Ok(None);
        };
        self.convert(atom, limit)
    }

    fn changed_since_begin(&mut self) -> Result<bool, PlatformError> {
        // A round trip brings in every change reported before it.
        let owner = self.selection_owner()?;
        self.drain()?;
        Ok(self.changed || owner != self.owner)
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
    /// [`WAITING`] until the owner thread claims it or the worker gives up
    /// on it, whichever comes first.
    state: Arc<AtomicU8>,
    done: mpsc::SyncSender<Result<(), PlatformError>>,
}

const WAITING: u8 = 0;
const CLAIMED: u8 = 1;
const GIVEN_UP: u8 = 2;

impl Take {
    /// Claims it for the owner thread; `false` when the worker gave up on
    /// it, and it must not happen.
    fn claim(&self) -> bool {
        self.state
            .compare_exchange(WAITING, CLAIMED, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }
}

fn closed() -> PlatformError {
    unavailable("the X11 clipboard connection closed")
}

impl OwnerHandle {
    /// Has the owner thread take the clipboard. A take it does not get to in
    /// time is given up on, and never happens later.
    fn take(&mut self, text: ClipboardText, time: Timestamp) -> Result<(), PlatformError> {
        let (done, result) = mpsc::sync_channel(1);
        let state = Arc::new(AtomicU8::new(WAITING));
        self.commands
            .send(Take {
                text,
                time,
                state: state.clone(),
                done,
            })
            .map_err(|_| closed())?;
        match self.wake.write(&[0]) {
            Ok(_) => {}
            // Full of wake-ups not read yet: it wakes anyway.
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
            Err(_) => return Err(closed()),
        }
        match result.recv_timeout(TRANSFER_TIMEOUT) {
            Ok(taken) => taken,
            Err(RecvTimeoutError::Timeout) => {
                if state
                    .compare_exchange(WAITING, GIVEN_UP, Ordering::AcqRel, Ordering::Acquire)
                    .is_ok()
                {
                    Err(unavailable("the clipboard was not taken in time"))
                } else {
                    // Claimed just now: it is being taken.
                    result.recv().map_err(|_| closed())?
                }
            }
            Err(RecvTimeoutError::Disconnected) => Err(closed()),
        }
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
    fn connect(display: Option<&str>) -> Result<Self, ReplyOrIdError> {
        let (conn, root, atoms) = connect(display)?;
        let window = create_window(&conn, root, &atoms)?;
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

    fn run(mut self, commands: &mpsc::Receiver<Take>, wake: &UnixStream, changes: &Reporter) {
        if let Err(error) = self.serve(commands, wake, changes) {
            tracing::warn!(%error, "the X11 clipboard connection closed");
        }
    }

    /// Serves until the clipboard is dropped, which closes `wake`.
    fn serve(
        &mut self,
        commands: &mpsc::Receiver<Take>,
        mut wake: &UnixStream,
        changes: &Reporter,
    ) -> Result<(), ReplyOrIdError> {
        wake.set_nonblocking(true)
            .map_err(ConnectionError::IoError)?;
        loop {
            while let Ok(take) = commands.try_recv() {
                if take.claim() {
                    let result = self.take(&take.text, take.time);
                    let _ = take.done.send(result);
                }
            }
            self.give_up_stalled(Instant::now())?;
            while let Some(event) = self.conn.poll_for_event()? {
                self.handle(event, changes)?;
            }
            self.conn.flush()?;
            // Flushing may read what the server sent meanwhile into the
            // connection's queue, where waiting on the socket would miss it.
            if let Some(event) = self.conn.poll_for_event()? {
                self.handle(event, changes)?;
                continue;
            }
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

    fn handle(&mut self, event: Event, changes: &Reporter) -> Result<(), ReplyOrIdError> {
        match event {
            Event::XfixesSelectionNotify(notify) if notify.selection == self.atoms.CLIPBOARD => {
                changes.report_change();
            }
            Event::SelectionRequest(request) => self.answer(&request)?,
            Event::SelectionClear(clear)
                if clear.selection == self.atoms.CLIPBOARD && clear.owner == self.window =>
            {
                // Late news when the clipboard was taken again since: the
                // server says who holds it now.
                let owner = self
                    .conn
                    .get_selection_owner(self.atoms.CLIPBOARD)?
                    .reply()?
                    .owner;
                if owner != self.window {
                    self.owned = None;
                }
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
        // Asked again on the same property: the earlier transfer there ends.
        self.transfers
            .retain(|transfer| transfer.requestor != requestor || transfer.property != property);
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
    use std::sync::atomic::AtomicBool;

    use super::super::tests::{NO_X_SERVER, PrivateX};
    use super::super::{Link, link};
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

    #[test]
    fn fails_at_once_without_an_x_server() {
        // No server on that display, and a name like an unset `DISPLAY`.
        for display in [NO_X_SERVER, ""] {
            let started = Instant::now();
            let (reporter, _) = link();
            assert!(X11Clipboard::open(reporter, Some(display)).is_err());
            assert!(started.elapsed() < OPEN_TIMEOUT, "{display:?}");
        }
    }

    #[test]
    fn a_take_given_up_on_never_happens() {
        // An owner thread that never gets to it.
        let (commands, received) = mpsc::channel();
        let (wake, _woken) = UnixStream::pair().unwrap();
        wake.set_nonblocking(true).unwrap();
        let mut owner = OwnerHandle { commands, wake };
        let text = ClipboardText {
            text: "late".into(),
            sensitive: false,
        };
        let started = Instant::now();
        let error = owner.take(text, 1).unwrap_err();
        assert!(started.elapsed() >= TRANSFER_TIMEOUT);
        assert!(error.to_string().contains("not taken in time"), "{error}");
        // When the owner thread gets to it after all, it leaves it.
        let take = received.try_recv().unwrap();
        assert!(!take.claim());
    }

    /// Opens the clipboard of the X server on `display`.
    fn open(display: &str, part_bytes: Option<usize>) -> (X11Clipboard, Link) {
        let (reporter, link) = link();
        let clipboard =
            X11Clipboard::open_with(reporter, Some(display), part_bytes).expect("the X server");
        (clipboard, link)
    }

    async fn next_change(link: &Link) {
        tokio::time::timeout(TRANSFER_TIMEOUT, link.changed())
            .await
            .expect("a change in time")
            .unwrap();
    }

    fn text(text: &str, sensitive: bool) -> ClipboardText {
        ClipboardText {
            text: text.into(),
            sensitive,
        }
    }

    /// What a [`FakeOwner`] answers to a request.
    enum Answer {
        /// The targets it offers.
        Targets,
        /// Data of the type named.
        Data(&'static str, Vec<u8>),
        Refuse,
    }

    /// Another app, holding the clipboard of a private X server: it offers
    /// `targets` and answers each request, on a thread of its own, as
    /// `answer` says, which may also wait first.
    struct FakeOwner {
        stop: Arc<AtomicBool>,
        thread: Option<thread::JoinHandle<()>>,
    }

    impl FakeOwner {
        fn start(
            display: &str,
            targets: &[&str],
            answer: impl FnMut(&str) -> Answer + Send + 'static,
        ) -> Self {
            let (conn, root, atoms) = connect(Some(display)).unwrap();
            let window = create_window(&conn, root, &atoms).unwrap();
            let offered: Vec<Atom> = targets.iter().map(|name| intern(&conn, name)).collect();
            conn.set_selection_owner(window, atoms.CLIPBOARD, CURRENT_TIME)
                .unwrap();
            let owner = conn.get_selection_owner(atoms.CLIPBOARD).unwrap();
            assert_eq!(owner.reply().unwrap().owner, window);
            let stop = Arc::new(AtomicBool::new(false));
            let stopping = stop.clone();
            let thread = thread::spawn(move || serve_fake(&conn, &offered, answer, &stopping));
            Self {
                stop,
                thread: Some(thread),
            }
        }
    }

    impl Drop for FakeOwner {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Release);
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }

    fn intern(conn: &RustConnection, name: &str) -> Atom {
        conn.intern_atom(false, name.as_bytes())
            .unwrap()
            .reply()
            .unwrap()
            .atom
    }

    fn serve_fake(
        conn: &RustConnection,
        offered: &[Atom],
        mut answer: impl FnMut(&str) -> Answer,
        stop: &AtomicBool,
    ) {
        while !stop.load(Ordering::Acquire) {
            let Some(event) = conn.poll_for_event().unwrap() else {
                thread::sleep(Duration::from_millis(5));
                continue;
            };
            // Errors about windows gone meanwhile are dropped with the rest.
            let Event::SelectionRequest(request) = event else {
                continue;
            };
            let name = conn.get_atom_name(request.target).unwrap().reply().unwrap();
            let property = match answer(&String::from_utf8_lossy(&name.name)) {
                Answer::Targets => {
                    conn.change_property32(
                        PropMode::REPLACE,
                        request.requestor,
                        request.property,
                        AtomEnum::ATOM,
                        offered,
                    )
                    .unwrap();
                    request.property
                }
                Answer::Data(kind, data) => {
                    let kind = intern(conn, kind);
                    conn.change_property8(
                        PropMode::REPLACE,
                        request.requestor,
                        request.property,
                        kind,
                        &data,
                    )
                    .unwrap();
                    request.property
                }
                Answer::Refuse => NONE,
            };
            conn.send_event(
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
                    property,
                },
            )
            .unwrap();
            conn.flush().unwrap();
        }
    }

    // The tests below start their own Xvfb, so they never reach the desktop
    // session: `cargo test -p link2linux-platform -- --ignored own_xvfb`

    #[tokio::test]
    #[ignore = "starts its own Xvfb"]
    async fn own_xvfb_x11_clipboard_round_trip() {
        let server = PrivateX::start();
        // Another app, as far as the X server can tell: its own connections.
        let (reader, changed) = open(&server.display, None);
        // Sends anything over 4 KiB in parts.
        let (writer, _) = open(&server.display, Some(4096));
        let secret = text(&"ü".repeat(128 << 10), true);
        writer.write(secret.clone()).await.unwrap();
        next_change(&changed).await;
        assert_eq!(reader.read().await.unwrap(), Some(secret.clone()));
        // Its own text too.
        assert_eq!(writer.read().await.unwrap(), Some(secret));

        let plain = text("plain", false);
        writer.write(plain.clone()).await.unwrap();
        next_change(&changed).await;
        assert_eq!(reader.read().await.unwrap(), Some(plain));

        // 256 KiB in one request, as BIG-REQUESTS allows.
        let (whole, _) = open(&server.display, None);
        let big = text(&"x".repeat(256 << 10), false);
        whole.write(big.clone()).await.unwrap();
        assert_eq!(reader.read().await.unwrap(), Some(big));
    }

    #[tokio::test]
    #[ignore = "starts its own Xvfb"]
    async fn own_xvfb_a_password_copied_mid_read_is_not_shared_unmarked() {
        let server = PrivateX::start();
        let (clipboard, _) = open(&server.display, None);
        let (password_manager, _) = open(&server.display, None);
        // Plain text, whose app answers which types it offers only once the
        // password manager copied, so the read fetches the password next.
        let (copy_now, time_to_copy) = tokio::sync::oneshot::channel();
        let mut copy_now = Some(copy_now);
        let _plain = FakeOwner::start(&server.display, &["TARGETS", "UTF8_STRING"], move |name| {
            if name != "TARGETS" {
                return Answer::Data("UTF8_STRING", b"note".to_vec());
            }
            if let Some(copy_now) = copy_now.take() {
                let _ = copy_now.send(());
                thread::sleep(Duration::from_millis(300));
            }
            Answer::Targets
        });
        let password = text("hunter2", true);
        let (read, wrote) = tokio::join!(clipboard.read(), async {
            time_to_copy.await.unwrap();
            password_manager.write(password.clone()).await
        });
        wrote.unwrap();
        assert_eq!(read.unwrap(), Some(password));
    }

    #[tokio::test]
    #[ignore = "starts its own Xvfb"]
    async fn own_xvfb_types_in_any_letter_case() {
        let server = PrivateX::start();
        let targets = [
            "TARGETS",
            "UTF8_STRING",
            "TEXT/URI-LIST",
            "X-KDE-PASSWORDMANAGERHINT",
        ];
        let _owner = FakeOwner::start(&server.display, &targets, |name| match name {
            "TARGETS" => Answer::Targets,
            "UTF8_STRING" => Answer::Data("UTF8_STRING", b"hunter2".to_vec()),
            "TEXT/URI-LIST" => Answer::Data("TEXT/URI-LIST", b"https://example.com/".to_vec()),
            "X-KDE-PASSWORDMANAGERHINT" => Answer::Data("STRING", b"secret".to_vec()),
            _ => Answer::Refuse,
        });
        let (clipboard, _) = open(&server.display, None);
        // Twice: the first read leaves the clipboard working.
        for _ in 0..2 {
            assert_eq!(clipboard.read().await.unwrap(), Some(text("hunter2", true)));
        }
    }

    #[tokio::test]
    #[ignore = "starts its own Xvfb"]
    async fn own_xvfb_a_hint_that_says_nothing_counts_as_secret() {
        let server = PrivateX::start();
        let (clipboard, _) = open(&server.display, None);
        let targets = ["TARGETS", "UTF8_STRING", "x-kde-passwordManagerHint"];
        for hint in [Some(Vec::new()), None] {
            let _owner = FakeOwner::start(&server.display, &targets, move |name| match name {
                "TARGETS" => Answer::Targets,
                "UTF8_STRING" => Answer::Data("UTF8_STRING", b"hunter2".to_vec()),
                _ => hint
                    .clone()
                    .map_or(Answer::Refuse, |hint| Answer::Data("STRING", hint)),
            });
            assert_eq!(clipboard.read().await.unwrap(), Some(text("hunter2", true)));
        }
    }

    #[tokio::test]
    #[ignore = "starts its own Xvfb"]
    async fn own_xvfb_a_late_answer_never_reaches_the_next_read() {
        let server = PrivateX::start();
        let (clipboard, _) = open(&server.display, None);
        let targets = ["TARGETS", "UTF8_STRING"];
        // Refuses, but only after the read gave up on it.
        let slow = FakeOwner::start(&server.display, &targets, |name| {
            if name == "TARGETS" {
                thread::sleep(TRANSFER_TIMEOUT + Duration::from_millis(500));
            }
            Answer::Refuse
        });
        assert!(clipboard.read().await.is_err());
        // Another app copies, and answers after the slow app's refusal.
        let _next = FakeOwner::start(&server.display, &targets, |name| {
            if name == "TARGETS" {
                thread::sleep(Duration::from_secs(1));
                return Answer::Targets;
            }
            Answer::Data("UTF8_STRING", b"from the next app".to_vec())
        });
        let read = clipboard.read().await.unwrap();
        assert_eq!(read, Some(text("from the next app", false)));
        drop(slow);
    }

    #[test]
    #[ignore = "starts its own Xvfb"]
    fn own_xvfb_a_slow_app_cannot_hold_a_read_past_its_deadline() {
        let server = PrivateX::start();
        // Takes 1.5 s over each answer: under the time one may take, but
        // three of them are over the read's deadline.
        let targets = ["TARGETS", "UTF8_STRING", "x-kde-passwordManagerHint"];
        let _owner = FakeOwner::start(&server.display, &targets, |name| {
            thread::sleep(Duration::from_millis(1500));
            match name {
                "TARGETS" => Answer::Targets,
                _ => Answer::Data("UTF8_STRING", b"slow".to_vec()),
            }
        });
        let mut reader = Reader::connect(Some(&server.display)).unwrap();
        let started = Instant::now();
        let deadline = started + Duration::from_millis(2500);
        assert!(super::super::read(&mut reader, deadline).is_err());
        assert!(started.elapsed() < Duration::from_secs(3));
    }

    #[tokio::test]
    #[ignore = "starts its own Xvfb"]
    async fn own_xvfb_a_stale_selection_clear_does_not_stop_serving() {
        let server = PrivateX::start();
        let (writer, _) = open(&server.display, None);
        let (reader, _) = open(&server.display, None);
        let written = text("from the phone", false);
        writer.write(written.clone()).await.unwrap();
        // News that another app took the clipboard, which it did not.
        let (conn, _, atoms) = connect(Some(&server.display)).unwrap();
        let owner = conn.get_selection_owner(atoms.CLIPBOARD).unwrap();
        let owner = owner.reply().unwrap().owner;
        conn.send_event(
            false,
            owner,
            EventMask::NO_EVENT,
            x11rb::protocol::xproto::SelectionClearEvent {
                response_type: x11rb::protocol::xproto::SELECTION_CLEAR_EVENT,
                sequence: 0,
                time: 0,
                owner,
                selection: atoms.CLIPBOARD,
            },
        )
        .unwrap();
        conn.sync().unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(reader.read().await.unwrap(), Some(written));
    }
}
