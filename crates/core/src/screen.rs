//! The phone's screen, shown and controlled on the desktop
//! (`docs/protocol.md`, "Screen").
//!
//! The desktop asks with [`ScreenService::start`]. The phone asks its user,
//! and once the user agrees it sends a `ScreenFormat`, then its screen as
//! `ScreenFrame` events, until either side stops. The service hands each
//! frame to the viewer through a bounded queue ([`hand_off`]), so a slow
//! viewer never holds up the session: when the queue is full, the frame is
//! dropped, and so is every frame up to the next key frame, which the
//! desktop asks for. Where the phone allows it, [`ScreenService::input`]
//! sends what the user does in the viewer back to the phone.
//!
//! Nothing here waits for the session to write: requests and events are
//! queued, so the session goes on reading the video meanwhile.

use std::collections::VecDeque;

use link2linux_protocol::Payload;
use link2linux_protocol::v1::{self as wire, ErrorCode, ErrorReply, ScreenEndReason};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::mpsc::{self, error::TrySendError};
use tokio::time::{Duration, Instant};

use crate::Feature;
use crate::session::{Availability, Session, SessionError, SessionEvent};

/// How many codecs a start may list.
pub const MAX_CODECS: usize = 8;
/// The smallest longest side the desktop may ask for, other than 0.
pub const MIN_MAX_SIZE: u32 = 240;
/// The largest longest side the desktop may ask for.
pub const MAX_MAX_SIZE: u32 = 4096;
/// The longest `ScreenFormat.codec_config`.
pub const MAX_CODEC_CONFIG: usize = 4 * 1024;
/// The longest `ScreenFrame.data`.
pub const MAX_FRAME: usize = 448 * 1024;
/// The widest or tallest frame the desktop shows. The phone never sends a
/// picture larger than its screen; this only keeps a broken format from
/// asking the viewer for a huge canvas.
pub const MAX_SIDE: u32 = 8192;
/// How often the desktop may ask for a key frame.
pub const KEY_FRAME_EVERY: Duration = Duration::from_secs(1);
/// The most points in one gesture.
pub const MAX_POINTS: usize = 64;
/// The longest gesture, in milliseconds from its first point.
pub const MAX_GESTURE_MS: u32 = 10_000;
/// The longest text in one `ScreenInput`, in bytes. Longer text goes as
/// several, in order.
pub const MAX_TEXT: usize = 4096;
/// How many frames wait for the viewer at most. At 60 frames per second
/// that is a tenth of a second of video.
pub const FRAMES_WAITING: usize = 6;
/// How many inputs may wait for the phone's answer: the one it carries
/// out and the 32 it queues behind it. The phone answers more with
/// `ERROR_CODE_BUSY`, so the desktop drops them itself.
const MAX_INPUTS_WAITING: usize = 33;

/// How the phone encodes its screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Codec {
    /// H.264: the format carries the avcC record, and each frame is one
    /// access unit with 4-byte lengths before its NAL units.
    H264,
    /// Each frame is a whole JPEG image.
    Jpeg,
}

impl Codec {
    fn to_wire(self) -> wire::ScreenCodec {
        match self {
            Self::H264 => wire::ScreenCodec::H264,
            Self::Jpeg => wire::ScreenCodec::Jpeg,
        }
    }

    fn from_wire(codec: wire::ScreenCodec) -> Option<Self> {
        match codec {
            wire::ScreenCodec::Unspecified => None,
            wire::ScreenCodec::H264 => Some(Self::H264),
            wire::ScreenCodec::Jpeg => Some(Self::Jpeg),
        }
    }
}

/// What the desktop asks for when it starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScreenRequest {
    /// The codecs the viewer decodes, most preferred first. Repeats are
    /// left out, and only the first [`MAX_CODECS`] count.
    pub codecs: Vec<Codec>,
    /// The longest side the viewer wants, in pixels, or 0 to leave it to
    /// the phone. Other values are kept within [`MIN_MAX_SIZE`] and
    /// [`MAX_MAX_SIZE`].
    pub max_size: u32,
}

impl ScreenRequest {
    /// The codecs as the phone gets them: no repeats, at most
    /// [`MAX_CODECS`].
    fn codecs(&self) -> Vec<Codec> {
        let mut codecs: Vec<Codec> = Vec::new();
        for &codec in &self.codecs {
            if !codecs.contains(&codec) && codecs.len() < MAX_CODECS {
                codecs.push(codec);
            }
        }
        codecs
    }

    fn max_size(&self) -> u32 {
        if self.max_size == 0 {
            0
        } else {
            self.max_size.clamp(MIN_MAX_SIZE, MAX_MAX_SIZE)
        }
    }
}

/// How the frames that follow are encoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoFormat {
    /// Names this size and encoding within the session.
    pub id: u32,
    /// The encoding.
    pub codec: Codec,
    /// The frames' width in pixels.
    pub width: u32,
    /// The frames' height in pixels.
    pub height: u32,
    /// For H.264, the avcC record; empty for JPEG.
    pub codec_config: Vec<u8>,
}

