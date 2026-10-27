//! Showing each phone's screen, and controlling the phone from there.
//!
//! The core's [`ScreenService`] applies the protocol's rules. Each phone's
//! [`ScreenDriver`] runs one in its session loop and puts what the UI shows
//! on the phone's [`PhoneBoard`]. The frames go another way, so the session
//! loop never waits for the window: a task per stream takes them from the
//! service's queue and hands each one, as a [`packet`], to the
//! [`ScreenViewer`] the window gave with its start. [`dto`] has what the
//! UI receives and sends.

pub mod dto;

use std::sync::Arc;

pub use link2linux_core::screen::{Codec, VideoFormat, VideoFrame};
use link2linux_core::screen::{Input, ScreenRequest, ScreenService, hand_off};
use link2linux_core::session::{SessionError, SessionEvent};
use tokio::sync::mpsc::Receiver;
use tokio::time::Instant;

pub use dto::{
    ScreenCodec, ScreenEnd, ScreenFormat, ScreenInput, ScreenInputProblem, ScreenKey, ScreenPhase,
    ScreenPoint, ScreenProblem, ScreenStatus,
};

use crate::notify::PhoneSession;
use crate::phone::PhoneBoard;

/// Where a stream's frames go: the window that shows them.
pub trait ScreenViewer: Send + Sync {
    /// Shows one frame, encoded as a [`packet`]. Returns `false` once the
    /// window is gone, which stops the stream.
    fn show(&self, packet: Vec<u8>) -> bool;
}

/// What the user asks of a phone's screen.
pub enum ScreenCommand {
    /// Show the phone's screen in `viewer`.
    Start {
        /// The codecs and size the window asks for.
        request: ScreenRequest,
        /// Where the frames go.
        viewer: Arc<dyn ScreenViewer>,
    },
    /// Stop showing it.
    Stop,
    /// Something the user did in the window.
    Input(Input),
    /// The window could not decode a frame, or fell behind.
    KeyFrame,
}

impl std::fmt::Debug for ScreenCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Start { request, .. } => {
                f.debug_struct("Start").field("request", request).finish()
            }
            Self::Stop => f.write_str("Stop"),
            Self::Input(input) => f.debug_tuple("Input").field(input).finish(),
            Self::KeyFrame => f.write_str("KeyFrame"),
        }
    }
}

/// A frame as the window receives it: a header, then the frame's data.
/// Numbers are big-endian.
///
/// | Offset | Size | Content |
/// |---|---|---|
/// | 0 | 1 | 1: a frame |
/// | 1 | 1 | flags: bit 0 a key frame, bit 1 a format follows the header |
/// | 2 | 4 | the format's id |
/// | 6 | 8 | the capture time in microseconds |
///
/// A format, sent with the first frame in it, is the codec (1 H.264,
/// 2 JPEG), the width and the height as 4 bytes each, and the codec's
/// configuration (the avcC record for H.264) with its length as 2 bytes
/// before it.
#[must_use]
pub fn packet(frame: &VideoFrame) -> Vec<u8> {
    let format_len = frame
        .format
        .as_ref()
        .map_or(0, |format| 11 + format.codec_config.len());
    let mut packet = Vec::with_capacity(HEADER_LEN + format_len + frame.data.len());
    packet.push(FRAME);
    let mut flags = 0;
    if frame.key_frame {
        flags |= KEY_FRAME;
    }
    if frame.format.is_some() {
        flags |= HAS_FORMAT;
    }
    packet.push(flags);
    packet.extend_from_slice(&frame.format_id.to_be_bytes());
    packet.extend_from_slice(&frame.timestamp_us.to_be_bytes());
    if let Some(format) = &frame.format {
        packet.push(match format.codec {
            Codec::H264 => 1,
            Codec::Jpeg => 2,
        });
        packet.extend_from_slice(&format.width.to_be_bytes());
        packet.extend_from_slice(&format.height.to_be_bytes());
        // The core keeps the configuration within 4 KiB.
        let config_len = u16::try_from(format.codec_config.len()).unwrap_or(u16::MAX);
        packet.extend_from_slice(&config_len.to_be_bytes());
        packet.extend_from_slice(&format.codec_config[..usize::from(config_len)]);
    }
    packet.extend_from_slice(&frame.data);
    packet
}

const HEADER_LEN: usize = 14;
const FRAME: u8 = 1;
const KEY_FRAME: u8 = 1;
const HAS_FORMAT: u8 = 2;

/// Hands a stream's frames to `viewer` until either side is done. Dropping
/// the queue when the window is gone tells the service to stop the stream.
async fn pump(mut frames: Receiver<VideoFrame>, viewer: Arc<dyn ScreenViewer>) {
    while let Some(frame) = frames.recv().await {
        if !viewer.show(packet(&frame)) {
            tracing::debug!("the window that showed the screen is gone");
            return;
        }
    }
}

/// One phone's screen, as its session loop runs it.
pub struct ScreenDriver {
    service: ScreenService,
    board: PhoneBoard,
}

