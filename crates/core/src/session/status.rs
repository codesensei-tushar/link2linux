//! What each side reports about its features after `Hello`, in
//! `FeatureStatus`.

use std::collections::{BTreeMap, BTreeSet};

use link2linux_protocol::v1::{FeatureAvailability, FeatureStatus};
use tracing::debug;

use crate::device::Feature;

/// Whether a feature can be used right now on one device.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Availability {
    /// Usable.
    Ready,
    /// The user turned the feature off on that device.
    Disabled,
    /// An OS permission is missing on that device. The token names it, such
    /// as `notification-access`, so the other device can show a setup hint.
    PermissionRequired(String),
    /// The device cannot offer the feature, for example a phone without
    /// telephony. Availabilities this build does not know read as this.
    Unavailable,
}

impl Availability {
    /// Whether the feature is usable.
    #[must_use]
    pub fn is_ready(&self) -> bool {
        matches!(self, Self::Ready)
    }
}

/// The availability of each feature one device listed in its `Hello`: the
/// content of a `FeatureStatus`.
pub type FeatureStates = BTreeMap<Feature, Availability>;

/// Reads the peer's `FeatureStatus`, keeping the features it listed in its
/// `Hello` and this build knows.
pub(crate) fn states_from(status: FeatureStatus, listed: &BTreeSet<Feature>) -> FeatureStates {
    let mut states = FeatureStates::new();
    for state in status.features {
        let Some(feature) = Feature::from_name(&state.feature).filter(|f| listed.contains(f))
        else {
            debug!(
                feature = state.feature,
                "ignoring the state of an unknown feature"
            );
            continue;
        };
        let availability = match state.availability() {
            FeatureAvailability::Ready => Availability::Ready,
            FeatureAvailability::Disabled => Availability::Disabled,
            FeatureAvailability::PermissionRequired => {
                Availability::PermissionRequired(state.permission)
            }
            FeatureAvailability::Unavailable | FeatureAvailability::Unspecified => {
                Availability::Unavailable
            }
        };
        states.insert(feature, availability);
    }
    states
}

#[cfg(test)]
mod tests {
    use link2linux_protocol::v1::FeatureState;

    use super::*;

    #[test]
    fn unknown_names_and_values_are_tolerated() {
        let status = FeatureStatus {
            features: vec![
                FeatureState {
                    feature: "teleport".into(),
                    availability: FeatureAvailability::Ready.into(),
                    ..FeatureState::default()
                },
                FeatureState {
                    feature: "files".into(),
                    availability: 42,
                    ..FeatureState::default()
                },
                // Not in the peer's Hello.
                FeatureState {
                    feature: "screen".into(),
                    availability: FeatureAvailability::Ready.into(),
                    ..FeatureState::default()
                },
            ],
        };
        let listed = BTreeSet::from([Feature::Files]);
        let states = states_from(status, &listed);
        assert_eq!(
            states,
            FeatureStates::from([(Feature::Files, Availability::Unavailable)])
        );
        assert!(!states[&Feature::Files].is_ready());
    }
}
