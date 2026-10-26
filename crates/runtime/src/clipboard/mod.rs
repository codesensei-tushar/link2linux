//! The clipboard shared with the paired phones.
//!
//! The core's [`ClipboardHub`] and one [`ClipboardService`] per phone apply
//! the protocol's rules. This module connects them to the desktop:
//!
//! - [`watch`] follows the desktop clipboard where the desktop session lets
//!   a background app do that, and hands each new copy to every phone's
//!   session loop;
//! - elsewhere, [`window_focus`] puts the text that waits for the window on
//!   the clipboard when the window gains focus, or reads the clipboard
//!   then;
//! - [`send_now`] sends the clipboard to every phone when the user asks;
//! - [`follow`] keeps the [`ClipboardStatus`] the clipboard page shows,
//!   and the pop-up that offers text waiting for the window.
//!
//! [`ClipboardHub`]: link2linux_core::clipboard::ClipboardHub

use std::sync::Arc;

use link2linux_core::clipboard::{Clip, ClipboardEvent, ClipboardService, Origin};
use link2linux_platform::{
    Clipboard, ClipboardAccess, ClipboardText, DesktopNotification, PlatformError,
};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::{broadcast, watch};
use tokio::time::Instant;

use crate::link::lock;
use crate::notify::BoxFuture;
use crate::shared::{Core, now_unix_ms};

mod status;

pub(crate) use status::{Board, bytes};
pub use status::{
    ClipboardActivity, ClipboardDirection, ClipboardMode, ClipboardPhone, ClipboardPhoneState,
    ClipboardProblem, ClipboardProblemKind, ClipboardStatus, HeldClipboard, MAX_ACTIVITY,
};

/// A [`Clipboard`] usable as a trait object.
pub trait ClipboardPort: Send + Sync {
    /// See [`Clipboard::access`].
    fn access(&self) -> ClipboardAccess;

    /// See [`Clipboard::read`].
    fn read(&self) -> BoxFuture<'_, Result<Option<ClipboardText>, PlatformError>>;

    /// See [`Clipboard::write`].
    fn write<'a>(&'a self, text: &'a ClipboardText) -> BoxFuture<'a, Result<(), PlatformError>>;

    /// See [`Clipboard::changed`].
    fn changed(&self) -> BoxFuture<'_, Result<(), PlatformError>>;
}

impl<C: Clipboard> ClipboardPort for C {
    fn access(&self) -> ClipboardAccess {
        Clipboard::access(self)
    }

    fn read(&self) -> BoxFuture<'_, Result<Option<ClipboardText>, PlatformError>> {
        Box::pin(Clipboard::read(self))
    }

    fn write<'a>(&'a self, text: &'a ClipboardText) -> BoxFuture<'a, Result<(), PlatformError>> {
        Box::pin(Clipboard::write(self, text))
    }

    fn changed(&self) -> BoxFuture<'_, Result<(), PlatformError>> {
        Box::pin(Clipboard::changed(self))
    }
}

/// The desktop clipboard shared by every phone's service, used as the
/// [`Clipboard`] each service wants to own.
#[derive(Clone)]
pub struct SharedClipboard(pub Arc<dyn ClipboardPort>);

impl Clipboard for SharedClipboard {
    fn access(&self) -> ClipboardAccess {
        self.0.access()
    }

    async fn read(&self) -> Result<Option<ClipboardText>, PlatformError> {
        self.0.read().await
    }

    async fn write(&self, text: &ClipboardText) -> Result<(), PlatformError> {
        self.0.write(text).await
    }

    async fn changed(&self) -> Result<(), PlatformError> {
        self.0.changed().await
    }
}

/// The clipboard of a desktop without one: every use fails, and the
/// feature is reported unavailable, so the phones send nothing.
pub(crate) struct NoClipboard;

impl Clipboard for NoClipboard {
    fn access(&self) -> ClipboardAccess {
        ClipboardAccess::Background
    }

    async fn read(&self) -> Result<Option<ClipboardText>, PlatformError> {
        Err(PlatformError::Unsupported("clipboard"))
    }

    async fn write(&self, _text: &ClipboardText) -> Result<(), PlatformError> {
        Err(PlatformError::Unsupported("clipboard"))
    }

    async fn changed(&self) -> Result<(), PlatformError> {
        std::future::pending().await
    }
}

/// One phone's clipboard service, as the runtime runs it.
pub type ClipboardDriver = ClipboardService<SharedClipboard>;

/// A copy on its way to a phone's session loop.
#[derive(Debug, Clone)]
pub struct Outgoing {
    /// The copy.
    pub clip: Arc<Clip>,
    /// Whether the desktop shares it by itself or the user asked.
    pub origin: Origin,
}

/// Where a phone's session loop finds the newest copy to push. Only the
/// newest matters, so copies never pile up behind a loop that is busy.
pub type Copies = watch::Receiver<Option<Outgoing>>;

/// What the runtime keeps for the clipboard page.
#[derive(Default)]
pub(crate) struct Page {
    pub board: Board,
    /// The pop-up that offers text waiting for the window.
    pub banner: Option<link2linux_platform::NotificationId>,
}

