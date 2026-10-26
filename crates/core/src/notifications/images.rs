//! The images that notifications refer to, kept by content
//! (`docs/protocol.md`, "Images").
//!
//! A notification carries an `ImageRef`, the SHA-256 of an image; the
//! desktop asks the phone for the bytes with an `ImageRequest` once, checks
//! that they hash to what was asked for, and keeps them: as files, which
//! the desktop's notification service can show, or only in memory. Either
//! way the cache stays under a byte budget by dropping the images used
//! longest ago.
//!
//! Before an image is kept, its header must show the format it was sent as
//! and a size the phone may send. The images reach decoders outside this
//! app, in the window's web view and in the desktop's notification
//! service, which pick a decoder by sniffing the bytes, so nothing but a
//! plain PNG, JPEG or WebP of a sensible size gets that far.

use std::collections::HashMap;
use std::fs::{self, DirBuilder};
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use aws_lc_rs::digest;
use link2linux_protocol::v1::{ImageData, ImageFormat, ImageRequest};

/// The largest image the phone may send, in bytes.
pub const MAX_IMAGE_BYTES: usize = 1024 * 1024;

/// The longest side, in pixels, of an image the phone may send: pictures
/// are at most 1024 pixels on the longer side, icons less.
pub const MAX_IMAGE_EDGE: u32 = 1024;

/// Why an image was not stored.
#[derive(Debug, thiserror::Error)]
pub enum ImageError {
    /// The bytes do not hash to the SHA-256 they came with.
    #[error("the image does not match its SHA-256")]
    Mismatch,
    /// More than [`MAX_IMAGE_BYTES`] or [`MAX_IMAGE_EDGE`], an unknown
    /// format, or bytes that are not the format they came as.
    #[error("the image is too large or not in the format it came as")]
    Invalid,
    /// The cache directory could not be written.
    #[error(transparent)]
    Io(#[from] io::Error),
}

/// An image read back from the cache.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedImage {
    /// The encoded image.
    pub data: Vec<u8>,
    /// Its format as a file extension: `png`, `jpg` or `webp`.
    pub extension: &'static str,
}

/// Images named by their SHA-256. Copies of a cache share its images.
#[derive(Debug, Clone)]
pub struct ImageCache {
    store: Store,
    max_bytes: u64,
}

#[derive(Debug, Clone)]
enum Store {
    /// One file per image in this directory.
    Files(PathBuf),
    /// Only in memory, so they never reach a disk.
    Memory(Arc<Mutex<Memory>>),
}

/// The images of a cache in memory, by hex SHA-256.
#[derive(Debug, Default)]
struct Memory {
    images: HashMap<String, Kept>,
    /// Counts uses, to tell which image was used longest ago.
    clock: u64,
}

#[derive(Debug)]
struct Kept {
    image: CachedImage,
    used: u64,
}

const EXTENSIONS: [&str; 3] = ["png", "jpg", "webp"];

impl ImageCache {
    /// A cache of files in `dir`, created with mode 0700 when missing,
    /// that keeps at most `max_bytes` of images.
    pub fn new(dir: impl Into<PathBuf>, max_bytes: u64) -> io::Result<Self> {
        let dir = dir.into();
        DirBuilder::new().recursive(true).mode(0o700).create(&dir)?;
        Ok(Self {
            store: Store::Files(dir),
            max_bytes,
        })
    }

    /// A cache that keeps at most `max_bytes` of images in memory only.
    /// It has no files, so desktop banners go without its images.
    #[must_use]
    pub fn in_memory(max_bytes: u64) -> Self {
        Self {
            store: Store::Memory(Arc::default()),
            max_bytes,
        }
    }