/// One frame for the viewer.
#[derive(Clone, PartialEq, Eq)]
pub struct VideoFrame {
    /// The format to decode this frame and the ones after it with, when it
    /// is new to the viewer: always on the first frame a viewer gets, and
    /// on the first after each change. It always comes with a key frame.
    pub format: Option<VideoFormat>,
    /// The id of the format the frame is in.
    pub format_id: u32,
    /// When the phone captured it, in microseconds from a start the phone
    /// picked.
    pub timestamp_us: u64,
    /// Decoding can start at this frame.
    pub key_frame: bool,
    /// The encoded frame.
    pub data: Vec<u8>,
}

impl std::fmt::Debug for VideoFrame {
    // The frame is the user's screen: only its size goes into logs.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VideoFrame")
            .field("format", &self.format.as_ref().map(|format| format.id))
            .field("format_id", &self.format_id)
            .field("timestamp_us", &self.timestamp_us)
            .field("key_frame", &self.key_frame)
            .field("len", &self.data.len())
            .finish()
    }
}

/// The two ends of one stream's queue to the viewer: the service gets the
/// sender with [`ScreenService::start`], the viewer reads the receiver.
/// Dropping the receiver tells the service the viewer is gone, and the
/// service then stops the stream.
#[must_use]
pub fn hand_off() -> (mpsc::Sender<VideoFrame>, mpsc::Receiver<VideoFrame>) {
    mpsc::channel(FRAMES_WAITING)
}

/// Where the stream stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreenPhase {
    /// No stream, as far as the desktop knows.
    Idle,
    /// The desktop asked the phone and waits for its answer.
    Starting,
    /// The phone asks its user whether to share the screen.
    Asking,
    /// The phone shares its screen.
    Showing,
    /// The desktop asked the phone to stop and waits for its answer.
    Stopping,
}

/// Why the screen cannot be shown, or did not start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreenProblem {
    /// No session with the phone is up.
    NotConnected,
    /// The session is up, but the phone has not yet said whether it can
    /// share its screen.
    Waiting,
    /// The phone's app cannot share its screen: it is older than this
    /// desktop's.
    Unsupported,
    /// The user turned screen sharing off on the phone.
    TurnedOff,
    /// The phone cannot share its screen right now.
    Unavailable,
    /// The phone shares its screen with another desktop, or asks its user
    /// to.
    Busy,
    /// The phone cannot ask its user, for example because its app's
    /// notifications are blocked.
    CannotAsk,
    /// The phone can encode none of the codecs the viewer decodes.
    NoCodec,
    /// The phone refused, or gave no answer in time.
    Failed,
}

/// Why a stream ended without the desktop stopping it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreenEnd {
    /// The user did not agree to share the screen.
    Declined,
    /// The user did not answer on the phone in time.
    TimedOut,
    /// The user stopped sharing on the phone.
    StoppedOnPhone,
    /// The phone locked.
    Locked,
    /// The user turned screen sharing off on the phone.
    TurnedOff,
    /// The phone could not go on, or sent what the desktop cannot show.
    Interrupted,
    /// The session with the phone ended.
    Disconnected,
}

impl ScreenEnd {
    fn from_wire(reason: ScreenEndReason) -> Self {
        match reason {
            ScreenEndReason::Declined => Self::Declined,
            ScreenEndReason::TimedOut => Self::TimedOut,
            ScreenEndReason::StoppedOnPhone => Self::StoppedOnPhone,
            ScreenEndReason::Locked => Self::Locked,
            ScreenEndReason::Interrupted | ScreenEndReason::Unspecified => Self::Interrupted,
        }
    }
}

/// Why the phone did not carry out what the user did in the viewer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputProblem {
    /// The phone does not take input from this desktop now: control is off
    /// on the phone.
    ControlOff,
    /// The screen changed, for example it rotated, before the gesture
    /// arrived.
    ScreenChanged,
    /// No text field has the focus on the phone.
    NoTextField,
    /// The focused field does not take the text or key: a password field
    /// the phone cannot edit, or Enter where the field has no action.
    CannotType,
    /// The phone refused, or gave no answer in time.
    Failed,
}

/// What an input acts on, which says what the phone's refusal means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Aim {
    /// The screen: a gesture or a navigation key.
    Screen,
    /// The focused text field: text, Backspace or Enter.
    Field,
}

/// One point of a gesture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Point {
    /// In the frames' pixels.
    pub x: u32,
    /// In the frames' pixels.
    pub y: u32,
    /// Milliseconds after the gesture's first point.
    pub time_ms: u32,
}

/// The phone's keys the viewer offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    /// The system's Back.
    Back,
    /// The system's Home.
    Home,
    /// The list of recent apps.
    Recents,
    /// The notification shade.
    Notifications,
    /// Deletes before the cursor in the focused text field.
    Backspace,
    /// Enter in the focused text field.
    Enter,
}

