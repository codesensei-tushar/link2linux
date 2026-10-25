//! Device identity and the feature names both apps share.

use std::collections::BTreeSet;
use std::fmt;

pub use link2linux_protocol::DeviceKind;

/// A feature one device can serve to the other.
///
/// The names match the app modules on both platforms (`feature/<name>` on
/// Android, `src/features/<name>` on the desktop, a module in this crate)
/// and are the strings sent in `Hello.features`. Pairing is not listed:
/// every session runs between devices that are already paired.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub enum Feature {
    /// Phone notifications mirrored on the desktop.
    Notifications,
    /// SMS and MMS conversations.
    Messages,
    /// Placing, answering and ending calls.
    Calls,
    /// Recent photos from the phone's gallery.
    Photos,
    /// Sending files in either direction.
    Files,
    /// A clipboard shared between both devices.
    Clipboard,
    /// The phone's screen shown and controlled on the desktop.
    Screen,
}

impl Feature {
    /// Every feature, in a stable order.
    pub const ALL: [Self; 7] = [
        Self::Notifications,
        Self::Messages,
        Self::Calls,
        Self::Photos,
        Self::Files,
        Self::Clipboard,
        Self::Screen,
    ];

    /// The wire name, as sent in `Hello.features`.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Notifications => "notifications",
            Self::Messages => "messages",
            Self::Calls => "calls",
            Self::Photos => "photos",
            Self::Files => "files",
            Self::Clipboard => "clipboard",
            Self::Screen => "screen",
        }
    }

    /// Looks a feature up by its wire name. Names from newer app versions
    /// return `None` and are ignored by older peers.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|feature| feature.name() == name)
    }
}

impl fmt::Display for Feature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// Who a device is: what it announces in its `Hello`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceInfo {
    /// Stable identifier, unique per installation. Pairing records are keyed
    /// by it.
    pub id: String,
    /// Human-readable name shown on the other device.
    pub name: String,
    /// Whether this is a desktop or a phone.
    pub kind: DeviceKind,
    /// Features this device can serve.
    pub features: BTreeSet<Feature>,
}

impl DeviceInfo {
    /// A device with the given identity and features.
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        kind: DeviceKind,
        features: impl IntoIterator<Item = Feature>,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            kind,
            features: features.into_iter().collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feature_names_round_trip() {
        for feature in Feature::ALL {
            assert_eq!(Feature::from_name(feature.name()), Some(feature));
            assert_eq!(feature.to_string(), feature.name());
        }
        assert_eq!(Feature::from_name("pairing"), None);
        assert_eq!(Feature::from_name("Clipboard"), None);
    }
}