    /// The file of the image with this SHA-256, if cached as a file, for
    /// the desktop's notification service to show. Marks it as used, so it
    /// is removed last.
    #[must_use]
    pub fn get(&self, sha256: &[u8]) -> Option<PathBuf> {
        let Store::Files(dir) = &self.store else {
            return None;
        };
        let path = file_of(dir, sha256)?;
        // Touching the file records the use for eviction; failing to is harmless.
        let _ = fs::File::options()
            .append(true)
            .open(&path)
            .and_then(|file| file.set_modified(std::time::SystemTime::now()));
        Some(path)
    }

    /// Whether the image with this SHA-256 is cached, as a file or not.
    #[must_use]
    pub fn contains(&self, sha256: &[u8]) -> bool {
        match &self.store {
            Store::Files(dir) => file_of(dir, sha256).is_some(),
            Store::Memory(memory) => {
                hex(sha256).is_some_and(|stem| lock(memory).images.contains_key(&stem))
            }
        }
    }

    /// The image with this SHA-256, if cached, for the app's own window.
    #[must_use]
    pub fn read(&self, sha256: &[u8]) -> Option<CachedImage> {
        match &self.store {
            Store::Files(dir) => {
                let path = file_of(dir, sha256)?;
                let extension = EXTENSIONS
                    .into_iter()
                    .find(|ext| path.extension().is_some_and(|e| e == *ext))?;
                let data = fs::read(&path).ok()?;
                Some(CachedImage { data, extension })
            }
            Store::Memory(memory) => {
                let mut memory = lock(memory);
                memory.clock += 1;
                let clock = memory.clock;
                let kept = memory.images.get_mut(&hex(sha256)?)?;
                kept.used = clock;
                Some(kept.image.clone())
            }
        }
    }

    /// The request that fetches an image that is not cached.
    #[must_use]
    pub fn request(sha256: &[u8]) -> ImageRequest {
        ImageRequest {
            sha256: sha256.to_vec(),
        }
    }

    /// Checks `image` against its SHA-256 and stores it, then trims the
    /// cache to its budget.
    pub fn store(&self, image: &ImageData) -> Result<(), ImageError> {
        let extension = match image.format() {
            ImageFormat::Png => "png",
            ImageFormat::Jpeg => "jpg",
            ImageFormat::Webp => "webp",
            ImageFormat::Unspecified => return Err(ImageError::Invalid),
        };
        if image.data.len() > MAX_IMAGE_BYTES {
            return Err(ImageError::Invalid);
        }
        let (width, height) = dimensions(image.format(), &image.data).ok_or(ImageError::Invalid)?;
        if width == 0 || height == 0 || width.max(height) > MAX_IMAGE_EDGE {
            return Err(ImageError::Invalid);
        }
        let stem = hex(&image.sha256).ok_or(ImageError::Mismatch)?;
        if digest::digest(&digest::SHA256, &image.data).as_ref() != image.sha256.as_slice() {
            return Err(ImageError::Mismatch);
        }
        match &self.store {
            Store::Files(dir) => {
                let path = dir.join(format!("{stem}.{extension}"));
                if !path.is_file() {
                    // Write next to the final name and rename, so a reader
                    // never sees half an image.
                    // Readable by the user only, like the directory: the
                    // pictures can be contact photos.
                    let partial = dir.join(format!("{stem}.{extension}.partial"));
                    let _ = fs::remove_file(&partial);
                    fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .mode(0o600)
                        .open(&partial)?
                        .write_all(&image.data)?;
                    fs::rename(&partial, &path)?;
                }
                trim_files(dir, &path, self.max_bytes)?;
            }
            Store::Memory(memory) => {
                let mut memory = lock(memory);
                memory.clock += 1;
                let used = memory.clock;
                let image = CachedImage {
                    data: image.data.clone(),
                    extension,
                };
                memory.images.insert(stem.clone(), Kept { image, used });
                memory.trim(&stem, self.max_bytes);
            }
        }
        Ok(())
    }

