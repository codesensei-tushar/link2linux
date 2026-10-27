//! The pretend phones' screens: a connected phone asks its pretend user,
//! who agrees after a moment, and then plays a short loop of JPEG frames
//! until the window stops it. Control is on, and every input is taken.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use link2linux_runtime::screen::{
    Codec, ScreenFormat, ScreenInputProblem, ScreenPhase, ScreenProblem, VideoFormat, VideoFrame,
    packet,
};

use crate::backend::{
    BackendError, Device, DeviceStatus, EventSink, ScreenCodec, ScreenInput, ScreenStatus,
    ScreenViewer,
};

/// The loop the pretend phones play: a home screen with a dot moving
/// across it.
const FRAMES: [&[u8]; 6] = [
    include_bytes!("screen/frame-0.jpg"),
    include_bytes!("screen/frame-1.jpg"),
    include_bytes!("screen/frame-2.jpg"),
    include_bytes!("screen/frame-3.jpg"),
    include_bytes!("screen/frame-4.jpg"),
    include_bytes!("screen/frame-5.jpg"),
];
const WIDTH: u32 = 360;
const HEIGHT: u32 = 780;
/// The one format the pretend phones send.
const FORMAT_ID: u32 = 1;

/// How long the pretend user takes to agree, and how often a frame comes.
#[derive(Debug, Clone, Copy)]
pub struct ScreenDelays {
    pub answer: Duration,
    pub frame: Duration,
}

impl ScreenDelays {
    /// What a person sees: a short wait, then ten frames a second.
    pub fn real() -> Self {
        Self {
            answer: Duration::from_millis(1500),
            frame: Duration::from_millis(100),
        }
    }
}

/// Each pretend phone's screen.
pub struct FakeScreen {
    sink: Arc<dyn EventSink>,
    delays: ScreenDelays,
    /// Each phone's screen, with a count of starts and stops so that a
    /// stream that was stopped ends.
    screens: Arc<Mutex<HashMap<String, (ScreenStatus, u64)>>>,
}

impl FakeScreen {
    pub fn new(sink: Arc<dyn EventSink>, delays: ScreenDelays) -> Self {
        Self {
            sink,
            delays,
            screens: Arc::default(),
        }
    }

    /// The screen of `device`, which is `None` when it is not paired.
    pub fn status(&self, device: Option<&Device>) -> Result<ScreenStatus, BackendError> {
        let device = device.ok_or(BackendError::NotFound)?;
        Ok(current(&lock(&self.screens), device))
    }

    /// Asks `device` for its screen: its pretend user agrees, and the
    /// frames go to `viewer` until the stream stops.
    pub fn start(
        &self,
        device: Option<&Device>,
        codecs: &[ScreenCodec],
        viewer: Arc<dyn ScreenViewer>,
    ) -> Result<(), BackendError> {
        let device = device.ok_or(BackendError::NotFound)?;
        let generation = {
            let mut screens = lock(&self.screens);
            let mut screen = current(&screens, device);
            let generation = screens.get(&device.id).map_or(0, |(_, count)| count + 1);
            screen.ended = None;
            screen.input_problem = None;
            screen.problem = screen.blocked.or_else(|| {
                (!codecs.contains(&ScreenCodec::Jpeg)).then_some(ScreenProblem::NoCodec)
            });
            if screen.problem.is_none() {
                screen.phase = ScreenPhase::Asking;
                screen.format = None;
            }
            screens.insert(device.id.clone(), (screen.clone(), generation));
            self.sink.screen_changed(&device.id, &screen);
            if screen.problem.is_some() {
                return Ok(());
            }
            generation
        };
        let stream = Stream {
            screens: self.screens.clone(),
            sink: self.sink.clone(),
            id: device.id.clone(),
            generation,
            delays: self.delays,
        };
        let _ = std::thread::Builder::new()
            .name("fake-screen".to_owned())
            .spawn(move || stream.play(&*viewer));
        Ok(())
    }

    /// Stops showing the screen of `device`.
    pub fn stop(&self, device: Option<&Device>) -> Result<(), BackendError> {
        let device = device.ok_or(BackendError::NotFound)?;
        let mut screens = lock(&self.screens);
        if let Some((screen, count)) = screens.get_mut(&device.id) {
            *count += 1;
            if screen.phase != ScreenPhase::Idle {
                screen.phase = ScreenPhase::Idle;
                screen.format = None;
                screen.problem = None;
                self.sink.screen_changed(&device.id, screen);
            }
        }
        Ok(())
    }

