//! Pretend photos for the fake backend: a connected phone lists the same
//! recent photos, their thumbnails are small gradients drawn here, and
//! saving one only pretends.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use crate::backend::{
    BackendError, Device, DeviceStatus, PhonePhotos, Photo, PhotosStatus, SavedPhoto,
};

/// How many photos a pretend phone has.
const PHOTO_COUNT: u32 = 30;
/// The longest side of a pretend thumbnail. The page scales it up, and a
/// gradient stays smooth when it does, so a few KiB are enough.
const THUMBNAIL_SIDE: u32 = 48;
/// The colours, top to bottom, the pretend photos are drawn with.
const GRADIENTS: [([u8; 3], [u8; 3]); 6] = [
    ([0x6c, 0xa6, 0xd9], [0xe8, 0xd5, 0xa9]),
    ([0x2f, 0x6b, 0x3a], [0x9c, 0xc5, 0x7a]),
    ([0xf2, 0x8c, 0x48], [0x5b, 0x2a, 0x6e]),
    ([0x1d, 0x2b, 0x4f], [0x7f, 0x9c, 0xc9]),
    ([0xd9, 0xd4, 0xcc], [0x8a, 0x7f, 0x73]),
    ([0xf4, 0xc2, 0xc2], [0xb0, 0x3a, 0x48]),
];
/// The colours of a pretend screenshot: a light screen.
const SCREENSHOT: ([u8; 3], [u8; 3]) = ([0xf5, 0xf5, 0xf7], [0xd5, 0xdb, 0xe5]);

/// Each phone's pretend photos and what the user saved of them.
pub struct FakePhotos {
    photos: Vec<Photo>,
    saved: Mutex<Saved>,
}

#[derive(Default)]
struct Saved {
    /// The name each saved photo got, by phone and photo id.
    by_photo: HashMap<(String, String), String>,
    /// Every name given out, since all phones save into the same folder.
    names: HashSet<String>,
}

impl FakePhotos {
    /// Photos whose newest was taken at `newest_unix_ms`.
    pub fn new(newest_unix_ms: i64) -> Self {
        Self {
            photos: demo_photos(newest_unix_ms),
            saved: Mutex::default(),
        }
    }

    /// The photos of `device`, which is `None` when it is not paired.
    pub fn list(&self, device: Option<&Device>) -> Result<PhonePhotos, BackendError> {
        let device = device.ok_or(BackendError::NotFound)?;
        Ok(if device.status == DeviceStatus::Connected {
            PhonePhotos {
                state: PhotosStatus::Listed,
                photos: self.photos.clone(),
            }
        } else {
            PhonePhotos::default()
        })
    }

    /// A thumbnail of photo `photo_id` of `device`, as a PNG.
    pub fn thumbnail(
        &self,
        device: Option<&Device>,
        photo_id: &str,
    ) -> Result<Arc<[u8]>, BackendError> {
        connected(device)?;
        let (index, photo) = self.find(photo_id)?;
        let (width, height) = fit(photo.width, photo.height, THUMBNAIL_SIDE);
        let (top, bottom) = if photo.mime_type == "image/png" {
            SCREENSHOT
        } else {
            GRADIENTS[index % GRADIENTS.len()]
        };
        let image = png(width, height, |y| {
            let along = y as f32 / height.saturating_sub(1).max(1) as f32;
            std::array::from_fn(|channel| mix(top[channel], bottom[channel], along))
        });
        Ok(image.into())
    }

    /// Pretends to save photo `photo_id` of `device`: it gets its own name,
    /// or a free one when that was given out already, as the real backend
    /// never replaces a file.
    pub fn save(
        &self,
        device: Option<&Device>,
        photo_id: &str,
    ) -> Result<SavedPhoto, BackendError> {
        let device = connected(device)?;
        let (_, photo) = self.find(photo_id)?;
        let mut saved = self.lock();
        let name = (0..)
            .map(|n| numbered(&photo.name, n))
            .find(|name| !saved.names.contains(name))
            .unwrap_or_default();
        saved.names.insert(name.clone());
        saved
            .by_photo
            .insert((device.id.clone(), photo_id.to_owned()), name.clone());
        Ok(SavedPhoto { name })
    }

    /// Succeeds for a photo the user saved: there is nothing to open.
    pub fn saved(&self, device_id: &str, photo_id: &str) -> Result<(), BackendError> {
        if self
            .lock()
            .by_photo
            .contains_key(&(device_id.to_owned(), photo_id.to_owned()))
        {
            Ok(())
        } else {
            Err(BackendError::NotFound)
        }
    }

    /// Photo `photo_id`, with its place in the list.
    fn find(&self, photo_id: &str) -> Result<(usize, &Photo), BackendError> {
        self.photos
            .iter()
            .enumerate()
            .find(|(_, photo)| photo.id == photo_id)
            .ok_or(BackendError::PhotoGone)
    }

