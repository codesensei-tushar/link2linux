//! Protocol version negotiation.

use std::fmt;

use link2linux_protocol::{Hello, PROTOCOL_VERSION};

/// The protocol major versions one side speaks, from its lowest to its
/// highest.
///
/// Each side announces its range in `Hello`, as `min_protocol_version` and
/// `protocol_version`, and the session uses the highest version both ranges
/// contain. Both sides compute it from the two Hellos, so they agree without
/// another round trip.
///
/// ```
/// use link2linux_core::session::VersionRange;
///
/// let ours = VersionRange::new(1, 2).unwrap();
/// let theirs = VersionRange::new(2, 4).unwrap();
/// assert_eq!(ours.negotiate(theirs), Some(2));
///
/// let newer = VersionRange::new(3, 4).unwrap();
/// assert_eq!(ours.negotiate(newer), None);
/// assert!(ours.is_older_than(newer));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct VersionRange {
    min: u32,
    max: u32,
}

impl VersionRange {
    /// The versions this build speaks.
    pub const SUPPORTED: Self = Self {
        min: PROTOCOL_VERSION,
        max: PROTOCOL_VERSION,
    };

    /// The versions from `min` to `max`, both included. `None` when `min`
    /// is 0 or above `max`.
    #[must_use]
    pub const fn new(min: u32, max: u32) -> Option<Self> {
        if min == 0 || min > max {
            None
        } else {
            Some(Self { min, max })
        }
    }

    /// The range a `Hello` announces. A `min_protocol_version` of 0 stands
    /// for `protocol_version`. `None` when the `Hello` holds no valid range.
    #[must_use]
    pub fn of_hello(hello: &Hello) -> Option<Self> {
        let min = match hello.min_protocol_version {
            0 => hello.protocol_version,
            min => min,
        };
        Self::new(min, hello.protocol_version)
    }

    /// The lowest version in the range.
    #[must_use]
    pub const fn min(self) -> u32 {
        self.min
    }

    /// The highest version in the range.
    #[must_use]
    pub const fn max(self) -> u32 {
        self.max
    }

    /// Whether the range contains `version`.
    #[must_use]
    pub const fn contains(self, version: u32) -> bool {
        self.min <= version && version <= self.max
    }

    /// The highest version both ranges contain, or `None` when they do not
    /// overlap.
    #[must_use]
    pub fn negotiate(self, other: Self) -> Option<u32> {
        let highest = self.max.min(other.max);
        (highest >= self.min.max(other.min)).then_some(highest)
    }

    /// Whether every version in this range is below every version in
    /// `other`. When two ranges do not overlap, the side with the older
    /// range is the one whose app needs an update.
    #[must_use]
    pub const fn is_older_than(self, other: Self) -> bool {
        self.max < other.min
    }

    /// Writes the range into the version fields of `hello`.
    pub(crate) fn write_to(self, hello: &mut Hello) {
        hello.protocol_version = self.max;
        hello.min_protocol_version = self.min;
    }
}

impl fmt::Display for VersionRange {
    /// `1` for a single version, `1 to 3` for a wider range.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.min == self.max {
            write!(f, "{}", self.max)
        } else {
            write!(f, "{} to {}", self.min, self.max)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn range(min: u32, max: u32) -> VersionRange {
        VersionRange::new(min, max).unwrap()
    }

    #[test]
    fn rejects_empty_ranges() {
        assert_eq!(VersionRange::new(0, 1), None);
        assert_eq!(VersionRange::new(2, 1), None);
        assert_eq!(VersionRange::new(1, 1), Some(range(1, 1)));
    }

    #[test]
    fn picks_the_highest_common_version() {
        assert_eq!(range(1, 1).negotiate(range(1, 1)), Some(1));
        assert_eq!(range(1, 3).negotiate(range(2, 5)), Some(3));
        assert_eq!(range(2, 5).negotiate(range(1, 3)), Some(3));
        assert_eq!(range(1, 5).negotiate(range(2, 3)), Some(3));
        assert_eq!(range(1, 2).negotiate(range(3, 4)), None);
        assert_eq!(range(3, 4).negotiate(range(1, 2)), None);
    }

    #[test]
    fn tells_which_side_is_older() {
        assert!(range(1, 2).is_older_than(range(3, 4)));
        assert!(!range(3, 4).is_older_than(range(1, 2)));
        assert!(!range(1, 3).is_older_than(range(3, 4)));
    }

    #[test]
    fn reads_and_writes_hello_fields() {
        let mut hello = Hello {
            protocol_version: 3,
            ..Hello::default()
        };
        assert_eq!(VersionRange::of_hello(&hello), Some(range(3, 3)));
        hello.min_protocol_version = 2;
        assert_eq!(VersionRange::of_hello(&hello), Some(range(2, 3)));
        hello.min_protocol_version = 4;
        assert_eq!(VersionRange::of_hello(&hello), None);
        hello.protocol_version = 0;
        hello.min_protocol_version = 0;
        assert_eq!(VersionRange::of_hello(&hello), None);

        range(1, 2).write_to(&mut hello);
        assert_eq!((hello.min_protocol_version, hello.protocol_version), (1, 2));
    }

    #[test]
    fn displays_like_prose() {
        assert_eq!(range(1, 1).to_string(), "1");
        assert_eq!(range(1, 3).to_string(), "1 to 3");
        assert_eq!(VersionRange::SUPPORTED.max(), PROTOCOL_VERSION);
    }
}
