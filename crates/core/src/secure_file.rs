//! Files only the user may read: the desktop's private key, when no keyring
//! holds it, and its pairing records.
//!
//! Writes are atomic: the content goes to a new file in the same directory,
//! created with mode 0600, is flushed to disk, and then renamed over the
//! target. A crash leaves either the old or the new file, never a mix.

use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;

use tracing::warn;

/// Mode of the files written here: read and write for the owner only.
const FILE_MODE: u32 = 0o600;
/// Mode of directories created on the way to such a file.
const DIR_MODE: u32 = 0o700;

/// Replaces the file at `path` with `contents`, atomically and with mode
/// 0600. Missing parent directories are created with mode 0700.
pub(crate) fn write(path: &Path, contents: &[u8]) -> io::Result<()> {
    let dir = match path.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir,
        _ => Path::new("."),
    };
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "the path has no file name"))?;
    DirBuilder::new()
        .recursive(true)
        .mode(DIR_MODE)
        .create(dir)?;

    let mut suffix = [0u8; 8];
    aws_lc_rs::rand::fill(&mut suffix)
        .map_err(|_| io::Error::other("the system random generator failed"))?;
    let suffix: String = suffix.iter().map(|byte| format!("{byte:02x}")).collect();
    let temp = dir.join(format!(".{}.{suffix}.tmp", name.to_string_lossy()));

    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(FILE_MODE)
            .open(&temp)?;
        file.write_all(contents)?;
        file.sync_all()?;
        fs::rename(&temp, path)?;
        // Make the rename itself durable.
        File::open(dir)?.sync_all()
    })();
    if result.is_err() {
        // Best effort: the temporary file may not exist or may be gone.
        let _ = fs::remove_file(&temp);
    }
    result
}

/// Reads the file at `path`, or returns `None` if it does not exist.
///
/// A file that others may read or write is tightened to mode 0600 first,
/// with a warning, since it holds secrets.
pub(crate) fn read(path: &Path) -> io::Result<Option<Vec<u8>>> {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if metadata.permissions().mode() & 0o077 != 0 {
        warn!(path = %path.display(), "restricting a secret file to its owner");
        fs::set_permissions(path, fs::Permissions::from_mode(FILE_MODE))?;
    }
    fs::read(path).map(Some)
}

/// Overwrites the file at `path` with zeros and deletes it. A missing file
/// is not an error.
///
/// This is best effort: a copy-on-write file system, a journal or an SSD
/// may still keep the old content somewhere on the disk.
pub(crate) fn erase(path: &Path) -> io::Result<()> {
    let mut file = match OpenOptions::new().write(true).open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    let length = usize::try_from(file.metadata()?.len()).unwrap_or(usize::MAX);
    file.write_all(&vec![0; length])?;
    file.sync_all()?;
    drop(file);
    fs::remove_file(path)?;
    if let Some(dir) = path.parent().filter(|dir| !dir.as_os_str().is_empty()) {
        File::open(dir)?.sync_all()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mode(path: &Path) -> u32 {
        fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn writes_owner_only_files_and_replaces_them() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/state.json");

        write(&path, b"one").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"one");
        assert_eq!(mode(&path), 0o600);
        assert_eq!(mode(path.parent().unwrap()), 0o700);

        write(&path, b"two").unwrap();
        assert_eq!(read(&path).unwrap().unwrap(), b"two");
        // No temporary files are left behind.
        assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
    }

    #[test]
    fn reading_tightens_loose_permissions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("key.pem");
        fs::write(&path, b"secret").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();

        assert_eq!(read(&path).unwrap().unwrap(), b"secret");
        assert_eq!(mode(&path), 0o600);
        assert_eq!(read(&dir.path().join("missing")).unwrap(), None);
    }

    #[test]
    fn erasing_removes_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("key.pem");
        write(&path, b"secret").unwrap();

        erase(&path).unwrap();
        assert!(!path.exists());
        // Erasing what is already gone is fine.
        erase(&path).unwrap();
    }
}
