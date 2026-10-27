//! What the pretend phone does, the clock the machine reads, and timings.

/// How long one QR offer is valid.
pub const OFFER_LIFETIME_MS: u32 = 300_000;
/// A new offer replaces the old one this long before it expires.
pub const OFFER_REFRESH_BEFORE_MS: u32 = 20_000;
/// Offers shown before giving up with `expired`.
pub const MAX_OFFERS: u8 = 3;
/// How long a code comparison can take.
pub const SAS_LIFETIME_MS: u32 = 60_000;
/// Code comparisons allowed per activation.
pub const SAS_MAX_ATTEMPTS: u8 = 3;

pub const SCAN_DELAY_MS: u64 = 4_000;
pub const DISCOVERY_DELAY_MS: u64 = 1_500;
pub const CONNECT_MS: u64 = 1_200;
pub const VERIFY_MS: u64 = 1_200;
pub const PHONE_CONFIRM_MS: u64 = 1_500;

/// What the pretend phone does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Script {
    /// Pairs successfully in both modes.
    #[default]
    Success,
    /// The phone shows a different code, or its proof fails.
    WrongCode,
    /// Nobody scans the QR code; offers run out.
    Expired,
    /// No phone is ever found.
    NotFound,
    /// The person on the phone says no.
    Declined,
    /// Like `Success`, with two phones already paired.
    Demo,
    /// Like `Demo`, with the first phone connected for calls over
    /// Bluetooth, and a call that rings on it a few seconds after start.
    IncomingCall,
}

impl Script {
    /// Reads a script name such as `wrong-code`.
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "success" => Some(Self::Success),
            "wrong-code" => Some(Self::WrongCode),
            "expired" => Some(Self::Expired),
            "not-found" => Some(Self::NotFound),
            "declined" => Some(Self::Declined),
            "demo" => Some(Self::Demo),
            "incoming-call" => Some(Self::IncomingCall),
            _ => None,
        }
    }

    /// Whether the app starts with the demo phones paired.
    pub fn has_demo_phones(self) -> bool {
        matches!(self, Self::Demo | Self::IncomingCall)
    }
}

/// A point in time: a monotonic reading for deadlines and the wall clock for
/// timestamps shown to the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Now {
    pub mono_ms: u64,
    pub unix_ms: i64,
}

impl Now {
    pub fn back(self, ms: u64) -> Self {
        Self {
            mono_ms: self.mono_ms.saturating_sub(ms),
            unix_ms: self.unix_ms - i64::try_from(ms).unwrap_or(i64::MAX),
        }
    }

    pub fn at(self, mono_ms: u64) -> Self {
        Self {
            mono_ms,
            unix_ms: self.unix_ms
                - i64::try_from(self.mono_ms.saturating_sub(mono_ms)).unwrap_or(0),
        }
    }
}

/// Which of the machine's outputs changed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Changed {
    pub pairing: bool,
    pub devices: bool,
}

impl Changed {
    pub fn merge(&mut self, other: Self) {
        self.pairing |= other.pairing;
        self.devices |= other.devices;
    }
}
