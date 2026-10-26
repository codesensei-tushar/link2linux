//! The images that notifications refer to, kept by content
//! (`docs/protocol.md`, "Images").
//!
//! A notification carries an `ImageRef`, the SHA-256 of an image; the
//! desktop asks the phone for the bytes with an `ImageRequest` once, checks
//! that they hash to what was asked for, and keeps them: as files, which
//! the desktop's notification service can show, or only in memory. Either
//! way the cache stays under a byte budget by dropping the images used
//! longest ago.

use std::collections::HashMap;
use std::fs::{self, DirBuilder};
use std::io::{self, Write};
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use aws_lc_rs::digest;
use link2linux_protocol::v1::{ImageData, ImageFormat, ImageRequest};

/// The largest image the phone may send, in bytes.
pub const MAX_IMAGE_BYTES: usize = 1024 * 1024;

/// Why an image was not stored.
#[derive(Debug, thiserror::Error)]
pub enum ImageError {
    /// The bytes do not hash to the SHA-256 they came with.
    #[error("the image does not match its SHA-256")]
    Mismatch,
    /// More than [`MAX_IMAGE_BYTES`], or an unknown format.
    #[error("the image is too large or has an unknown format")]
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
                    let partial = dir.join(format!("{stem}.{extension}.partial"));
                    fs::File::create(&partial)?.write_all(&image.data)?;
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
        ImageData {
            sha256: digest::digest(&digest::SHA256, bytes).as_ref().to_vec(),
            format: ImageFormat::Png.into(),
            data: bytes.to_vec(),
        }
    }

    #[test]
    fn stores_a_matching_image_once_and_finds_it_again() {
        let dir = TempDir::new();
        let cache = ImageCache::new(&dir.0, 1 << 20).unwrap();
        let icon = image(b"png bytes");
        assert_eq!(cache.get(&icon.sha256), None);
        assert!(!cache.contains(&icon.sha256));
        cache.store(&icon).unwrap();
        let path = cache.get(&icon.sha256).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"png bytes");
        assert!(path.to_string_lossy().ends_with(".png"));
        assert!(cache.contains(&icon.sha256));
        assert_eq!(
            cache.read(&icon.sha256),
            Some(CachedImage {
                data: b"png bytes".to_vec(),
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
        let mut forged = image(b"real");
        forged.data = b"forged".to_vec();
        assert!(matches!(cache.store(&forged), Err(ImageError::Mismatch)));
        let mut short = image(b"x");
        short.sha256.truncate(31);
        assert!(matches!(cache.store(&short), Err(ImageError::Mismatch)));
        let mut unknown = image(b"x");
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
    fn keeps_to_its_budget_by_removing_the_least_recently_used() {
        let dir = TempDir::new();
        let cache = ImageCache::new(&dir.0, 25).unwrap();
        let a = image(&[b'a'; 10]);
        let b = image(&[b'b'; 10]);
        let c = image(&[b'c'; 10]);
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
        let cache = ImageCache::in_memory(25);
        let shared = cache.clone();
        let a = image(&[b'a'; 10]);
        let b = image(&[b'b'; 10]);
        let c = image(&[b'c'; 10]);
        cache.store(&a).unwrap();
        cache.store(&b).unwrap();
        assert!(shared.contains(&a.sha256), "copies share the images");
        assert_eq!(cache.get(&a.sha256), None, "there is no file to show");
        assert_eq!(cache.read(&a.sha256).unwrap().data, a.data);

        // Reading a made it newer than b, so b goes first.
        cache.store(&c).unwrap();
        assert!(!cache.contains(&b.sha256));
        assert!(cache.contains(&a.sha256) && cache.contains(&c.sha256));

        let mut forged = image(b"real");
        forged.data = b"forged".to_vec();
        assert!(matches!(cache.store(&forged), Err(ImageError::Mismatch)));

        cache.clear().unwrap();
        assert!(!shared.contains(&a.sha256));
    }
}
