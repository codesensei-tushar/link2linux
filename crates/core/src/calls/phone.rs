//! Which of the desktop's paired Bluetooth devices is a paired phone.
//!
//! The phone's name in its `Hello` is usually its Bluetooth name too, so a
//! paired device of that name is the phone. When names do not settle it,
//! a single device that could be a phone is taken, and with several the
//! user picks one once; the desktop keeps that choice.

use link2linux_platform::{BluetoothAddress, BluetoothDevice};

/// What the desktop knows about a phone's Bluetooth device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PhoneDevice {
    /// This paired device is the phone.
    Found(BluetoothAddress),
    /// Each of these paired devices could be the phone: the user picks.
    Choose(Vec<BluetoothAddress>),
    /// No paired device could be the phone: the user pairs it.
    NotPaired,
}

/// Finds the device of the phone named `phone_name` among `devices`.
/// `chosen` is the device kept for this phone, if any, and `taken` the
/// devices kept for other phones, which this phone is not.
#[must_use]
pub fn find_phone(
    devices: &[BluetoothDevice],
    phone_name: &str,
    chosen: Option<BluetoothAddress>,
    taken: &[BluetoothAddress],
) -> PhoneDevice {
    if let Some(chosen) = chosen
        && devices.iter().any(|d| d.address == chosen && d.paired)
    {
        return PhoneDevice::Found(chosen);
    }
    let candidates: Vec<&BluetoothDevice> = devices
        .iter()
        .filter(|d| d.paired && (d.phone || d.hands_free_gateway))
        .filter(|d| !taken.contains(&d.address))
        .collect();
    let named: Vec<BluetoothAddress> = candidates
        .iter()
        .filter(|d| same_name(&d.name, phone_name))
        .map(|d| d.address)
        .collect();
    match (named.as_slice(), candidates.as_slice()) {
        ([one], _) => PhoneDevice::Found(*one),
        ([], []) => PhoneDevice::NotPaired,
        ([], [only]) => PhoneDevice::Found(only.address),
        ([], several) => PhoneDevice::Choose(several.iter().map(|d| d.address).collect()),
        (several, _) => PhoneDevice::Choose(several.to_vec()),
    }
}

/// Whether two device names are the same, ignoring case and the spaces
/// around them.
fn same_name(a: &str, b: &str) -> bool {
    let (a, b) = (a.trim(), b.trim());
    !a.is_empty() && a.to_lowercase() == b.to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(address: &str, name: &str) -> BluetoothDevice {
        BluetoothDevice {
            address: address.parse().unwrap(),
            name: name.into(),
            paired: true,
            trusted: false,
            connected: false,
            phone: true,
            hands_free_gateway: true,
        }
    }

    fn address(text: &str) -> BluetoothAddress {
        text.parse().unwrap()
    }

    const A: &str = "00:00:00:00:00:0A";
    const B: &str = "00:00:00:00:00:0B";
    const C: &str = "00:00:00:00:00:0C";

    #[test]
    fn the_device_named_like_the_phone_is_the_phone() {
        let devices = [device(A, "Work phone"), device(B, "  pixel 8 ")];
        assert_eq!(
            find_phone(&devices, "Pixel 8", None, &[]),
            PhoneDevice::Found(address(B))
        );
    }

    #[test]
    fn a_kept_choice_wins_while_it_stays_paired() {
        let mut devices = vec![device(A, "Pixel 8"), device(B, "Other")];
        assert_eq!(
            find_phone(&devices, "Pixel 8", Some(address(B)), &[]),
            PhoneDevice::Found(address(B))
        );
        devices[1].paired = false;
        assert_eq!(
            find_phone(&devices, "Pixel 8", Some(address(B)), &[]),
            PhoneDevice::Found(address(A))
        );
    }

    #[test]
    fn a_single_phone_is_taken_and_several_are_offered() {
        let mut headset = device(C, "Headphones");
        headset.phone = false;
        headset.hands_free_gateway = false;
        let one = [device(A, "Galaxy"), headset.clone()];
        assert_eq!(
            find_phone(&one, "Pixel 8", None, &[]),
            PhoneDevice::Found(address(A))
        );
        let two = [device(A, "Galaxy"), device(B, "Moto"), headset];
        assert_eq!(
            find_phone(&two, "Pixel 8", None, &[]),
            PhoneDevice::Choose(vec![address(A), address(B)])
        );
        // Two of the same name are offered too.
        let twins = [device(A, "Pixel 8"), device(B, "Pixel 8"), device(C, "x")];
        assert_eq!(
            find_phone(&twins, "Pixel 8", None, &[]),
            PhoneDevice::Choose(vec![address(A), address(B)])
        );
    }

    #[test]
    fn devices_kept_for_other_phones_and_unpaired_ones_are_left_out() {
        let mut nearby = device(B, "Pixel 8");
        nearby.paired = false;
        let devices = [device(A, "Pixel 8"), nearby];
        assert_eq!(
            find_phone(&devices, "Pixel 8", None, &[address(A)]),
            PhoneDevice::NotPaired
        );
        assert_eq!(
            find_phone(&[], "Pixel 8", None, &[]),
            PhoneDevice::NotPaired
        );
    }

    #[test]
    fn a_device_that_only_offers_calls_counts_as_a_phone() {
        let mut gateway = device(A, "Car phone");
        gateway.phone = false;
        assert_eq!(
            find_phone(&[gateway], "Pixel 8", None, &[]),
            PhoneDevice::Found(address(A))
        );
    }
}
