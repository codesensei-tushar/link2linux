//! Which announced phones matter, and what changed.
//!
//! mDNS shows every phone with the app on the network. The tracker keeps
//! the records, checks their tags against the current keys, and reports
//! only phones that matter: a paired phone, the phone that scanned the QR
//! offer, or a phone in SAS pairing mode. It is a pure state machine, so
//! the network code stays thin and the logic is testable.
//!
//! A repeated announcement of a known phone is reported as
//! [`TrackerEvent::Updated`] even when nothing changed, because an
//! announcement is fresh evidence that the phone is reachable. Callers
//! rate-limit the dials this triggers.

use std::collections::HashMap;
use std::net::SocketAddr;

use super::presence::{DiscoveryKeys, TxtRecord};

/// The most records kept; the oldest unmatched ones go first.
const MAX_RECORDS: usize = 512;

/// A phone announced over mDNS.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhoneRecord {
    /// The mDNS instance name, such as `l2l-3f9a12bc`.
    pub instance: String,
    /// Where it listens: its addresses with the announced port. IPv6
    /// link-local addresses carry their interface as scope id.
    pub addresses: Vec<SocketAddr>,
    /// The parsed TXT record.
    pub txt: TxtRecord,
}

/// Why a phone matters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Match {
    /// A presence tag made with this paired phone's key.
    Paired {
        /// The phone's id in the pairing store.
        phone_id: String,
    },
    /// The pairing tag of the QR offer being shown.
    Offer,
    /// The phone is in SAS pairing mode (`pm=1`).
    PairingMode,
}

/// A phone that matters, with the reasons.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sighting {
    /// The announcement.
    pub record: PhoneRecord,
    /// Why it matters; never empty.
    pub matches: Vec<Match>,
}

/// A change in the set of phones that matter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrackerEvent {
    /// A phone started to matter.
    Found(Sighting),
    /// An announcement of a phone already found, or its matches changed.
    Updated(Sighting),
    /// A phone that mattered was withdrawn or stopped matching.
    Lost {
        /// Its instance name.
        instance: String,
    },
}

/// Checks a record against the keys.
#[must_use]
pub fn classify(txt: &TxtRecord, keys: &DiscoveryKeys) -> Vec<Match> {
    let mut matches = Vec::new();
    for paired in &keys.paired {
        if txt.matches_presence(paired.presence_key.as_bytes()) {
            matches.push(Match::Paired {
                phone_id: paired.phone_id.clone(),
            });
        }
    }
    if keys
        .offer_secret
        .as_ref()
        .is_some_and(|secret| txt.matches_pairing(secret.as_bytes()))
    {
        matches.push(Match::Offer);
    }
    if txt.pairing_mode {
        matches.push(Match::PairingMode);
    }
    matches
}

/// Tracks announced phones.
#[derive(Debug, Default)]
pub struct PhoneTracker {
    keys: DiscoveryKeys,
    records: HashMap<String, (u64, PhoneRecord)>,
    active: HashMap<String, Vec<Match>>,
    counter: u64,
}

impl PhoneTracker {
    /// A tracker that knows no keys yet.
    #[must_use]
    pub fn new(keys: DiscoveryKeys) -> Self {
        Self {
            keys,
            ..Self::default()
        }
    }

    /// Handles a resolved or re-announced service.
    pub fn resolved(&mut self, record: PhoneRecord) -> Option<TrackerEvent> {
        let matches = classify(&record.txt, &self.keys);
        let instance = record.instance.clone();
        self.counter += 1;
        self.records
            .insert(instance.clone(), (self.counter, record.clone()));
        self.evict();
        self.transition(&instance, record, matches, true)
    }

    /// Handles a withdrawn service. The desktop does not dial on removals,
    /// but a user interface wants to drop the phone from a list.
    pub fn removed(&mut self, instance: &str) -> Option<TrackerEvent> {
        self.records.remove(instance);
        self.active.remove(instance).map(|_| TrackerEvent::Lost {
            instance: instance.to_owned(),
        })
    }

    /// Replaces the keys and re-checks every stored record, so a phone
    /// paired a moment ago is found without waiting for its next
    /// announcement, and a removed pairing stops matching.
    pub fn set_keys(&mut self, keys: DiscoveryKeys) -> Vec<TrackerEvent> {
        self.keys = keys;
        let mut instances: Vec<_> = self.records.keys().cloned().collect();
        instances.sort();
        let mut events = Vec::new();
        for instance in instances {
            let record = self.records[&instance].1.clone();
            let matches = classify(&record.txt, &self.keys);
            events.extend(self.transition(&instance, record, matches, false));
        }
        events
    }

    /// The phones that matter now, ordered by instance name.
    #[must_use]
    pub fn sightings(&self) -> Vec<Sighting> {
        let mut out: Vec<_> = self
            .active
            .iter()
            .filter_map(|(instance, matches)| {
                self.records.get(instance).map(|(_, record)| Sighting {
                    record: record.clone(),
                    matches: matches.clone(),
                })
            })
            .collect();
        out.sort_by(|a, b| a.record.instance.cmp(&b.record.instance));
        out
    }

    fn transition(
        &mut self,
        instance: &str,
        record: PhoneRecord,
        matches: Vec<Match>,
        fresh_announcement: bool,
    ) -> Option<TrackerEvent> {
        let before = self.active.get(instance).cloned();
        if matches.is_empty() {
            return before.map(|_| {
                self.active.remove(instance);
                TrackerEvent::Lost {
                    instance: instance.to_owned(),
                }
            });
        }
        self.active.insert(instance.to_owned(), matches.clone());
        let sighting = Sighting { record, matches };
        match before {
            None => Some(TrackerEvent::Found(sighting)),
            Some(old) if fresh_announcement || old != sighting.matches => {
                Some(TrackerEvent::Updated(sighting))
            }
            Some(_) => None,
        }
    }