    /// Drops every image, and the directory of a cache of files.
    pub fn clear(&self) -> io::Result<()> {
        match &self.store {
            Store::Files(dir) => match fs::remove_dir_all(dir) {
                Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
                _ => Ok(()),
            },
            Store::Memory(memory) => {
                lock(memory).images.clear();
                Ok(())
            }
        }
    }
}

impl Memory {
    /// Drops the images used longest ago until the rest fit in
    /// `max_bytes`, keeping `keep`.
    fn trim(&mut self, keep: &str, max_bytes: u64) {
        let mut total: u64 = self
            .images
            .values()
            .map(|kept| kept.image.data.len() as u64)
            .sum();
        let mut by_use: Vec<(u64, String)> = self
            .images
            .iter()
            .map(|(stem, kept)| (kept.used, stem.clone()))
            .collect();
        by_use.sort();
        for (_, stem) in by_use {
            if total <= max_bytes {
                break;
            }
            if stem != keep
                && let Some(kept) = self.images.remove(&stem)
            {
                total -= kept.image.data.len() as u64;
            }
        }
    }
}

/// The width and height in the header of `data`, if it is an image of
/// `format`. Only the header is read; nothing is decoded.
fn dimensions(format: ImageFormat, data: &[u8]) -> Option<(u32, u32)> {
    match format {
        ImageFormat::Png => png_size(data),
        ImageFormat::Jpeg => jpeg_size(data),
        ImageFormat::Webp => webp_size(data),
        ImageFormat::Unspecified => None,
    }
}

/// A PNG starts with its signature and then the IHDR chunk, which holds
/// the width and height.
fn png_size(data: &[u8]) -> Option<(u32, u32)> {
    let rest = data.strip_prefix(b"\x89PNG\r\n\x1a\n")?;
    if rest.get(..8)? != b"\0\0\0\x0dIHDR" {
        return None;
    }
    Some((be32(rest.get(8..12)?), be32(rest.get(12..16)?)))
}

/// A JPEG's size is in its frame header (a SOFn segment), which comes
/// after the start of image and any number of other segments.
fn jpeg_size(data: &[u8]) -> Option<(u32, u32)> {
    if data.get(..2)? != [0xFF, 0xD8] {
        return None;
    }
    let mut at = 2;
    loop {
        if *data.get(at)? != 0xFF {
            return None;
        }
        // A marker may follow any number of fill bytes.
        while *data.get(at)? == 0xFF {
            at += 1;
        }
        let marker = *data.get(at)?;
        // Every segment before the frame header has a length, which counts
        // itself; the end of the image or a scan here means there is none.
        if matches!(marker, 0x00 | 0x01 | 0xD0..=0xDA) {
            return None;
        }
        let length = usize::from(be16(data.get(at + 1..at + 3)?));
        if length < 2 {
            return None;
        }
        // SOF0 to SOF15, apart from DHT, JPG and DAC, which share the range.
        if matches!(marker, 0xC0..=0xCF) && !matches!(marker, 0xC4 | 0xC8 | 0xCC) {
            let height = be16(data.get(at + 4..at + 6)?);
            let width = be16(data.get(at + 6..at + 8)?);
            return Some((u32::from(width), u32::from(height)));
        }
        at += 1 + length;
    }
}

/// A WebP is a RIFF file whose first chunk is a lossy (`VP8 `), lossless
/// (`VP8L`) or extended (`VP8X`) header, each with the size in its own
/// layout.
fn webp_size(data: &[u8]) -> Option<(u32, u32)> {
    if data.get(..4)? != b"RIFF" || data.get(8..12)? != b"WEBP" {
        return None;
    }
    let chunk = data.get(20..)?;
    match data.get(12..16)? {
        b"VP8 " => {
            if chunk.get(3..6)? != [0x9D, 0x01, 0x2A] {
                return None;
            }
            let width = u32::from(le16(chunk.get(6..8)?) & 0x3FFF);
            let height = u32::from(le16(chunk.get(8..10)?) & 0x3FFF);
            Some((width, height))
        }
        b"VP8L" => {
            if *chunk.first()? != 0x2F {
                return None;
            }
            let bits = u32::from_le_bytes(chunk.get(1..5)?.try_into().ok()?);
            Some(((bits & 0x3FFF) + 1, ((bits >> 14) & 0x3FFF) + 1))
        }
        b"VP8X" => {
            let width = le24(chunk.get(4..7)?) + 1;
            let height = le24(chunk.get(7..10)?) + 1;
            Some((width, height))
        }
        _ => None,
    }
}