    fn lock(&self) -> MutexGuard<'_, Saved> {
        self.saved.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// `device` when it is paired and connected, which a photo needs.
fn connected(device: Option<&Device>) -> Result<&Device, BackendError> {
    let device = device.ok_or(BackendError::NotFound)?;
    if device.status == DeviceStatus::Connected {
        Ok(device)
    } else {
        Err(BackendError::PhoneNotConnected)
    }
}

/// The pretend phone's photos, newest first: camera photos, some upright,
/// with a screenshot now and then.
fn demo_photos(newest_unix_ms: i64) -> Vec<Photo> {
    (0..PHOTO_COUNT)
        .map(|index| {
            // Taken a little under an hour apart, on and off.
            let taken =
                newest_unix_ms - i64::from(index) * 53 * 60_000 - i64::from(index % 4) * 7_000;
            let number = 4_200 - index;
            let (name, mime_type, width, height, size) = if index % 7 == 3 {
                (
                    format!("Screenshot_{number}.png"),
                    "image/png",
                    1080,
                    2400,
                    380_000 + u64::from(index) * 1_100,
                )
            } else {
                let upright = index % 5 == 1;
                (
                    format!("IMG_{number}.jpg"),
                    "image/jpeg",
                    if upright { 3072 } else { 4080 },
                    if upright { 4080 } else { 3072 },
                    2_100_000 + u64::from(index) * 37_000,
                )
            };
            Photo {
                id: (10_000 + PHOTO_COUNT - index).to_string(),
                name,
                mime_type: mime_type.to_owned(),
                size,
                taken_unix_ms: taken,
                modified_unix_ms: taken,
                width,
                height,
            }
        })
        .collect()
}

/// `width` × `height` scaled to fit in `side`, at least one pixel each.
fn fit(width: u32, height: u32, side: u32) -> (u32, u32) {
    let longest = width.max(height).max(1);
    let scale = |length: u32| {
        u32::try_from(u64::from(length) * u64::from(side) / u64::from(longest))
            .unwrap_or(side)
            .max(1)
    };
    (scale(width), scale(height))
}

/// The colour `along` the way from `from` to `to`.
fn mix(from: u8, to: u8, along: f32) -> u8 {
    let value = f32::from(from) + (f32::from(to) - f32::from(from)) * along.clamp(0.0, 1.0);
    // In 0..=255 by construction, so the cast only drops the fraction.
    value.round() as u8
}

/// `name` with ` (n)` before its extension, or `name` itself for 0, as the
/// real backend numbers a name that is taken.
fn numbered(name: &str, n: u32) -> String {
    if n == 0 {
        return name.to_owned();
    }
    match name.rsplit_once('.') {
        Some((stem, extension)) => format!("{stem} ({n}).{extension}"),
        None => format!("{name} ({n})"),
    }
}

/// An RGB PNG of `width` × `height` pixels, at least one row, whose row `y`
/// has the colour `row(y)`. The data is stored without compression, which
/// keeps this encoder short; it is meant for the few KiB of a pretend
/// thumbnail.
fn png(width: u32, height: u32, row: impl Fn(u32) -> [u8; 3]) -> Vec<u8> {
    let mut pixels = Vec::new();
    for y in 0..height {
        // Each row starts with its filter: none.
        pixels.push(0);
        let colour = row(y);
        for _ in 0..width {
            pixels.extend_from_slice(&colour);
        }
    }
    // A zlib stream of stored deflate blocks, each at most 65535 bytes.
    let mut zlib = vec![0x78, 0x01];
    let blocks: Vec<&[u8]> = pixels.chunks(usize::from(u16::MAX)).collect();
    for (index, block) in blocks.iter().enumerate() {
        let last = index + 1 == blocks.len();
        let length = u16::try_from(block.len()).unwrap_or(u16::MAX);
        zlib.push(u8::from(last));
        zlib.extend_from_slice(&length.to_le_bytes());
        zlib.extend_from_slice(&(!length).to_le_bytes());
        zlib.extend_from_slice(block);
    }
    zlib.extend_from_slice(&adler32(&pixels).to_be_bytes());

    let mut header = Vec::with_capacity(13);
    header.extend_from_slice(&width.to_be_bytes());
    header.extend_from_slice(&height.to_be_bytes());
    // 8 bits per channel, RGB, deflate, no filters beyond the per-row
    // byte, not interlaced.
    header.extend_from_slice(&[8, 2, 0, 0, 0]);

    let mut image = b"\x89PNG\r\n\x1a\n".to_vec();
    let chunks: [(&[u8; 4], &[u8]); 3] = [(b"IHDR", &header), (b"IDAT", &zlib), (b"IEND", &[])];
    for (kind, data) in chunks {
        image.extend_from_slice(&u32::try_from(data.len()).unwrap_or(0).to_be_bytes());
        let start = image.len();
        image.extend_from_slice(kind);
        image.extend_from_slice(data);
        let crc = crc32(&image[start..]);
        image.extend_from_slice(&crc.to_be_bytes());
    }
    image
}

/// The CRC-32 a PNG chunk ends with.
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xedb8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// The Adler-32 checksum a zlib stream ends with.
fn adler32(bytes: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in bytes {
        a = (a + u32::from(byte)) % 65_521;
        b = (b + a) % 65_521;
    }
    (b << 16) | a
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn a_connected_phone_lists_its_photos_newest_first() {
        let photos = FakePhotos::new(1_000_000_000);
        let listed = photos.list(Some(&device(DeviceStatus::Connected))).unwrap();
        assert_eq!(listed.state, PhotosStatus::Listed);
        assert_eq!(listed.photos.len(), 30);
        assert_eq!(listed.photos[0].taken_unix_ms, 1_000_000_000);
        assert!(
            listed
                .photos
                .windows(2)
                .all(|pair| pair[0].taken_unix_ms > pair[1].taken_unix_ms)
        );
        let ids: HashSet<_> = listed.photos.iter().map(|photo| &photo.id).collect();
        assert_eq!(ids.len(), 30);
        assert!(
            listed
                .photos
                .iter()
                .any(|photo| photo.mime_type == "image/png")
        );

        let away = photos
            .list(Some(&device(DeviceStatus::Reconnecting)))
            .unwrap();
        assert_eq!(away, PhonePhotos::default());
        assert_eq!(away.state, PhotosStatus::Disconnected);
        assert_eq!(photos.list(None), Err(BackendError::NotFound));
    }

    #[test]
    fn thumbnails_are_small_pngs_shaped_like_their_photo() {
        let photos = FakePhotos::new(0);
        let connected = device(DeviceStatus::Connected);
        let list = photos.list(Some(&connected)).unwrap().photos;
        for photo in &list {
            let image = photos.thumbnail(Some(&connected), &photo.id).unwrap();
            assert!(image.starts_with(b"\x89PNG\r\n\x1a\n"));
            assert!(image.len() < 8 * 1024, "{} bytes", image.len());
            let width = u32::from_be_bytes(image[16..20].try_into().unwrap());
            let height = u32::from_be_bytes(image[20..24].try_into().unwrap());
            assert_eq!(width.max(height), THUMBNAIL_SIDE);
            assert_eq!(width > height, photo.width > photo.height, "{}", photo.name);
            // Each chunk's checksum covers its type and data.
            let header_crc = u32::from_be_bytes(image[29..33].try_into().unwrap());
            assert_eq!(header_crc, crc32(&image[12..29]));
            assert!(image.ends_with(&[0, 0, 0, 0, b'I', b'E', b'N', b'D', 0xae, 0x42, 0x60, 0x82]));
        }

        assert_eq!(
            photos.thumbnail(Some(&connected), "nope"),
            Err(BackendError::PhotoGone)
        );
        assert_eq!(
            photos.thumbnail(Some(&device(DeviceStatus::Disconnected)), &list[0].id),
            Err(BackendError::PhoneNotConnected)
        );
        assert_eq!(
            photos.thumbnail(None, &list[0].id),
            Err(BackendError::NotFound)
        );
    }

    #[test]
    fn checksums_match_their_definitions() {
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
        assert_eq!(adler32(b"Wikipedia"), 0x11e6_0398);
        assert_eq!(fit(4080, 3072, 48), (48, 36));
        assert_eq!(fit(1, 5000, 48), (1, 48));
        assert_eq!(fit(0, 0, 48), (1, 1));
    }

    #[test]
    fn saving_again_takes_a_free_name_and_only_saved_photos_open() {
        let photos = FakePhotos::new(0);
        let connected = device(DeviceStatus::Connected);
        let first = photos.list(Some(&connected)).unwrap().photos[0].clone();
        assert_eq!(
            photos.saved("phone", &first.id),
            Err(BackendError::NotFound)
        );

        let saved = photos.save(Some(&connected), &first.id).unwrap();
        assert_eq!(saved.name, first.name);
        let again = photos.save(Some(&connected), &first.id).unwrap();
        assert_eq!(again.name, numbered(&first.name, 1));
        assert!(again.name.ends_with(" (1).jpg"), "{}", again.name);
        assert_eq!(photos.saved("phone", &first.id), Ok(()));
        assert_eq!(
            photos.saved("other", &first.id),
            Err(BackendError::NotFound)
        );

        assert_eq!(
            photos.save(Some(&device(DeviceStatus::Disconnected)), &first.id),
            Err(BackendError::PhoneNotConnected)
        );
        assert_eq!(numbered("notes", 2), "notes (2)");
    }
}