impl Key {
    fn to_wire(self) -> wire::ScreenKey {
        match self {
            Self::Back => wire::ScreenKey::Back,
            Self::Home => wire::ScreenKey::Home,
            Self::Recents => wire::ScreenKey::Recents,
            Self::Notifications => wire::ScreenKey::Notifications,
            Self::Backspace => wire::ScreenKey::Backspace,
            Self::Enter => wire::ScreenKey::Enter,
        }
    }
}

/// Something the user did in the viewer.
#[derive(Clone, PartialEq, Eq)]
pub enum Input {
    /// One finger following `points`, in the frames of format `format_id`.
    Gesture {
        /// The format of the frames the user saw.
        format_id: u32,
        /// The finger's way, first point first.
        points: Vec<Point>,
    },
    /// A key.
    Key(Key),
    /// Text to type into the focused field.
    Text(String),
}

impl std::fmt::Debug for Input {
    // Typed text is the user's content: only its length goes into logs.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Gesture { format_id, points } => f
                .debug_struct("Gesture")
                .field("format_id", format_id)
                .field("points", &points.len())
                .finish(),
            Self::Key(key) => f.debug_tuple("Key").field(key).finish(),
            Self::Text(text) => f.debug_struct("Text").field("len", &text.len()).finish(),
        }
    }
}

/// The current format as the viewer's page shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShownFormat {
    /// The format's id, which gestures name.
    pub id: u32,
    /// The encoding.
    pub codec: Codec,
    /// The frames' width in pixels.
    pub width: u32,
    /// The frames' height in pixels.
    pub height: u32,
    /// Whether the phone takes input now.
    pub input: bool,
}

/// What to show of the screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScreenState {
    /// Why the screen cannot be shown now, or `None` when it can.
    pub blocked: Option<ScreenProblem>,
    /// Where the stream stands.
    pub phase: ScreenPhase,
    /// The format of the frames being shown.
    pub format: Option<ShownFormat>,
    /// Why the last start did not work. The next start clears it.
    pub failure: Option<ScreenProblem>,
    /// Why the last stream ended, when the desktop did not stop it. The
    /// next start clears it.
    pub ended: Option<ScreenEnd>,
    /// Why the phone did not carry out the user's last input. The next
    /// input the phone carries out clears it.
    pub input_problem: Option<InputProblem>,
}

/// One stream, from the start the user asked for to its end.
#[derive(Debug)]
struct Stream {
    /// The viewer's queue.
    frames: mpsc::Sender<VideoFrame>,
    /// The codecs the viewer decodes.
    codecs: Vec<Codec>,
    /// The phone answered the start: it asks its user or shows already.
    acked: bool,
    /// The format of the frames, once the phone sent one.
    format: Option<VideoFormat>,
    /// Whether the phone takes input now.
    input: bool,
    /// The viewer has not got the format yet.
    format_unsent: bool,
    /// The desktop started the stream again, and its frames wait for the
    /// format that answers that start: the ones before it are in what the
    /// earlier viewer asked for.
    awaiting_format: bool,
    /// Frames are dropped up to the next key frame.
    awaiting_key: bool,
}

/// The screen feature with one phone, driven by its session. It does not
/// own the session, so it can share one with other features:
///
/// 1. [`ScreenService::session_started`] once the session is up;
/// 2. every [`SessionEvent`] through [`ScreenService::handle_event`], which
///    keeps the screen's events and hands back the rest;
/// 3. [`ScreenService::start`], [`ScreenService::stop`],
///    [`ScreenService::input`] and [`ScreenService::request_key_frame`]
///    when the user asks, or [`ScreenService::not_connected`] for a start
///    while no session is up;
/// 4. [`ScreenService::tick`] at [`ScreenService::next_deadline`];
/// 5. [`ScreenService::session_ended`] when the session is gone.
///
/// [`ScreenService::state`] says what to show after each of them.
#[derive(Debug)]
pub struct ScreenService {
    blocked: Option<ScreenProblem>,
    failure: Option<ScreenProblem>,
    ended: Option<ScreenEnd>,
    input_problem: Option<InputProblem>,
    /// The `ScreenStart` that waits for its answer.
    start_asked: Option<u64>,
    /// The `ScreenStop` that waits for its answer. Until it comes, what the
    /// phone sends of its screen is left over from the stopped stream.
    stop_asked: Option<u64>,
    stream: Option<Stream>,
    /// The `ScreenInput`s that wait for their answers, oldest first.
    inputs: VecDeque<(u64, Aim)>,
    /// When the desktop last asked for a key frame.
    key_asked_at: Option<Instant>,
    /// When the desktop may ask for the key frame it still waits for.
    key_due: Option<Instant>,
}

impl Default for ScreenService {
    fn default() -> Self {
        Self::new()
    }
}

impl ScreenService {
    /// A service with no session yet.
    #[must_use]
    pub fn new() -> Self {
        Self {
            blocked: Some(ScreenProblem::NotConnected),
            failure: None,
            ended: None,
            input_problem: None,
            start_asked: None,
            stop_asked: None,
            stream: None,
            inputs: VecDeque::new(),
            key_asked_at: None,
            key_due: None,
        }
    }