fn be32(bytes: &[u8]) -> u32 {
    u32::from_be_bytes(bytes.try_into().expect("four bytes"))
}

fn be16(bytes: &[u8]) -> u16 {
    u16::from_be_bytes(bytes.try_into().expect("two bytes"))
}

fn le16(bytes: &[u8]) -> u16 {
    u16::from_le_bytes(bytes.try_into().expect("two bytes"))
}

fn le24(bytes: &[u8]) -> u32 {
    u32::from(bytes[0]) | u32::from(bytes[1]) << 8 | u32::from(bytes[2]) << 16
}

/// The file of the image with this SHA-256 in `dir`, if there is one.
fn file_of(dir: &Path, sha256: &[u8]) -> Option<PathBuf> {
    let stem = hex(sha256)?;
    EXTENSIONS
        .into_iter()
        .map(|ext| dir.join(format!("{stem}.{ext}")))
        .find(|path| path.is_file())
}

/// Removes the files used longest ago until the directory fits its
/// budget, keeping `keep`.
fn trim_files(dir: &Path, keep: &Path, max_bytes: u64) -> io::Result<()> {
    let mut files = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let meta = entry.metadata()?;
        if meta.is_file() {
            files.push((meta.modified()?, meta.len(), entry.path()));
        }
    }
    let mut total: u64 = files.iter().map(|(_, len, _)| len).sum();
    files.sort();
    for (_, len, path) in files {
        if total <= max_bytes {
            break;
        }
        if path != keep {
            fs::remove_file(&path)?;
            total -= len;
        }
    }
    Ok(())
}

