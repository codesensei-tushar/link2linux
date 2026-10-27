//! Which of the desktop's paired Bluetooth devices is a paired phone.
//!
//! The phone's name in its `Hello` is usually its Bluetooth name too, so a
//! paired device of that name is the phone, when the Bluetooth stack
//! trusts it. Otherwise the user picks the phone's device among those that
//! could be it, even when there is only one, and the desktop keeps that
//! choice, as it keeps the device whose pairing code the user confirmed.
//!
//! Nothing else is taken without asking. A nearby device can pair with no
//! prompt at all: the kernel accepts "just works" pairing by itself when
//! neither side asks for protection, so being paired says nothing about
//! what the user wanted. The desktop's Bluetooth settings trust a device
//! the user paired there. And a device that could be a phone may be
//! another phone, such as an old one, whose calls would then ring here.

use link2linux_platform::{BluetoothAddress, BluetoothDevice};

/// The device kept for a phone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Chosen {
    /// The device's address.
    pub address: BluetoothAddress,
    /// The user picked it, or confirmed its pairing code, since the
    /// desktop started, so it is the phone's before the Bluetooth stack
    /// trusts it. A kept device that is not trusted is offered to the
    /// user again otherwise.
    pub confirmed: bool,
}

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
    chosen: Option<Chosen>,
    taken: &[BluetoothAddress],
) -> PhoneDevice {
    if let Some(chosen) = chosen
        && devices
            .iter()
            .any(|d| d.address == chosen.address && d.paired && (d.trusted || chosen.confirmed))
    {
        return PhoneDevice::Found(chosen.address);
    }
    let candidates: Vec<&BluetoothDevice> = devices
        .iter()
        .filter(|d| d.paired && (d.phone || d.hands_free_gateway))
        .filter(|d| !taken.contains(&d.address))
        .collect();
    let named: Vec<BluetoothAddress> = candidates
        .iter()
        .filter(|d| d.trusted && same_name(&d.name, phone_name))
        .map(|d| d.address)
        .collect();
    match (named.as_slice(), candidates.as_slice()) {
        ([one], _) => PhoneDevice::Found(*one),
        ([], []) => PhoneDevice::NotPaired,
        ([], all) => PhoneDevice::Choose(all.iter().map(|d| d.address).collect()),
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
            trusted: true,
            connected: false,
            phone: true,
            hands_free_gateway: true,
        }
    }

    fn address(text: &str) -> BluetoothAddress {
        text.parse().unwrap()
    }

    fn kept(text: &str) -> Option<Chosen> {
        Some(Chosen {
            address: address(text),
            confirmed: false,
        })
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
            find_phone(&devices, "Pixel 8", kept(B), &[]),
            PhoneDevice::Found(address(B))
        );
        devices[1].paired = false;
        assert_eq!(
            find_phone(&devices, "Pixel 8", kept(B), &[]),
            PhoneDevice::Found(address(A))
        );
    }

    #[test]
    fn devices_not_named_like_the_phone_are_offered_even_alone() {
        // Maybe an old phone, still paired: its calls are not this one's.
        let mut headset = device(C, "Headphones");
        headset.phone = false;
        headset.hands_free_gateway = false;
        let one = [device(A, "Galaxy"), headset.clone()];
        assert_eq!(
            find_phone(&one, "Pixel 8", None, &[]),
            PhoneDevice::Choose(vec![address(A)])
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
    fn a_device_that_is_not_trusted_is_never_taken_without_asking() {
        // Paired with no prompt, as "just works" pairing allows, and named
        // like the phone: offered, not taken.
        let mut stranger = device(A, "Pixel 8");
        stranger.trusted = false;
        assert_eq!(
            find_phone(std::slice::from_ref(&stranger), "Pixel 8", None, &[]),
            PhoneDevice::Choose(vec![address(A)])
        );
        // Next to the trusted phone of the same name, the phone is taken.
        assert_eq!(
            find_phone(&[stranger, device(B, "Pixel 8")], "Pixel 8", None, &[]),
            PhoneDevice::Found(address(B))
        );
    }

    #[test]
    fn a_kept_device_that_is_not_trusted_counts_only_once_the_user_picked_it() {
        let mut stranger = device(A, "Pixel 8");
        stranger.trusted = false;
        let devices = [stranger];
        // Kept from an earlier run: the user is asked again.
        assert_eq!(
            find_phone(&devices, "Pixel 8", kept(A), &[]),
            PhoneDevice::Choose(vec![address(A)])
        );
        // Picked (or paired with a confirmed code) since the desktop
        // started: the phone's, until the Bluetooth stack trusts it.
        let picked = Chosen {
            address: address(A),
            confirmed: true,
        };
        assert_eq!(
            find_phone(&devices, "Pixel 8", Some(picked), &[]),
            PhoneDevice::Found(address(A))
        );
    }

    #[test]
    fn a_device_that_only_offers_calls_counts_as_a_phone() {
        let mut gateway = device(A, "Pixel 8");
        gateway.phone = false;
        assert_eq!(
            find_phone(&[gateway], "Pixel 8", None, &[]),
            PhoneDevice::Found(address(A))
        );
    }
}