    /// What to show now.
    #[must_use]
    pub fn state(&self) -> ScreenState {
        let phase = match &self.stream {
            Some(stream) if stream.format.is_some() => ScreenPhase::Showing,
            Some(stream) if stream.acked => ScreenPhase::Asking,
            Some(_) => ScreenPhase::Starting,
            None if self.stop_asked.is_some() => ScreenPhase::Stopping,
            None => ScreenPhase::Idle,
        };
        let format = self.stream.as_ref().and_then(|stream| {
            stream.format.as_ref().map(|format| ShownFormat {
                id: format.id,
                codec: format.codec,
                width: format.width,
                height: format.height,
                input: stream.input,
            })
        });
        ScreenState {
            blocked: self.blocked,
            phase,
            format,
            failure: self.failure,
            ended: self.ended,
            input_problem: self.input_problem,
        }
    }

    /// Notes whether the phone can share its screen. Call it once the
    /// session is up.
    pub fn session_started<S>(&mut self, session: &Session<S>)
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        self.blocked = blocked(session);
        self.failure = None;
    }

    /// Forgets the stream and what waits for an answer: a stream ends with
    /// its session.
    pub fn session_ended(&mut self) {
        self.blocked = Some(ScreenProblem::NotConnected);
        if self.stream.take().is_some() {
            self.ended = Some(ScreenEnd::Disconnected);
        }
        self.start_asked = None;
        self.stop_asked = None;
        self.inputs.clear();
        self.key_asked_at = None;
        self.key_due = None;
    }

    /// Asks the phone to share its screen, with frames for the viewer going
    /// to `frames` (see [`hand_off`]). While a stream runs, the new viewer
    /// takes it over, and the phone starts it again with a key frame
    /// without asking its user. A phone that cannot share its screen is
    /// not asked, and the state says why.
    ///
    /// # Errors
    ///
    /// Fails when the session fails.
    pub fn start<S>(
        &mut self,
        session: &mut Session<S>,
        request: &ScreenRequest,
        frames: mpsc::Sender<VideoFrame>,
    ) -> Result<(), SessionError>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        self.failure = None;
        self.ended = None;
        self.input_problem = None;
        self.blocked = blocked(session);
        if let Some(reason) = self.blocked {
            self.failure = Some(reason);
            return Ok(());
        }
        let codecs = request.codecs();
        if codecs.is_empty() {
            self.failure = Some(ScreenProblem::NoCodec);
            return Ok(());
        }
        let seq = session.queue_request(Payload::ScreenStart(wire::ScreenStart {
            codecs: codecs.iter().map(|codec| codec.to_wire().into()).collect(),
            max_size: request.max_size(),
        }))?;
        session.write_queued()?;
        tracing::debug!(seq, codecs = codecs.len(), "asked the phone for its screen");
        self.start_asked = Some(seq);
        match &mut self.stream {
            // The phone sends a new format and a key frame, which the new
            // viewer starts from.
            Some(stream) => {
                stream.frames = frames;
                stream.codecs = codecs;
                stream.format_unsent = true;
                stream.awaiting_key = true;
                stream.awaiting_format = true;
            }
            None => {
                self.stream = Some(Stream {
                    frames,
                    codecs,
                    acked: false,
                    format: None,
                    input: false,
                    format_unsent: true,
                    awaiting_key: true,
                    awaiting_format: false,
                });
            }
        }
        Ok(())
    }

    /// The user asked for the screen while no session is up.
    pub fn not_connected(&mut self) {
        self.blocked = Some(ScreenProblem::NotConnected);
        self.failure = Some(ScreenProblem::NotConnected);
        self.ended = None;
    }

    /// Stops the stream, or the start that waits: the viewer's queue
    /// closes, and the phone hears of it. Nothing happens without either.
    ///
    /// # Errors
    ///
    /// Fails when the session fails.
    pub fn stop<S>(&mut self, session: &mut Session<S>) -> Result<(), SessionError>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        self.failure = None;
        if self.stream.is_none() && self.start_asked.is_none() {
            return Ok(());
        }
        self.drop_stream();
        self.ask_to_stop(session)
    }

    /// Tells the phone what the user did in the viewer. Text longer than
    /// [`MAX_TEXT`] goes in several parts, and a gesture is fitted to the
    /// protocol's limits. Input the phone does not take now is not sent,
    /// and the state says why.
    ///
    /// # Errors
    ///
    /// Fails when the session fails.
    pub fn input<S>(&mut self, session: &mut Session<S>, input: Input) -> Result<(), SessionError>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        let Some((format, true)) = self
            .stream
            .as_ref()
            .and_then(|stream| stream.format.as_ref().map(|format| (format, stream.input)))
        else {
            self.input_problem = Some(InputProblem::ControlOff);
            return Ok(());
        };
        let aim = match input {
            Input::Text(_) | Input::Key(Key::Backspace | Key::Enter) => Aim::Field,
            Input::Gesture { .. } | Input::Key(_) => Aim::Screen,
        };
        let inputs = match input {
            Input::Gesture { format_id, .. } if format_id != format.id => {
                self.input_problem = Some(InputProblem::ScreenChanged);
                return Ok(());
            }
            Input::Gesture { format_id, points } => {
                let Some(points) = fit_gesture(&points, format.width, format.height) else {
                    return Ok(());
                };
                vec![wire::screen_input::Input::Gesture(wire::ScreenGesture {
                    format_id,
                    points,
                })]
            }
            Input::Key(key) => vec![wire::screen_input::Input::Key(key.to_wire().into())],
            Input::Text(text) => split_text(&text)
                .into_iter()
                .map(|part| wire::screen_input::Input::Text(part.to_owned()))
                .collect(),
        };
        if self.inputs.len() + inputs.len() > MAX_INPUTS_WAITING {
            // As the phone would answer BUSY: the user sees the phone not
            // keep up, and needs no word on it.
            tracing::debug!(
                waiting = self.inputs.len(),
                "the phone is behind: input dropped"
            );
            return Ok(());
        }
        for input in inputs {
            let seq = session.queue_request(Payload::ScreenInput(wire::ScreenInput {
                input: Some(input),
            }))?;
            self.inputs.push_back((seq, aim));
        }
        session.write_queued()
    }

    /// The viewer could not decode a frame, or fell behind: frames are
    /// dropped up to the next key frame, which the phone is asked for.
    ///
    /// # Errors
    ///
    /// Fails when the session fails.
    pub fn request_key_frame<S>(&mut self, session: &mut Session<S>) -> Result<(), SessionError>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        let Some(stream) = self
            .stream
            .as_mut()
            .filter(|stream| stream.format.is_some())
        else {
            return Ok(());
        };
        stream.awaiting_key = true;
        self.want_key_frame(session, Instant::now())
    }

    /// Takes the events of the screen feature and returns every other
    /// event, for the caller to handle. A `FeatureStatus` is used and
    /// returned too.
    ///
    /// # Errors
    ///
    /// Fails when the session fails.
    pub fn handle_event<S>(
        &mut self,
        session: &mut Session<S>,
        event: SessionEvent,
    ) -> Result<Option<SessionEvent>, SessionError>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        match event {
            SessionEvent::Reply(reply) if self.start_asked == Some(reply.request) => {
                self.start_asked = None;
                match reply.into_result() {
                    Ok(_) => {
                        if let Some(stream) = &mut self.stream {
                            stream.acked = true;
                        }
                    }
                    Err(error) => {
                        tracing::info!(code = error.code, "the phone refused to share its screen");
                        self.failure = Some(start_problem(&error));
                        // A phone that shows already may go on unless told.
                        let showing = self
                            .stream
                            .as_ref()
                            .is_some_and(|stream| stream.format.is_some());
                        self.drop_stream();
                        if showing {
                            self.ask_to_stop(session)?;
                        }
                    }
                }
            }
            SessionEvent::RequestTimedOut { request } if self.start_asked == Some(request) => {
                self.start_asked = None;
                tracing::info!("the phone did not answer the screen start in time");
                self.failure = Some(ScreenProblem::Failed);
                // It may still ask its user.
                self.give_up(session, None)?;
            }
            SessionEvent::Reply(reply) if self.stop_asked == Some(reply.request) => {
                self.stop_asked = None;
                if let Err(error) = reply.into_result() {
                    tracing::debug!(code = error.code, "the phone refused to stop its screen");
                }
            }
            SessionEvent::RequestTimedOut { request } if self.stop_asked == Some(request) => {
                self.stop_asked = None;
                tracing::debug!("the phone did not answer the screen stop in time");
            }
            SessionEvent::Reply(reply) if self.waiting_input(reply.request).is_some() => {
                let aim = self.take_input(reply.request);
                match reply.into_result() {
                    Ok(_) => self.input_problem = None,
                    Err(error) => {
                        tracing::debug!(code = error.code, "the phone refused an input");
                        // A busy phone drops the input, which needs no word.
                        if let Some(problem) = input_problem(&error, aim) {
                            self.input_problem = Some(problem);
                        }
                    }
                }
            }
            SessionEvent::RequestTimedOut { request } if self.waiting_input(request).is_some() => {
                self.take_input(request);
                self.input_problem = Some(InputProblem::Failed);
            }
            SessionEvent::Message {
                payload: Payload::ScreenFormat(format),
                ..
            } => self.format(session, &format)?,
            SessionEvent::Message {
                payload: Payload::ScreenFrame(frame),
                ..
            } => self.frame(session, frame)?,
            SessionEvent::Message {
                payload: Payload::ScreenEnded(ended),
                ..
            } => {
                // After a stop, the end needs no word: it was sent before
                // the phone heard of the stop.
                if self.stop_asked.is_none() && self.stream.is_some() {
                    tracing::debug!(
                        reason = ended.reason,
                        "the phone stopped sharing its screen"
                    );
                    self.drop_stream();
                    self.ended = Some(ScreenEnd::from_wire(ended.reason()));
                }
            }
            // Only the desktop asks for key frames.
            SessionEvent::Message {
                payload: Payload::ScreenKeyFrameRequest(_),
                ..
            } => {}
            SessionEvent::FeatureStatus(states) => {
                self.blocked = blocked(session);
                if self.blocked.is_some() && self.stream.is_some() {
                    // The phone ends its stream when the feature leaves
                    // READY, and may not get to say so.
                    self.drop_stream();
                    self.ended = Some(match self.blocked {
                        Some(ScreenProblem::TurnedOff) => ScreenEnd::TurnedOff,
                        _ => ScreenEnd::Interrupted,
                    });
                }
                return Ok(Some(SessionEvent::FeatureStatus(states)));
            }
            other => return Ok(Some(other)),
        }
        Ok(None)
    }

    /// What the input `seq` acts on, while it waits for its answer.
    fn waiting_input(&self, seq: u64) -> Option<Aim> {
        self.inputs
            .iter()
            .find(|&&(waiting, _)| waiting == seq)
            .map(|&(_, aim)| aim)
    }

    /// Forgets the input `seq`, which got its answer, and says what it
    /// acted on.
    fn take_input(&mut self, seq: u64) -> Aim {
        let aim = self.waiting_input(seq).unwrap_or(Aim::Screen);
        self.inputs.retain(|&(waiting, _)| waiting != seq);
        aim
    }

    /// When the desktop may ask for the key frame it waits for.
    #[must_use]
    pub fn next_deadline(&self) -> Option<Instant> {
        self.key_due
    }

    /// The time [`ScreenService::next_deadline`] named may have come: asks
    /// for the key frame the viewer still waits for.
    ///
    /// # Errors
    ///
    /// Fails when the session fails.
    pub fn tick<S>(&mut self, session: &mut Session<S>, now: Instant) -> Result<(), SessionError>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        if self.key_due.is_none_or(|due| due > now) {
            return Ok(());
        }
        self.key_due = None;
        if self
            .stream
            .as_ref()
            .is_some_and(|stream| stream.format.is_some() && stream.awaiting_key)
        {
            self.want_key_frame(session, now)?;
        }
        Ok(())
    }

    fn format<S>(
        &mut self,
        session: &mut Session<S>,
        format: &wire::ScreenFormat,
    ) -> Result<(), SessionError>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        if self.stop_asked.is_some() {
            return Ok(());
        }
        let Some(stream) = &mut self.stream else {
            // Video nobody watches, perhaps of a start the desktop gave up
            // on: the phone stops it.
            tracing::debug!("the phone shares its screen unasked");
            return self.ask_to_stop(session);
        };
        let Some(next) = video_format(format, &stream.codecs) else {
            tracing::info!(
                codec = format.codec,
                width = format.width,
                height = format.height,
                config = format.codec_config.len(),
                "the phone's screen format cannot be shown"
            );
            return self.give_up(session, Some(ScreenEnd::Interrupted));
        };
        stream.acked = true;
        stream.input = format.input;
        stream.awaiting_format = false;
        if stream.format.as_ref() != Some(&next) {
            tracing::debug!(
                id = next.id,
                width = next.width,
                height = next.height,
                "the phone's screen format changed"
            );
            stream.format = Some(next);
            stream.format_unsent = true;
            // The phone sends a key frame next.
            stream.awaiting_key = true;
        }
        Ok(())
    }

    fn frame<S>(
        &mut self,
        session: &mut Session<S>,
        frame: wire::ScreenFrame,
    ) -> Result<(), SessionError>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        if self.stop_asked.is_some() {
            return Ok(());
        }
        let Some(stream) = &mut self.stream else {
            return Ok(());
        };
        let Some(format) = stream.format.as_ref().filter(|_| !stream.awaiting_format) else {
            return Ok(());
        };
        if frame.data.len() > MAX_FRAME {
            tracing::debug!(len = frame.data.len(), "a screen frame was too large");
            stream.awaiting_key = true;
            return self.want_key_frame(session, Instant::now());
        }
        if stream.awaiting_key && !frame.key_frame {
            return self.want_key_frame(session, Instant::now());
        }
        let video = VideoFrame {
            format: stream.format_unsent.then(|| format.clone()),
            format_id: format.id,
            timestamp_us: frame.timestamp_us,
            key_frame: frame.key_frame,
            data: frame.data,
        };
        match stream.frames.try_send(video) {
            Ok(()) => {
                stream.format_unsent = false;
                if frame.key_frame {
                    stream.awaiting_key = false;
                    self.key_due = None;
                }
                Ok(())
            }
            Err(TrySendError::Full(_)) => {
                stream.awaiting_key = true;
                self.want_key_frame(session, Instant::now())
            }
            Err(TrySendError::Closed(_)) => {
                tracing::debug!("the screen's viewer is gone");
                self.drop_stream();
                self.ask_to_stop(session)
            }
        }
    }

    /// Asks for a key frame now, or once a second has passed since the
    /// last time.
    fn want_key_frame<S>(
        &mut self,
        session: &mut Session<S>,
        now: Instant,
    ) -> Result<(), SessionError>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        if let Some(next) = self.key_asked_at.map(|at| at + KEY_FRAME_EVERY)
            && now < next
        {
            self.key_due = Some(next);
            return Ok(());
        }
        session.queue_send(Payload::ScreenKeyFrameRequest(
            wire::ScreenKeyFrameRequest {},
        ))?;
        session.write_queued()?;
        self.key_asked_at = Some(now);
        self.key_due = None;
        Ok(())
    }

    /// Ends the stream on the desktop, and stops the phone's.
    fn give_up<S>(
        &mut self,
        session: &mut Session<S>,
        ended: Option<ScreenEnd>,
    ) -> Result<(), SessionError>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        self.drop_stream();
        if ended.is_some() {
            self.ended = ended;
        }
        self.ask_to_stop(session)
    }

    /// Closes the viewer's queue and forgets the stream.
    fn drop_stream(&mut self) {
        self.stream = None;
        self.start_asked = None;
        self.key_due = None;
    }

    /// Sends `ScreenStop`, unless one waits for its answer already or the
    /// feature is not ready, when the phone stops by itself.
    fn ask_to_stop<S>(&mut self, session: &mut Session<S>) -> Result<(), SessionError>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        if self.stop_asked.is_some() || !session.is_ready(Feature::Screen) {
            return Ok(());
        }
        let seq = session.queue_request(Payload::ScreenStop(wire::ScreenStop {}))?;
        session.write_queued()?;
        tracing::debug!(seq, "asked the phone to stop sharing its screen");
        self.stop_asked = Some(seq);
        Ok(())
    }
}