impl ScreenDriver {
    /// The driver for the phone of `board`, which shows on it.
    #[must_use]
    pub fn new(board: PhoneBoard) -> Self {
        Self {
            service: ScreenService::new(),
            board,
        }
    }

    /// See [`ScreenService::session_started`].
    pub fn session_started(&mut self, session: &PhoneSession) {
        self.service.session_started(session);
        self.publish();
    }

    /// See [`ScreenService::handle_event`].
    ///
    /// # Errors
    ///
    /// Fails when the session fails.
    pub fn handle_event(
        &mut self,
        session: &mut PhoneSession,
        event: SessionEvent,
    ) -> Result<Option<SessionEvent>, SessionError> {
        let rest = self.service.handle_event(session, event);
        self.publish();
        rest
    }

    /// Acts on what the user asked. `session` is `None` between sessions,
    /// when a start tells the user the phone is not connected.
    ///
    /// # Errors
    ///
    /// Fails when the session fails.
    pub fn command(
        &mut self,
        session: Option<&mut PhoneSession>,
        command: ScreenCommand,
    ) -> Result<(), SessionError> {
        let done = match (command, session) {
            (ScreenCommand::Start { request, viewer }, Some(session)) => {
                let (frames, queue) = hand_off();
                tokio::spawn(pump(queue, viewer));
                self.service.start(session, &request, frames)
            }
            (ScreenCommand::Start { .. }, None) => {
                self.service.not_connected();
                Ok(())
            }
            (ScreenCommand::Stop, Some(session)) => self.service.stop(session),
            (ScreenCommand::Input(input), Some(session)) => self.service.input(session, input),
            (ScreenCommand::KeyFrame, Some(session)) => self.service.request_key_frame(session),
            (ScreenCommand::Stop | ScreenCommand::Input(_) | ScreenCommand::KeyFrame, None) => {
                Ok(())
            }
        };
        self.publish();
        done
    }

    /// See [`ScreenService::next_deadline`].
    #[must_use]
    pub fn next_deadline(&self) -> Option<Instant> {
        self.service.next_deadline()
    }

    /// See [`ScreenService::tick`].
    ///
    /// # Errors
    ///
    /// Fails when the session fails.
    pub fn tick(&mut self, session: &mut PhoneSession) -> Result<(), SessionError> {
        let ticked = self.service.tick(session, Instant::now());
        self.publish();
        ticked
    }

    /// See [`ScreenService::session_ended`].
    pub fn session_ended(&mut self) {
        self.service.session_ended();
        self.publish();
    }

    fn publish(&self) {
        self.board.set_screen(dto::status(self.service.state()));
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use link2linux_core::screen::VideoFormat;

    use super::*;

    fn frame(format: Option<VideoFormat>) -> VideoFrame {
        VideoFrame {
            format,
            format_id: 0x0102_0304,
            timestamp_us: 0x1122_3344_5566_7788,
            key_frame: true,
            data: vec![0xaa, 0xbb],
        }
    }

    #[test]
    fn a_packet_carries_the_header_then_the_frame() {
        assert_eq!(
            packet(&frame(None)),
            [
                1, 1, 1, 2, 3, 4, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0xaa, 0xbb
            ]
        );
    }

    #[test]
    fn the_first_frame_of_a_format_carries_it() {
        let format = VideoFormat {
            id: 0x0102_0304,
            codec: Codec::H264,
            width: 720,
            height: 1560,
            codec_config: vec![1, 0x64, 0, 0x1f],
        };
        let packet = packet(&frame(Some(format)));
        assert_eq!(packet[1], KEY_FRAME | HAS_FORMAT);
        assert_eq!(
            packet[HEADER_LEN..],
            [
                1, 0, 0, 2, 0xd0, 0, 0, 6, 0x18, 0, 4, 1, 0x64, 0, 0x1f, 0xaa, 0xbb
            ]
        );
    }

    struct Window {
        shown: Mutex<Vec<usize>>,
        open: bool,
    }

    impl ScreenViewer for Window {
        fn show(&self, packet: Vec<u8>) -> bool {
            self.shown.lock().unwrap().push(packet.len());
            self.open
        }
    }

    #[tokio::test]
    async fn the_pump_hands_frames_over_until_the_window_is_gone() {
        let window = Arc::new(Window {
            shown: Mutex::default(),
            open: false,
        });
        let (frames, queue) = hand_off();
        frames.try_send(frame(None)).unwrap();
        frames.try_send(frame(None)).unwrap();
        pump(queue, window.clone()).await;
        assert_eq!(*window.shown.lock().unwrap(), [HEADER_LEN + 2]);
        // The queue closed: the service hears the window is gone.
        assert!(frames.is_closed());

        let window = Arc::new(Window {
            shown: Mutex::default(),
            open: true,
        });
        let (frames, queue) = hand_off();
        frames.try_send(frame(None)).unwrap();
        drop(frames);
        pump(queue, window.clone()).await;
        assert_eq!(window.shown.lock().unwrap().len(), 1);
    }
}
