//! The images that notifications refer to, kept on disk by content
//! (`docs/protocol.md`, "Images").
//!
//! A notification carries an `ImageRef`, the SHA-256 of an image; the
//! desktop asks the phone for the bytes with an `ImageRequest` once, checks
//! that they hash to what was asked for, and keeps them as a file, which
//! the desktop's notification service can show. The cache stays under a
//! byte budget by removing the files used longest ago.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

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

/// Images on disk, named by their SHA-256.
#[derive(Debug, Clone)]
pub struct ImageCache {
    dir: PathBuf,
    max_bytes: u64,
}

impl ImageCache {
    /// A cache in `dir`, created when missing, that keeps at most
    /// `max_bytes` of images.
    pub fn new(dir: impl Into<PathBuf>, max_bytes: u64) -> io::Result<Self> {
        let dir = dir.into();
        fs::create_dir_all(&dir)?;
        Ok(Self { dir, max_bytes })
    }

    /// The file of the image with this SHA-256, if cached. Marks it as
    /// used, so it is removed last.
    #[must_use]
    pub fn get(&self, sha256: &[u8]) -> Option<PathBuf> {
        let stem = hex(sha256)?;
        let path = ["png", "jpg", "webp"]
            .into_iter()
            .map(|ext| self.dir.join(format!("{stem}.{ext}")))
            .find(|path| path.is_file())?;
        // Touching the file records the use for eviction; failing to is harmless.
        let _ = fs::File::options()
            .append(true)
            .open(&path)
            .and_then(|file| file.set_modified(std::time::SystemTime::now()));
        Some(path)
    }

    /// The request that fetches an image that is not cached.
    #[must_use]
    pub fn request(sha256: &[u8]) -> ImageRequest {
        ImageRequest {
            sha256: sha256.to_vec(),
        }
    }

    /// Checks `image` against its SHA-256 and stores it, then trims the
    /// cache to its budget. Returns the image's file.
    pub fn store(&self, image: &ImageData) -> Result<PathBuf, ImageError> {
        let ext = match image.format() {
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
        let path = self.dir.join(format!("{stem}.{ext}"));
        if !path.is_file() {
            // Write next to the final name and rename, so a reader never
            // sees half an image.
            let partial = self.dir.join(format!("{stem}.{ext}.partial"));
            fs::File::create(&partial)?.write_all(&image.data)?;
            fs::rename(&partial, &path)?;
        }
        self.trim(&path)?;
        Ok(path)
    }

    /// Removes the files used longest ago until the cache fits its budget,
    /// keeping `keep`.
    fn trim(&self, keep: &Path) -> io::Result<()> {
        let mut files = Vec::new();
        for entry in fs::read_dir(&self.dir)? {
            let entry = entry?;
            let meta = entry.metadata()?;
            if meta.is_file() {
                files.push((meta.modified()?, meta.len(), entry.path()));
            }
        }
        let mut total: u64 = files.iter().map(|(_, len, _)| len).sum();
        files.sort();
        for (_, len, path) in files {
            if total <= self.max_bytes {
                break;
            }
            if path != keep {
                fs::remove_file(&path)?;
                total -= len;
            }
        }
        Ok(())
    }
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
        let path = cache.store(&icon).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"png bytes");
        assert!(path.to_string_lossy().ends_with(".png"));
        assert_eq!(cache.get(&icon.sha256), Some(path.clone()));
        assert_eq!(
            cache.store(&icon).unwrap(),
            path,
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
        let path_a = cache.store(&a).unwrap();
        let path_b = cache.store(&b).unwrap();
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
    }
}