/// The format the phone sent, if the viewer can use it.
fn video_format(format: &wire::ScreenFormat, codecs: &[Codec]) -> Option<VideoFormat> {
    let codec = Codec::from_wire(format.codec()).filter(|codec| codecs.contains(codec))?;
    let config_fits = match codec {
        Codec::H264 => (1..=MAX_CODEC_CONFIG).contains(&format.codec_config.len()),
        Codec::Jpeg => format.codec_config.is_empty(),
    };
    let size_fits =
        (1..=MAX_SIDE).contains(&format.width) && (1..=MAX_SIDE).contains(&format.height);
    (config_fits && size_fits).then(|| VideoFormat {
        id: format.id,
        codec,
        width: format.width,
        height: format.height,
        codec_config: format.codec_config.clone(),
    })
}

/// `points` within the protocol's limits: at most [`MAX_POINTS`], kept
/// inside a `width` by `height` frame, with times that start at 0, never go
/// back and end by [`MAX_GESTURE_MS`]. A long way keeps its first and last
/// points and evenly spread ones between. `None` without points.
fn fit_gesture(points: &[Point], width: u32, height: u32) -> Option<Vec<wire::ScreenPoint>> {
    let first = points.first()?;
    let kept: Vec<&Point> = if points.len() <= MAX_POINTS {
        points.iter().collect()
    } else {
        let last = points.len() - 1;
        (0..MAX_POINTS)
            .map(|i| &points[i * last / (MAX_POINTS - 1)])
            .collect()
    };
    let mut time = 0;
    Some(
        kept.into_iter()
            .map(|point| {
                time = point
                    .time_ms
                    .saturating_sub(first.time_ms)
                    .clamp(time, MAX_GESTURE_MS);
                wire::ScreenPoint {
                    x: point.x.min(width.saturating_sub(1)),
                    y: point.y.min(height.saturating_sub(1)),
                    time_ms: time,
                }
            })
            .collect(),
    )
}

