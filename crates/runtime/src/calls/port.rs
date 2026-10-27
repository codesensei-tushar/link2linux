//! The desktop's Bluetooth and telephony, as one trait object.

use std::time::Duration;

use link2linux_platform::{
    Bluetooth, BluetoothAdapter, BluetoothAddress, BluetoothDevice, Gateway, PairingEvent,
    PlatformError, Telephony,
};

use crate::notify::BoxFuture;

/// The platform's [`Bluetooth`] and [`Telephony`] together, usable as a
/// trait object. Each method is the trait method of the same name.
#[allow(
    missing_docs,
    reason = "each method is documented on the platform trait"
)]
pub trait CallsPort: Send + Sync {
    fn adapter(&self) -> BoxFuture<'_, Result<Option<BluetoothAdapter>, PlatformError>>;
    fn devices(&self) -> BoxFuture<'_, Result<Vec<BluetoothDevice>, PlatformError>>;
    fn bluetooth_changed(&self) -> BoxFuture<'_, Result<(), PlatformError>>;
    fn open_pairing(
        &self,
        window: Duration,
    ) -> BoxFuture<'_, Result<BluetoothAdapter, PlatformError>>;
    fn close_pairing(&self) -> BoxFuture<'_, Result<(), PlatformError>>;
    fn next_pairing_event(&self) -> BoxFuture<'_, PairingEvent>;
    fn answer_pairing(&self, id: u64, accept: bool);
    fn connect_hands_free(
        &self,
        device: BluetoothAddress,
    ) -> BoxFuture<'_, Result<(), PlatformError>>;
    fn disconnect_hands_free(
        &self,
        device: BluetoothAddress,
    ) -> BoxFuture<'_, Result<(), PlatformError>>;
    fn gateways(&self) -> BoxFuture<'_, Result<Vec<Gateway>, PlatformError>>;
    fn telephony_changed(&self) -> BoxFuture<'_, Result<(), PlatformError>>;
    fn answer<'a>(&'a self, call: &'a str) -> BoxFuture<'a, Result<(), PlatformError>>;
    fn hang_up<'a>(&'a self, call: &'a str) -> BoxFuture<'a, Result<(), PlatformError>>;
    fn dial<'a>(
        &'a self,
        gateway: &'a str,
        number: &'a str,
    ) -> BoxFuture<'a, Result<(), PlatformError>>;
    fn send_tones<'a>(
        &'a self,
        gateway: &'a str,
        tones: &'a str,
    ) -> BoxFuture<'a, Result<(), PlatformError>>;
    fn set_muted<'a>(
        &'a self,
        gateway: &'a str,
        muted: bool,
    ) -> BoxFuture<'a, Result<(), PlatformError>>;
}

/// A Bluetooth stack and a telephony service, as one [`CallsPort`].
#[derive(Debug, Default)]
pub struct CallStack<B, T> {
    /// The adapter, pairing and the hands-free profile.
    pub bluetooth: B,
    /// The calls.
    pub telephony: T,
}

impl<B: Bluetooth, T: Telephony> CallsPort for CallStack<B, T> {
    fn adapter(&self) -> BoxFuture<'_, Result<Option<BluetoothAdapter>, PlatformError>> {
        Box::pin(self.bluetooth.adapter())
    }

    fn devices(&self) -> BoxFuture<'_, Result<Vec<BluetoothDevice>, PlatformError>> {
        Box::pin(self.bluetooth.devices())
    }

    fn bluetooth_changed(&self) -> BoxFuture<'_, Result<(), PlatformError>> {
        Box::pin(self.bluetooth.changed())
    }

    fn open_pairing(
        &self,
        window: Duration,
    ) -> BoxFuture<'_, Result<BluetoothAdapter, PlatformError>> {
        Box::pin(self.bluetooth.open_pairing(window))
    }

    fn close_pairing(&self) -> BoxFuture<'_, Result<(), PlatformError>> {
        Box::pin(self.bluetooth.close_pairing())
    }

    fn next_pairing_event(&self) -> BoxFuture<'_, PairingEvent> {
        Box::pin(self.bluetooth.next_pairing_event())
    }

    fn answer_pairing(&self, id: u64, accept: bool) {
        self.bluetooth.answer_pairing(id, accept);
    }

    fn connect_hands_free(
        &self,
        device: BluetoothAddress,
    ) -> BoxFuture<'_, Result<(), PlatformError>> {
        Box::pin(self.bluetooth.connect_hands_free(device))
    }

    fn disconnect_hands_free(
        &self,
        device: BluetoothAddress,
    ) -> BoxFuture<'_, Result<(), PlatformError>> {
        Box::pin(self.bluetooth.disconnect_hands_free(device))
    }

    fn gateways(&self) -> BoxFuture<'_, Result<Vec<Gateway>, PlatformError>> {
        Box::pin(self.telephony.gateways())
    }

    fn telephony_changed(&self) -> BoxFuture<'_, Result<(), PlatformError>> {
        Box::pin(self.telephony.changed())
    }

    fn answer<'a>(&'a self, call: &'a str) -> BoxFuture<'a, Result<(), PlatformError>> {
        Box::pin(self.telephony.answer(call))
    }

    fn hang_up<'a>(&'a self, call: &'a str) -> BoxFuture<'a, Result<(), PlatformError>> {
        Box::pin(self.telephony.hang_up(call))
    }

    fn dial<'a>(
        &'a self,
        gateway: &'a str,
        number: &'a str,
    ) -> BoxFuture<'a, Result<(), PlatformError>> {
        Box::pin(self.telephony.dial(gateway, number))
    }

    fn send_tones<'a>(
        &'a self,
        gateway: &'a str,
        tones: &'a str,
    ) -> BoxFuture<'a, Result<(), PlatformError>> {
        Box::pin(self.telephony.send_tones(gateway, tones))
    }

    fn set_muted<'a>(
        &'a self,
        gateway: &'a str,
        muted: bool,
    ) -> BoxFuture<'a, Result<(), PlatformError>> {
        Box::pin(self.telephony.set_muted(gateway, muted))
    }
}
