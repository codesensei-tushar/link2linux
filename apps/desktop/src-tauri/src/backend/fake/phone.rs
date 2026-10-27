//! The pretend phones' status, and ringing them: a connected phone shows a
//! fixed status, and rings until it is stopped or its time is over.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use link2linux_runtime::view::{
    CellularNetwork, PhoneBattery, PhoneCellular, PhoneWifi, RingEnd, RingPhase, RingProblem,
    RingerMode,
};

use crate::backend::{BackendError, Device, DeviceStatus, EventSink, PhoneStatus, RingStatus};

/// How long a pretend phone rings, as a real one does.
pub const RING_FOR: Duration = Duration::from_secs(60);

/// Ringing each pretend phone.
pub struct FakePhone {
    sink: Arc<dyn EventSink>,
    ring_for: Duration,
    /// Each phone's ringing, with a count of rings and stops so that the
    /// end of an earlier ring leaves a later one alone.
    rings: Arc<Mutex<HashMap<String, (RingStatus, u64)>>>,
}

impl FakePhone {
    /// Phones that report to `sink`, and ring for `ring_for`.
    pub fn new(sink: Arc<dyn EventSink>, ring_for: Duration) -> Self {
        Self {
            sink,
            ring_for,
            rings: Arc::default(),
        }
    }

    /// The status of `device`, which is `None` when it is not paired.
    pub fn status(device: Option<&Device>) -> Result<PhoneStatus, BackendError> {
        let device = device.ok_or(BackendError::NotFound)?;
        Ok(if device.status == DeviceStatus::Connected {
            PhoneStatus::Current {
                battery: Some(PhoneBattery {
                    percent: Some(76),
                    plugged_in: false,
                    power_saver: false,
                }),
                cellular: Some(PhoneCellular {
                    level: Some(3),
                    network: Some(CellularNetwork::FiveG),
                    operator_name: Some("Carrier".to_owned()),
                }),
                wifi: Some(PhoneWifi { level: Some(4) }),
                ringer: Some(RingerMode::Vibrate),
                do_not_disturb: false,
            }
        } else {
            PhoneStatus::Disconnected
        })
    }

    /// Ringing `device`, which is `None` when it is not paired.
    pub fn ring_status(&self, device: Option<&Device>) -> Result<RingStatus, BackendError> {
        let device = device.ok_or(BackendError::NotFound)?;
        Ok(current(&lock(&self.rings), device))
    }

    /// Makes `device` ring, or says it is not connected.
    pub fn ring(&self, device: Option<&Device>) -> Result<(), BackendError> {
        let device = device.ok_or(BackendError::NotFound)?;
        let generation = {
            let mut rings = lock(&self.rings);
            let mut ring = current(&rings, device);
            let generation = rings.get(&device.id).map_or(0, |(_, count)| count + 1);
            ring.ended = None;
            if ring.blocked.is_some() {
                ring.problem = ring.blocked;
            } else {
                ring.problem = None;
                ring.phase = RingPhase::Ringing;
            }
            rings.insert(device.id.clone(), (ring, generation));
            self.sink.ring_changed(&device.id, &ring);
            if ring.phase != RingPhase::Ringing {
                return Ok(());
            }
            generation
        };
        let (rings, sink, id, ring_for) = (
            self.rings.clone(),
            self.sink.clone(),
            device.id.clone(),
            self.ring_for,
        );
        let _ = std::thread::Builder::new()
            .name("fake-ring".to_owned())
            .spawn(move || {
                std::thread::sleep(ring_for);
                let mut rings = lock(&rings);
                if let Some((ring, count)) = rings.get_mut(&id)
                    && *count == generation
                    && ring.phase == RingPhase::Ringing
                {
                    ring.phase = RingPhase::Idle;
                    ring.ended = Some(RingEnd::TimedOut);
                    sink.ring_changed(&id, ring);
                }
            });
        Ok(())
    }