/// `text` in parts of at most [`MAX_TEXT`] bytes, split between
/// characters. Empty text has no parts.
fn split_text(text: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        let mut end = rest.len().min(MAX_TEXT);
        while !rest.is_char_boundary(end) {
            end -= 1;
        }
        let (part, tail) = rest.split_at(end);
        parts.push(part);
        rest = tail;
    }
    parts
}

fn start_problem(error: &ErrorReply) -> ScreenProblem {
    match error.code() {
        ErrorCode::Disabled => ScreenProblem::TurnedOff,
        ErrorCode::Busy => ScreenProblem::Busy,
        ErrorCode::NotAllowed => ScreenProblem::CannotAsk,
        ErrorCode::InvalidArgument => ScreenProblem::NoCodec,
        ErrorCode::Unavailable | ErrorCode::PermissionRequired => ScreenProblem::Unavailable,
        ErrorCode::Unsupported => ScreenProblem::Unsupported,
        _ => ScreenProblem::Failed,
    }
}

/// What the phone's refusal of an input that acted on `aim` means for the
/// user, or `None` for a refusal that needs no word.
fn input_problem(error: &ErrorReply, aim: Aim) -> Option<InputProblem> {
    Some(match (error.code(), aim) {
        // The phone queues only so many inputs, and drops the rest.
        (ErrorCode::Busy, _) => return None,
        // A field the phone cannot type into, or Enter where it has no
        // action, while control is still on.
        (ErrorCode::NotAllowed | ErrorCode::Unsupported, Aim::Field) => InputProblem::CannotType,
        (ErrorCode::NotAllowed | ErrorCode::Disabled, _) => InputProblem::ControlOff,
        (ErrorCode::Expired, _) => InputProblem::ScreenChanged,
        (ErrorCode::NotFound, _) => InputProblem::NoTextField,
        _ => InputProblem::Failed,
    })
}