fn lock(memory: &Mutex<Memory>) -> std::sync::MutexGuard<'_, Memory> {
    memory
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// A SHA-256 as 64 lowercase hex digits, or `None` when it is not 32 bytes.
fn hex(sha256: &[u8]) -> Option<String> {
    (sha256.len() == 32).then(|| sha256.iter().map(|b| format!("{b:02x}")).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::time::{Duration, SystemTime};

    /// A fresh directory under the system's temporary directory, removed
    /// when dropped.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let name = format!("l2l-images-{}-{:x}", std::process::id(), rand_u64());
            Self(std::env::temp_dir().join(name))
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn rand_u64() -> u64 {
        use std::hash::{BuildHasher, RandomState};
        RandomState::new().hash_one(SystemTime::now())
    }

    fn image(bytes: &[u8]) -> ImageData {
        image_as(ImageFormat::Png, bytes)
    }

    fn image_as(format: ImageFormat, bytes: &[u8]) -> ImageData {
        ImageData {
            sha256: digest::digest(&digest::SHA256, bytes).as_ref().to_vec(),
            format: format.into(),
            data: bytes.to_vec(),
        }
    }

    /// A PNG header of `width` by `height`, then `rest`. 33 bytes plus
    /// `rest`; the stored bytes are never decoded, only their header read.
    fn png_of(width: u32, height: u32, rest: &[u8]) -> Vec<u8> {
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
        png.extend(width.to_be_bytes());
        png.extend(height.to_be_bytes());
        png.extend([8, 6, 0, 0, 0, 0, 0, 0, 0]);
        png.extend(rest);
        png
    }

    fn png(rest: &[u8]) -> Vec<u8> {
        png_of(128, 128, rest)
    }

    #[test]
    fn stores_a_matching_image_once_and_finds_it_again() {
        let dir = TempDir::new();
        let cache = ImageCache::new(&dir.0, 1 << 20).unwrap();
        let bytes = png(b"png bytes");
        let icon = image(&bytes);
        assert_eq!(cache.get(&icon.sha256), None);
        assert!(!cache.contains(&icon.sha256));
        cache.store(&icon).unwrap();
        let path = cache.get(&icon.sha256).unwrap();
        assert_eq!(fs::read(&path).unwrap(), bytes);
        assert!(path.to_string_lossy().ends_with(".png"));
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600,
            "only the user can read it"
        );
        assert!(cache.contains(&icon.sha256));
        assert_eq!(
            cache.read(&icon.sha256),
            Some(CachedImage {
                data: bytes.clone(),
                extension: "png"
            })
        );
        cache.store(&icon).unwrap();
        assert_eq!(
            fs::read_dir(&dir.0).unwrap().count(),
            1,
            "storing again is a no-op"
        );
        assert_eq!(ImageCache::request(&icon.sha256).sha256, icon.sha256);
    }

    #[test]
    fn refuses_bytes_that_do_not_match_or_are_invalid() {
        let dir = TempDir::new();
        let cache = ImageCache::new(&dir.0, 1 << 20).unwrap();
        let mut forged = image(&png(b"real"));
        forged.data = png(b"forged");
        assert!(matches!(cache.store(&forged), Err(ImageError::Mismatch)));
        let mut short = image(&png(b"x"));
        short.sha256.truncate(31);
        assert!(matches!(cache.store(&short), Err(ImageError::Mismatch)));
        let mut unknown = image(&png(b"x"));
        unknown.format = ImageFormat::Unspecified.into();
        assert!(matches!(cache.store(&unknown), Err(ImageError::Invalid)));
        let huge = image(&vec![0; MAX_IMAGE_BYTES + 1]);
        assert!(matches!(cache.store(&huge), Err(ImageError::Invalid)));
        assert_eq!(
            fs::read_dir(&dir.0).unwrap().count(),
            0,
            "nothing was written"
        );
    }

    #[test]
    fn refuses_bytes_that_are_not_the_image_they_claim_to_be() {
        let dir = TempDir::new();
        let cache = ImageCache::new(&dir.0, 1 << 20).unwrap();
        let refused = |format, bytes: &[u8]| {
            matches!(
                cache.store(&image_as(format, bytes)),
                Err(ImageError::Invalid)
            )
        };
        let jpeg = |width: u16, height: u16| {
            let mut jpeg = vec![
                0xFF, 0xD8, 0xFF, 0xE0, 0, 4, 0, 0, 0xFF, 0xFF, 0xC2, 0, 11, 8,
            ];
            jpeg.extend(height.to_be_bytes());
            jpeg.extend(width.to_be_bytes());
            jpeg.extend([1, 1, 0x11, 0]);
            jpeg
        };
        let webp = |kind: &[u8], chunk: &[u8]| {
            let mut webp = b"RIFF\0\0\0\0WEBP".to_vec();
            webp.extend(kind);
            webp.extend([0; 4]);
            webp.extend(chunk);
            webp
        };
        assert!(refused(ImageFormat::Png, b"GIF89a\x01\0\x01\0"), "a GIF");
        assert!(refused(ImageFormat::Png, b"II*\0\x08\0\0\0"), "a TIFF");
        assert!(refused(ImageFormat::Png, &jpeg(64, 64)), "a JPEG");
        assert!(refused(ImageFormat::Jpeg, &png(b"")), "a PNG");
        assert!(refused(ImageFormat::Png, &png_of(20_000, 20_000, b"")));
        assert!(refused(ImageFormat::Png, &png_of(0, 16, b"")));
        assert!(refused(ImageFormat::Jpeg, &jpeg(1025, 10)));
        assert!(refused(ImageFormat::Jpeg, &[0xFF, 0xD8, 0xFF, 0xDA, 0, 2]));
        assert!(refused(
            ImageFormat::Webp,
            &webp(b"VP8X", &[0, 0, 0, 0, 0xFF, 0x07, 0, 0, 1, 0])
        ));
        assert!(
            refused(ImageFormat::Png, &png(b"")[..20]),
            "a cut-off header"
        );
        assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 0);

        cache.store(&image(&png_of(1024, 1, b""))).unwrap();
        cache
            .store(&image_as(ImageFormat::Jpeg, &jpeg(1024, 768)))
            .unwrap();
        // 1024 by 768, extended; 1 by 1, lossless; 640 by 480, lossy.
        let accepted = [
            webp(b"VP8X", &[0, 0, 0, 0, 0xFF, 0x03, 0, 0xFF, 0x02, 0]),
            webp(b"VP8L", &[0x2F, 0, 0, 0, 0]),
            webp(
                b"VP8 ",
                &[0, 0, 0, 0x9D, 0x01, 0x2A, 0x80, 0x02, 0xE0, 0x01],
            ),
        ];
        for bytes in accepted {
            cache.store(&image_as(ImageFormat::Webp, &bytes)).unwrap();
        }
        assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 5);
    }

    #[test]
    fn keeps_to_its_budget_by_removing_the_least_recently_used() {
        let dir = TempDir::new();
        // Room for two images of 43 bytes, not three.
        let cache = ImageCache::new(&dir.0, 100).unwrap();
        let a = image(&png(&[b'a'; 10]));
        let b = image(&png(&[b'b'; 10]));
        let c = image(&png(&[b'c'; 10]));
        cache.store(&a).unwrap();
        cache.store(&b).unwrap();
        let path_a = cache.get(&a.sha256).unwrap();
        let path_b = cache.get(&b.sha256).unwrap();
        // Make the order of use certain: a was used before b.
        let old = SystemTime::now() - Duration::from_secs(60);
        fs::File::options()
            .append(true)
            .open(&path_a)
            .unwrap()
            .set_modified(old)
            .unwrap();
        fs::File::options()
            .append(true)
            .open(&path_b)
            .unwrap()
            .set_modified(old + Duration::from_secs(1))
            .unwrap();
        assert!(
            cache.get(&b.sha256).is_some(),
            "using b makes it the newest"
        );

        cache.store(&c).unwrap();
        assert_eq!(cache.get(&a.sha256), None, "a, used longest ago, is gone");
        assert!(cache.get(&b.sha256).is_some() && cache.get(&c.sha256).is_some());

        cache.clear().unwrap();
        assert!(!dir.0.exists());
    }

    #[test]
    fn a_cache_in_memory_writes_no_files_and_keeps_to_its_budget() {
        let cache = ImageCache::in_memory(100);
        let shared = cache.clone();
        let a = image(&png(&[b'a'; 10]));
        let b = image(&png(&[b'b'; 10]));
        let c = image(&png(&[b'c'; 10]));
        cache.store(&a).unwrap();
        cache.store(&b).unwrap();
        assert!(shared.contains(&a.sha256), "copies share the images");
        assert_eq!(cache.get(&a.sha256), None, "there is no file to show");
        assert_eq!(cache.read(&a.sha256).unwrap().data, a.data);

        // Reading a made it newer than b, so b goes first.
        cache.store(&c).unwrap();
        assert!(!cache.contains(&b.sha256));
        assert!(cache.contains(&a.sha256) && cache.contains(&c.sha256));

        let mut forged = image(&png(b"real"));
        forged.data = png(b"forged");
        assert!(matches!(cache.store(&forged), Err(ImageError::Mismatch)));

        cache.clear().unwrap();
        assert!(!shared.contains(&a.sha256));
    }
}