    /// Takes an input while the screen shows, as a phone with control on
    /// does.
    pub fn input(&self, device: Option<&Device>, input: &ScreenInput) -> Result<(), BackendError> {
        let device = device.ok_or(BackendError::NotFound)?;
        let mut screens = lock(&self.screens);
        let Some((screen, _)) = screens.get_mut(&device.id) else {
            return Ok(());
        };
        let problem = match (screen.format, input) {
            (None, _) => Some(ScreenInputProblem::ControlOff),
            (Some(format), ScreenInput::Gesture { format_id, .. }) if *format_id != format.id => {
                Some(ScreenInputProblem::ScreenChanged)
            }
            _ => None,
        };
        if screen.input_problem != problem {
            screen.input_problem = problem;
            self.sink.screen_changed(&device.id, screen);
        }
        Ok(())
    }
}

/// One pretend stream, played on its own thread.
struct Stream {
    screens: Arc<Mutex<HashMap<String, (ScreenStatus, u64)>>>,
    sink: Arc<dyn EventSink>,
    id: String,
    generation: u64,
    delays: ScreenDelays,
}

impl Stream {
    fn play(&self, viewer: &dyn ScreenViewer) {
        std::thread::sleep(self.delays.answer);
        if !self.update(|screen| {
            screen.phase = ScreenPhase::Showing;
            screen.format = Some(ScreenFormat {
                id: FORMAT_ID,
                codec: ScreenCodec::Jpeg,
                width: WIDTH,
                height: HEIGHT,
                input: true,
            });
        }) {
            return;
        }
        let frame_us = u64::try_from(self.delays.frame.as_micros()).unwrap_or(u64::MAX);
        for (index, data) in (0_u64..).zip(FRAMES.iter().cycle()) {
            if !self.update(|_| {}) {
                return;
            }
            let frame = VideoFrame {
                format: (index == 0).then(|| VideoFormat {
                    id: FORMAT_ID,
                    codec: Codec::Jpeg,
                    width: WIDTH,
                    height: HEIGHT,
                    codec_config: Vec::new(),
                }),
                format_id: FORMAT_ID,
                timestamp_us: index.saturating_mul(frame_us),
                key_frame: true,
                data: data.to_vec(),
            };
            if !viewer.show(packet(&frame)) {
                // The window is gone: the stream stops.
                self.update(|screen| {
                    screen.phase = ScreenPhase::Idle;
                    screen.format = None;
                });
                return;
            }
            std::thread::sleep(self.delays.frame);
        }
    }

    /// Changes the screen with `change` while this stream is still the
    /// phone's, and reports it if it changed. Returns whether the stream
    /// goes on.
    fn update(&self, change: impl FnOnce(&mut ScreenStatus)) -> bool {
        let mut screens = lock(&self.screens);
        let Some((screen, count)) = screens.get_mut(&self.id) else {
            return false;
        };
        if *count != self.generation || screen.blocked.is_some() {
            return false;
        }
        let before = screen.clone();
        change(screen);
        if *screen != before {
            self.sink.screen_changed(&self.id, screen);
        }
        true
    }
}