/// Why the phone cannot share its screen in `session`, or `None` when it
/// can.
fn blocked<S>(session: &Session<S>) -> Option<ScreenProblem>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    if session.is_ready(Feature::Screen) {
        return None;
    }
    if !session.shared_features().contains(&Feature::Screen) {
        return Some(ScreenProblem::Unsupported);
    }
    let Some(states) = session.peer_availability() else {
        return Some(ScreenProblem::Waiting);
    };
    Some(match states.get(&Feature::Screen) {
        Some(Availability::Disabled) => ScreenProblem::TurnedOff,
        _ => ScreenProblem::Unavailable,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn point(x: u32, y: u32, time_ms: u32) -> Point {
        Point { x, y, time_ms }
    }

    #[test]
    fn a_request_lists_each_codec_once_and_keeps_the_size_in_range() {
        let request = ScreenRequest {
            codecs: vec![Codec::H264, Codec::Jpeg, Codec::H264],
            max_size: 100,
        };
        assert_eq!(request.codecs(), [Codec::H264, Codec::Jpeg]);
        assert_eq!(request.max_size(), MIN_MAX_SIZE);
        let request = ScreenRequest {
            codecs: vec![],
            max_size: 10_000,
        };
        assert_eq!(request.max_size(), MAX_MAX_SIZE);
        assert_eq!(
            ScreenRequest {
                max_size: 0,
                ..request
            }
            .max_size(),
            0
        );
    }

    #[test]
    fn a_gesture_is_fitted_to_the_frame_and_the_limits() {
        let points = [
            point(5, 5, 100),
            point(2000, 50, 90),
            point(10, 4000, 20_000),
        ];
        let fitted = fit_gesture(&points, 1080, 2340).unwrap();
        let got: Vec<_> = fitted.iter().map(|p| (p.x, p.y, p.time_ms)).collect();
        // Times start at 0, never go back, and end by 10 s; points stay
        // inside the frame.
        assert_eq!(got, [(5, 5, 0), (1079, 50, 0), (10, 2339, MAX_GESTURE_MS)]);
        assert_eq!(fit_gesture(&[], 10, 10), None);
    }

    #[test]
    fn a_long_gesture_keeps_its_ends_and_at_most_64_points() {
        let points: Vec<Point> = (0..500).map(|i| point(i, i, i)).collect();
        let fitted = fit_gesture(&points, 1000, 1000).unwrap();
        assert_eq!(fitted.len(), MAX_POINTS);
        assert_eq!((fitted[0].x, fitted[0].time_ms), (0, 0));
        assert_eq!(fitted[MAX_POINTS - 1].x, 499);
        assert!(
            fitted
                .windows(2)
                .all(|pair| pair[0].time_ms <= pair[1].time_ms)
        );
    }

    #[test]
    fn long_text_is_split_between_characters() {
        // Two bytes per character.
        let text = "\u{e9}".repeat(MAX_TEXT);
        let parts = split_text(&text);
        assert_eq!(parts.len(), 2);
        assert!(parts.iter().all(|part| part.len() <= MAX_TEXT));
        assert_eq!(parts.concat(), text);
        let odd = format!("a{}", "\u{e9}".repeat(MAX_TEXT / 2));
        assert_eq!(split_text(&odd)[0].len(), MAX_TEXT - 1);
        assert!(split_text("").is_empty());
    }

    #[test]
    fn frames_and_inputs_keep_their_content_out_of_logs() {
        let frame = VideoFrame {
            format: None,
            format_id: 3,
            timestamp_us: 1,
            key_frame: true,
            data: vec![42; 3],
        };
        assert!(!format!("{frame:?}").contains("42"));
        let text = Input::Text("my password".into());
        assert!(!format!("{text:?}").contains("password"));
    }

    #[test]
    fn a_format_must_be_one_the_viewer_asked_for_and_fit() {
        let h264 = wire::ScreenFormat {
            id: 1,
            codec: wire::ScreenCodec::H264.into(),
            width: 1080,
            height: 2340,
            codec_config: vec![1; 40],
            input: false,
        };
        assert!(video_format(&h264, &[Codec::H264]).is_some());
        assert!(video_format(&h264, &[Codec::Jpeg]).is_none());
        let no_config = wire::ScreenFormat {
            codec_config: vec![],
            ..h264.clone()
        };
        assert!(video_format(&no_config, &[Codec::H264]).is_none());
        let huge = wire::ScreenFormat {
            width: MAX_SIDE + 1,
            ..h264.clone()
        };
        assert!(video_format(&huge, &[Codec::H264]).is_none());
        let jpeg_with_config = wire::ScreenFormat {
            codec: wire::ScreenCodec::Jpeg.into(),
            ..h264
        };
        assert!(video_format(&jpeg_with_config, &[Codec::Jpeg]).is_none());
    }
}