/// Hands `clip` to the session loop of every phone, replacing a copy it
/// has not taken yet.
pub(crate) fn share(core: &Core, clip: &Arc<Clip>, origin: Origin) {
    for slot in lock(&core.slots).values() {
        slot.copies.send_replace(Some(Outgoing {
            clip: clip.clone(),
            origin,
        }));
    }
}

/// Follows the desktop clipboard and shares each new copy, until the
/// runtime stops or the clipboard can no longer be followed. Returns at
/// once on a desktop whose session does not let a background app follow
/// the clipboard.
pub(crate) async fn watch(core: Arc<Core>) {
    let Some(clipboard) = core.clipboard.clone() else {
        return;
    };
    if Clipboard::access(&clipboard) != ClipboardAccess::Background {
        return;
    }
    loop {
        let changed = tokio::select! {
            () = core.cancel.cancelled() => return,
            changed = Clipboard::changed(&clipboard) => changed,
        };
        if let Err(error) = changed {
            // The connection to the display server is gone, and with it
            // reading and writing.
            tracing::warn!(%error, "the clipboard can no longer be followed");
            return;
        }
        match Clipboard::read(&clipboard).await {
            Ok(Some(text)) => {
                if let Some(clip) = core.clipboard_hub.copied(text, Instant::now()) {
                    share(&core, &clip, Origin::Automatic);
                }
            }
            Ok(None) => {}
            Err(error) => tracing::debug!(%error, "could not read the clipboard"),
        }
    }
}

/// The window gained or lost focus. On a desktop that lets the app use its
/// clipboard only while the window has focus, gaining it puts the text that
/// waits on the clipboard, or else shares a copy made while the window was
/// in the background.
pub(crate) fn window_focus(core: &Arc<Core>, focused: bool) {
    core.clipboard_hub.set_focused(focused);
    let Some(clipboard) = core.clipboard.clone() else {
        return;
    };
    if !focused || Clipboard::access(&clipboard) != ClipboardAccess::FocusOnly {
        return;
    }
    let task = core.clone();
    core.handle.spawn(async move {
        let copy = task.clipboard_hub.focused(&clipboard, Instant::now()).await;
        if let Some(clip) = copy {
            share(&task, &clip, Origin::Automatic);
        }
    });
}

/// Sends what the clipboard holds to every phone, as the user asked. Why it
/// did not go shows on the clipboard page.
pub(crate) fn send_now(core: &Arc<Core>) {
    let task = core.clone();
    core.handle.spawn(async move {
        let clipboard = task.clipboard_port();
        let Ok(clip) = task
            .clipboard_hub
            .send_clipboard(&clipboard, Instant::now())
            .await
        else {
            // The hub reported why.
            return;
        };
        let nobody = {
            let mut page = lock(&task.clipboard_page);
            let nobody = !page.board.any_ready();
            if nobody {
                page.board.no_phone(now_unix_ms());
            }
            nobody
        };
        if nobody {
            task.emit_clipboard();
        }
        // A phone that connects within a minute still gets it.
        share(&task, &clip, Origin::Explicit);
    });
}

/// Keeps the clipboard page's state from the hub's `events`, offers text
/// that waits for the window in a pop-up, and reports each change, until
/// the runtime stops. [`Core::clipboard_changed`] wakes it to look again.
pub(crate) async fn follow(core: Arc<Core>, mut events: broadcast::Receiver<ClipboardEvent>) {
    loop {
        let event = tokio::select! {
            () = core.cancel.cancelled() => return,
            () = core.clipboard_nudge.notified() => None,
            event = events.recv() => match event {
                Ok(event) => Some(event),
                Err(RecvError::Lagged(missed)) => {
                    tracing::warn!(missed, "the clipboard page missed some changes");
                    None
                }
                Err(RecvError::Closed) => return,
            },
        };
        if let Some(event) = &event {
            let name_of = |id: &str| core.phone_name(id);
            lock(&core.clipboard_page)
                .board
                .apply(event, &name_of, now_unix_ms());
        }
        offer(&core, event.as_ref()).await;
        core.emit_clipboard();
    }
}

/// Shows the pop-up for text that just arrived to wait for the window, and
/// takes it down once no text waits.
async fn offer(core: &Core, event: Option<&ClipboardEvent>) {
    let held = core.clipboard_hub.held();
    let banner = lock(&core.clipboard_page).banner;
    match held {
        Some(held) if matches!(event, Some(ClipboardEvent::Received { held: true, .. })) => {
            let popup = DesktopNotification {
                app_name: "link2linux".into(),
                summary: format!("Clipboard from {}", core.phone_name(&held.device_id)),
                body: "Click to put it on this computer's clipboard.".into(),
                ..DesktopNotification::default()
            };
            match core.presenter.show_own(&popup, banner).await {
                Ok(id) => lock(&core.clipboard_page).banner = Some(id),
                Err(error) => tracing::warn!(%error, "could not offer the phone's clipboard"),
            }
        }
        None => {
            let shown = lock(&core.clipboard_page).banner.take();
            if let Some(id) = shown
                && let Err(error) = core.presenter.close(id).await
            {
                tracing::debug!(%error, "could not take down the clipboard pop-up");
            }
        }
        Some(_) => {}
    }
}