/// The screen of `device` as last reported, with what its connection
/// allows now.
fn current(screens: &HashMap<String, (ScreenStatus, u64)>, device: &Device) -> ScreenStatus {
    let connected = device.status == DeviceStatus::Connected;
    let mut screen = screens
        .get(&device.id)
        .map_or_else(ScreenStatus::default, |(screen, _)| screen.clone());
    screen.blocked = (!connected).then_some(ScreenProblem::NotConnected);
    if !connected {
        screen.phase = ScreenPhase::Idle;
        screen.format = None;
    }
    screen
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use super::*;
    use crate::backend::PairingState;

    #[derive(Default)]
    struct Screens(Mutex<Vec<ScreenStatus>>);

    impl EventSink for Screens {
        fn pairing_changed(&self, _state: &PairingState) {}
        fn devices_changed(&self, _devices: &[Device]) {}
        fn screen_changed(&self, _device_id: &str, screen: &ScreenStatus) {
            self.0.lock().unwrap().push(screen.clone());
        }
    }

    /// A window that keeps the packets it gets, and closes after `open_for`
    /// of them.
    struct Window {
        packets: Mutex<mpsc::Sender<Vec<u8>>>,
        open_for: usize,
        shown: Mutex<usize>,
    }

    impl ScreenViewer for Window {
        fn show(&self, packet: Vec<u8>) -> bool {
            let mut shown = self.shown.lock().unwrap();
            *shown += 1;
            let _ = self.packets.lock().unwrap().send(packet);
            *shown < self.open_for
        }
    }

    fn device(status: DeviceStatus) -> Device {
        Device {
            id: "phone".into(),
            name: "Pixel".into(),
            model: None,
            status,
            paired_at_unix_ms: 0,
            last_connected_unix_ms: None,
        }
    }

    fn fast() -> ScreenDelays {
        ScreenDelays {
            answer: Duration::from_millis(5),
            frame: Duration::from_millis(1),
        }
    }

    #[test]
    fn a_connected_phone_plays_jpeg_frames_until_the_window_closes() {
        let sink = Arc::new(Screens::default());
        let screen = FakeScreen::new(sink.clone(), fast());
        let phone = device(DeviceStatus::Connected);
        let (packets, received) = mpsc::channel();
        let window = Arc::new(Window {
            packets: Mutex::new(packets),
            open_for: 8,
            shown: Mutex::default(),
        });
        screen
            .start(Some(&phone), &[ScreenCodec::Jpeg], window)
            .unwrap();
        let first = received.recv_timeout(Duration::from_secs(10)).unwrap();
        // A key frame with the format, then the JPEG's start-of-image mark.
        assert_eq!(&first[..2], [1, 3]);
        let jpeg = &first[14 + 11..];
        assert_eq!(&jpeg[..2], [0xff, 0xd8]);
        let second = received.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!(&second[..2], [1, 1]);
        // The window closed after its eighth frame: the stream stopped.
        assert_eq!(received.iter().count(), 6);
        let phases: Vec<_> = sink.0.lock().unwrap().iter().map(|s| s.phase).collect();
        assert_eq!(
            phases,
            [ScreenPhase::Asking, ScreenPhase::Showing, ScreenPhase::Idle]
        );
    }

    #[test]
    fn a_phone_away_or_without_jpeg_does_not_start() {
        let sink = Arc::new(Screens::default());
        let screen = FakeScreen::new(sink.clone(), fast());
        let (packets, _received) = mpsc::channel();
        let window = Arc::new(Window {
            packets: Mutex::new(packets),
            open_for: 1,
            shown: Mutex::default(),
        });
        let away = device(DeviceStatus::Disconnected);
        screen
            .start(Some(&away), &[ScreenCodec::Jpeg], window.clone())
            .unwrap();
        assert_eq!(
            screen.status(Some(&away)).unwrap().problem,
            Some(ScreenProblem::NotConnected)
        );
        let phone = device(DeviceStatus::Connected);
        screen
            .start(Some(&phone), &[ScreenCodec::H264], window)
            .unwrap();
        let status = screen.status(Some(&phone)).unwrap();
        assert_eq!(
            (status.phase, status.problem),
            (ScreenPhase::Idle, Some(ScreenProblem::NoCodec))
        );
        assert_eq!(screen.status(None), Err(BackendError::NotFound));
    }

    #[test]
    fn a_stop_ends_the_stream_and_input_needs_one() {
        let sink = Arc::new(Screens::default());
        let screen = FakeScreen::new(
            sink.clone(),
            ScreenDelays {
                answer: Duration::from_secs(600),
                frame: Duration::from_millis(1),
            },
        );
        let phone = device(DeviceStatus::Connected);
        let (packets, _received) = mpsc::channel();
        let window = Arc::new(Window {
            packets: Mutex::new(packets),
            open_for: usize::MAX,
            shown: Mutex::default(),
        });
        screen
            .start(Some(&phone), &[ScreenCodec::Jpeg], window)
            .unwrap();
        screen
            .input(
                Some(&phone),
                &ScreenInput::Key {
                    key: link2linux_runtime::screen::ScreenKey::Back,
                },
            )
            .unwrap();
        let asking = screen.status(Some(&phone)).unwrap();
        assert_eq!(asking.phase, ScreenPhase::Asking);
        assert_eq!(asking.input_problem, Some(ScreenInputProblem::ControlOff));
        screen.stop(Some(&phone)).unwrap();
        assert_eq!(
            screen.status(Some(&phone)).unwrap().phase,
            ScreenPhase::Idle
        );
    }
}