    fn evict(&mut self) {
        while self.records.len() > MAX_RECORDS {
            let victim = self
                .records
                .iter()
                .filter(|(instance, _)| !self.active.contains_key(*instance))
                .min_by_key(|(_, (seq, _))| *seq)
                .map(|(instance, _)| instance.clone());
            match victim {
                Some(instance) => {
                    self.records.remove(&instance);
                }
                None => break,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discovery::presence::{
        PairedKey, SecretBytes, encode_value, pairing_tag, presence_tag,
    };

    fn key(byte: u8) -> SecretBytes {
        SecretBytes::new(vec![byte; 32])
    }

    fn keys(phone: &str, byte: u8) -> DiscoveryKeys {
        DiscoveryKeys {
            paired: vec![PairedKey {
                phone_id: phone.to_owned(),
                presence_key: key(byte),
            }],
            offer_secret: None,
        }
    }

    fn record(instance: &str, txt: &[(&str, String)]) -> PhoneRecord {
        PhoneRecord {
            instance: instance.to_owned(),
            addresses: vec!["192.168.1.9:27380".parse().unwrap()],
            txt: TxtRecord::parse(txt.iter().map(|(k, v)| (*k, v.as_str()))),
        }
    }

    fn presence_txt(byte: u8, nonce: [u8; 8]) -> Vec<(&'static str, String)> {
        vec![
            ("n", encode_value(&nonce)),
            ("t", encode_value(&presence_tag(&[byte; 32], &nonce))),
        ]
    }

    #[test]
    fn strangers_are_not_reported() {
        let mut t = PhoneTracker::new(keys("a", 1));
        assert_eq!(t.resolved(record("l2l-1", &presence_txt(9, [1; 8]))), None);
        assert!(t.sightings().is_empty());
    }

    #[test]
    fn found_then_updated_then_lost() {
        let mut t = PhoneTracker::new(keys("a", 1));
        let first = t.resolved(record("l2l-1", &presence_txt(1, [1; 8])));
        assert!(matches!(&first, Some(TrackerEvent::Found(s))
            if s.matches == vec![Match::Paired { phone_id: "a".into() }]));
        let again = t.resolved(record("l2l-1", &presence_txt(1, [1; 8])));
        assert!(matches!(again, Some(TrackerEvent::Updated(_))));
        // A new nonce and instance name: the old name is withdrawn.
        let renamed = t.resolved(record("l2l-2", &presence_txt(1, [2; 8])));
        assert!(matches!(renamed, Some(TrackerEvent::Found(_))));
        assert_eq!(
            t.removed("l2l-1"),
            Some(TrackerEvent::Lost {
                instance: "l2l-1".into()
            })
        );
        assert_eq!(t.removed("l2l-1"), None);
        assert_eq!(t.sightings().len(), 1);
    }

    #[test]
    fn one_of_three_tags_is_enough() {
        let nonce = [4; 8];
        let txt = vec![
            ("n", encode_value(&nonce)),
            ("t", encode_value(&presence_tag(&[7; 32], &nonce))),
            ("t2", encode_value(&presence_tag(&[8; 32], &nonce))),
            ("t3", encode_value(&presence_tag(&[1; 32], &nonce))),
        ];
        let mut t = PhoneTracker::new(keys("a", 1));
        assert!(matches!(
            t.resolved(record("l2l-1", &txt)),
            Some(TrackerEvent::Found(_))
        ));
    }

    #[test]
    fn new_keys_recheck_stored_records() {
        let mut t = PhoneTracker::new(DiscoveryKeys::default());
        assert_eq!(t.resolved(record("l2l-1", &presence_txt(1, [1; 8]))), None);
        let events = t.set_keys(keys("a", 1));
        assert!(matches!(events.as_slice(), [TrackerEvent::Found(_)]));
        // Same keys again: nothing changed.
        assert!(t.set_keys(keys("a", 1)).is_empty());
        // The pairing is removed.
        assert_eq!(
            t.set_keys(DiscoveryKeys::default()),
            vec![TrackerEvent::Lost {
                instance: "l2l-1".into()
            }]
        );
    }

    #[test]
    fn the_offer_and_pairing_mode_match() {
        let secret = [5u8; 16];
        let mut t = PhoneTracker::new(DiscoveryKeys {
            paired: vec![],
            offer_secret: Some(SecretBytes::new(secret.to_vec())),
        });
        let q = vec![("q", encode_value(&pairing_tag(&secret)))];
        let event = t.resolved(record("l2l-1", &q));
        assert!(matches!(&event, Some(TrackerEvent::Found(s)) if s.matches == vec![Match::Offer]));
        let pm = vec![("pm", "1".to_owned())];
        let event = t.resolved(record("l2l-2", &pm));
        assert!(
            matches!(&event, Some(TrackerEvent::Found(s)) if s.matches == vec![Match::PairingMode])
        );
    }

    #[test]
    fn stored_records_are_bounded() {
        let mut t = PhoneTracker::new(keys("a", 1));
        for i in 0..MAX_RECORDS + 50 {
            t.resolved(record(&format!("l2l-{i}"), &[]));
        }
        assert_eq!(t.records.len(), MAX_RECORDS);
        // A matching phone survives the flood.
        t.resolved(record("mine", &presence_txt(1, [1; 8])));
        for i in 0..MAX_RECORDS {
            t.resolved(record(&format!("x-{i}"), &[]));
        }
        assert_eq!(t.sightings().len(), 1);
    }
}
