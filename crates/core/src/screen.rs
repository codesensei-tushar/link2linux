//! The phone's screen, shown and controlled on the desktop.
//!
//! The phone encodes its screen as a video stream; the desktop decodes and
//! shows it, and sends touches and keys back. Video runs on its own stream
//! next to the session, so it never delays other features' messages.
//!
//! Stub: the types and the [`Screen`] trait are in place; the wire messages
//! and the video stream come with this feature's schema.

use crate::device::FeatureError;

/// What the desktop asks for when it starts mirroring.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StreamSettings {
    /// Longest edge of the video, in pixels. The phone keeps its aspect
    /// ratio.
    pub max_size: u32,
    /// Frame rate cap.
    pub max_fps: u32,
    /// Target bit rate, in bits per second.
    pub bit_rate: u32,
}

impl Default for StreamSettings {
    fn default() -> Self {
        Self {
            max_size: 1920,
            max_fps: 60,
            bit_rate: 8_000_000,
        }
    }
}

/// Input sent from the desktop to the phone. Coordinates are in video
/// pixels, as last reported by [`ScreenEvent::Started`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum InputEvent {
    /// A tap at a point.
    Tap {
        /// Horizontal position.
        x: u32,
        /// Vertical position.
        y: u32,
    },
    /// A swipe from one point to another.
    Swipe {
        /// Start point.
        from: (u32, u32),
        /// End point.
        to: (u32, u32),
        /// How long the gesture takes, in milliseconds.
        duration_ms: u32,
    },
    /// Text typed on the desktop keyboard.
    Text(String),
    /// The system Back button.
    Back,
    /// The system Home button.
    Home,
}

/// What the phone reports about mirroring.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ScreenEvent {
    /// Mirroring started, or the video size changed (for example after a
    /// rotation).
    Started {
        /// Video width in pixels.
        width: u32,
        /// Video height in pixels.
        height: u32,
    },
    /// Mirroring stopped.
    Stopped {
        /// Why, for the user.
        reason: String,
    },
}

/// Requests the desktop makes about screen mirroring.
pub trait Screen: Send + Sync {
    /// Asks the phone to start mirroring. The user confirms on the phone.
    fn start(
        &self,
        settings: StreamSettings,
    ) -> impl Future<Output = Result<(), FeatureError>> + Send;

    /// Stops mirroring.
    fn stop(&self) -> impl Future<Output = Result<(), FeatureError>> + Send;

    /// Sends one input event to the phone.
    fn send_input(
        &self,
        event: InputEvent,
    ) -> impl Future<Output = Result<(), FeatureError>> + Send;
}