    /// Stops `device` ringing.
    pub fn stop(&self, device: Option<&Device>) -> Result<(), BackendError> {
        let device = device.ok_or(BackendError::NotFound)?;
        let mut rings = lock(&self.rings);
        if let Some((ring, count)) = rings.get_mut(&device.id)
            && ring.phase == RingPhase::Ringing
        {
            ring.phase = RingPhase::Idle;
            ring.problem = None;
            *count += 1;
            self.sink.ring_changed(&device.id, ring);
        }
        Ok(())
    }
}

/// Ringing `device` as last reported, with what its connection allows now.
fn current(rings: &HashMap<String, (RingStatus, u64)>, device: &Device) -> RingStatus {
    let connected = device.status == DeviceStatus::Connected;
    let mut ring = rings
        .get(&device.id)
        .map_or_else(RingStatus::default, |(ring, _)| *ring);
    ring.blocked = (!connected).then_some(RingProblem::NotConnected);
    if !connected {
        ring.phase = RingPhase::Idle;
    }
    ring
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex as StdMutex;
    use std::sync::mpsc;

    use super::*;
    use crate::backend::PairingState;

    #[derive(Default)]
    struct Rings {
        seen: StdMutex<Vec<RingStatus>>,
        told: StdMutex<Option<mpsc::Sender<RingStatus>>>,
    }

    impl EventSink for Rings {
        fn pairing_changed(&self, _state: &PairingState) {}
        fn devices_changed(&self, _devices: &[Device]) {}
        fn ring_changed(&self, _device_id: &str, ring: &RingStatus) {
            self.seen.lock().unwrap().push(*ring);
            if let Some(told) = &*self.told.lock().unwrap() {
                let _ = told.send(*ring);
            }
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

    #[test]
    fn a_connected_phone_shows_a_status_and_rings_until_stopped() {
        let sink = Arc::new(Rings::default());
        let phone = FakePhone::new(sink.clone(), Duration::from_secs(600));
        let connected = device(DeviceStatus::Connected);
        assert!(matches!(
            FakePhone::status(Some(&connected)),
            Ok(PhoneStatus::Current { .. })
        ));
        let idle = phone.ring_status(Some(&connected)).unwrap();
        assert_eq!((idle.phase, idle.blocked), (RingPhase::Idle, None));

        phone.ring(Some(&connected)).unwrap();
        assert_eq!(
            phone.ring_status(Some(&connected)).unwrap().phase,
            RingPhase::Ringing
        );
        phone.stop(Some(&connected)).unwrap();
        let phases: Vec<_> = sink.seen.lock().unwrap().iter().map(|r| r.phase).collect();
        assert_eq!(phases, [RingPhase::Ringing, RingPhase::Idle]);
        assert_eq!(FakePhone::status(None), Err(BackendError::NotFound));
    }

    #[test]
    fn a_phone_away_is_not_connected_and_a_ring_ends_by_itself() {
        let sink = Arc::new(Rings::default());
        let phone = FakePhone::new(sink.clone(), Duration::from_millis(10));
        let away = device(DeviceStatus::Disconnected);
        assert_eq!(
            FakePhone::status(Some(&away)),
            Ok(PhoneStatus::Disconnected)
        );
        phone.ring(Some(&away)).unwrap();
        let ring = phone.ring_status(Some(&away)).unwrap();
        assert_eq!(ring.problem, Some(RingProblem::NotConnected));
        assert_eq!(ring.phase, RingPhase::Idle);

        let (told, heard) = mpsc::channel();
        *sink.told.lock().unwrap() = Some(told);
        phone.ring(Some(&device(DeviceStatus::Connected))).unwrap();
        let ended = heard
            .iter()
            .find(|ring| ring.phase == RingPhase::Idle)
            .unwrap();
        assert_eq!(ended.ended, Some(RingEnd::TimedOut));
    }
}
