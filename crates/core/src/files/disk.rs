//! The disk side of transfers.
//!
//! Everything here blocks, so the service runs it through [`blocking`] on
//! tokio's blocking threads, never on the threads that drive sessions.
//!
//! A received file is written under a temporary name in the directory it
//! goes to, [`temp_name`], hidden and recognisable, so that the app can
//! remove what an earlier run left behind ([`remove_leftovers`]). Once its
//! hash matches, [`publish`] gives it a free name without ever replacing a
//! file.

use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions};
use std::io::{self, ErrorKind};
use std::os::unix::fs::{FileExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use aws_lc_rs::digest;
use rustix::fs::{CWD, OFlags, RenameFlags};
use rustix::io::Errno;

use super::{TransferId, names};

const TEMP_PREFIX: &str = ".link2linux-";
const TEMP_SUFFIX: &str = ".part";

/// How many numbered names [`publish`] tries before it gives up.
const MAX_NUMBER: u32 = 1000;

/// How much [`hash_prefix`] reads at a time.
const HASH_READ_LEN: usize = 1024 * 1024;

/// Runs blocking disk work on tokio's blocking threads.
pub(crate) async fn blocking<T, E>(
    work: impl FnOnce() -> Result<T, E> + Send + 'static,
) -> Result<T, E>
where
    T: Send + 'static,
    E: From<io::Error> + Send + 'static,
{
    tokio::task::spawn_blocking(work)
        .await
        .unwrap_or_else(|joined| Err(io::Error::other(joined).into()))
}

/// The temporary name of file `index` of transfer `id`.
pub(crate) fn temp_name(id: TransferId, index: usize) -> String {
    format!("{TEMP_PREFIX}{id}-{index}{TEMP_SUFFIX}")
}

/// Whether `name` is a [`temp_name`].
fn is_temp_name(name: &OsStr) -> bool {
    let Some(rest) = name
        .to_str()
        .and_then(|name| name.strip_prefix(TEMP_PREFIX))
        .and_then(|rest| rest.strip_suffix(TEMP_SUFFIX))
    else {
        return false;
    };
    let Some((id, index)) = rest.split_once('-') else {
        return false;
    };
    id.len() == 32
        && id.bytes().all(|b| b.is_ascii_hexdigit())
        && !index.is_empty()
        && index.bytes().all(|b| b.is_ascii_digit())
}

/// Creates the temporary file at `path`. It must not exist yet, which also
/// means a link planted there is never followed. Received files are never
/// executable; who else may read them is up to the user's umask, as for
/// any other download.
pub(crate) fn create_temp(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o666)
        .open(path)
}

/// Opens the temporary file at `path` to go on after its first `written`
/// bytes: a new one for 0, otherwise the one written before, which must
/// still be a regular file holding at least those bytes. What lies beyond
/// them is cut off. A link planted there is never followed.
pub(crate) fn open_temp(path: &Path, written: u64) -> io::Result<File> {
    if written == 0 {
        match create_temp(path) {
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
            created => return created,
        }
    }
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(OFlags::NOFOLLOW.bits().cast_signed())
        .open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() < written {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            "the unfinished file changed",
        ));
    }
    file.set_len(written)?;
    Ok(file)
}

/// The hash of the first `written` bytes of the temporary `file`, to go on
/// from there.
pub(crate) fn hash_written(file: &File, written: u64) -> io::Result<digest::Context> {
    hash_prefix(file, written).map_err(|error| match error {
        SourceError::Changed => {
            io::Error::new(ErrorKind::UnexpectedEof, "the unfinished file shrank")
        }
        SourceError::Unreadable(error) => error,
    })
}

/// Writes `data` at `offset` of `file` and adds it to `hash`.
pub(crate) fn write_chunk(
    file: &File,
    offset: u64,
    data: &[u8],
    hash: &mut digest::Context,
) -> io::Result<()> {
    file.write_all_at(data, offset)?;
    hash.update(data);
    Ok(())
}

/// Makes the finished file `temp` durable, then gives it its name in
/// `dir`: `name` if it is free, otherwise the first free numbered one.
/// Returns where it ended up. Never replaces a file.
pub(crate) fn publish(file: &File, temp: &Path, dir: &Path, name: &str) -> io::Result<PathBuf> {
    file.sync_all()?;
    for n in 0..MAX_NUMBER {
        let target = dir.join(names::numbered(name, n));
        match rename_new(temp, &target) {
            Ok(()) => return Ok(target),
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        ErrorKind::AlreadyExists,
        "every numbered name is taken",
    ))
}

/// Renames `from` to `to`, failing with [`ErrorKind::AlreadyExists`] when
/// anything is at `to`, even a dangling link.
fn rename_new(from: &Path, to: &Path) -> io::Result<()> {
    match rustix::fs::renameat_with(CWD, from, CWD, to, RenameFlags::NOREPLACE) {
        Ok(()) => Ok(()),
        // File systems without the flag: a hard link fails the same way
        // when the name is taken.
        Err(Errno::INVAL | Errno::NOSYS | Errno::NOTSUP) => match fs::hard_link(from, to) {
            Ok(()) => fs::remove_file(from),
            Err(error) if error.kind() == ErrorKind::AlreadyExists => Err(error),
            // Nor hard links: reserve the name with a new empty file, then
            // replace that.
            Err(_) => {
                drop(create_temp(to)?);
                fs::rename(from, to)
            }
        },
        Err(error) => Err(error.into()),
    }
}

/// Bytes the user may still write to the file system of `dir`.
pub(crate) fn free_space(dir: &Path) -> io::Result<u64> {
    let stat = rustix::fs::statvfs(dir)?;
    Ok(stat.f_bavail.saturating_mul(stat.f_frsize))
}

/// Removes the temporary files that transfers of an earlier run left in
/// `dir`, and returns how many there were. A missing `dir` has none.
pub(crate) fn remove_leftovers(dir: &Path) -> io::Result<usize> {
    let entries = match fs::read_dir(dir) {
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(0),
        entries => entries?,
    };
    let mut removed = 0;
    for entry in entries {
        let entry = entry?;
        if is_temp_name(&entry.file_name()) && entry.file_type()?.is_file() {
            match fs::remove_file(entry.path()) {
                Ok(()) => removed += 1,
                Err(error) => tracing::warn!(%error, "could not remove an unfinished download"),
            }
        }
    }
    Ok(removed)
}

/// Removes `path` if it is there.
pub(crate) fn remove_if_present(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        removed => removed,
    }
}

/// Why a file being sent cannot be sent on.
#[derive(Debug, thiserror::Error)]
pub(crate) enum SourceError {
    /// Its size or modification time is not what it was when the user sent
    /// it.
    #[error("the file changed")]
    Changed,
    /// Reading it failed.
    #[error(transparent)]
    Unreadable(#[from] io::Error),
}

/// Opens the file at `path` to send it, checking that it is still the
/// file of `size` bytes, last modified at `modified`, that the user sent.
pub(crate) fn open_source(
    path: &Path,
    size: u64,
    modified: Option<SystemTime>,
) -> Result<File, SourceError> {
    let file = File::open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() != size || metadata.modified().ok() != modified {
        return Err(SourceError::Changed);
    }
    Ok(file)
}

/// Reads `len` bytes at `offset` of `file`, a file of `size` bytes last
/// modified at `modified`, and adds them to `hash`. A file rewritten since,
/// even at the same size, has changed.
pub(crate) fn read_chunk(
    file: &File,
    offset: u64,
    len: usize,
    (size, modified): (u64, Option<SystemTime>),
    hash: &mut digest::Context,
) -> Result<Vec<u8>, SourceError> {
    let mut data = vec![0; len];
    file.read_exact_at(&mut data, offset)
        .map_err(|error| match error.kind() {
            ErrorKind::UnexpectedEof => SourceError::Changed,
            _ => SourceError::Unreadable(error),
        })?;
    let metadata = file.metadata()?;
    if metadata.len() != size || metadata.modified().ok() != modified {
        return Err(SourceError::Changed);
    }
    hash.update(&data);
    Ok(data)
}

/// The hash of the first `len` bytes of `file`, to go on from there.
pub(crate) fn hash_prefix(file: &File, len: u64) -> Result<digest::Context, SourceError> {
    let mut hash = digest::Context::new(&digest::SHA256);
    let mut buffer = vec![0; HASH_READ_LEN];
    let mut offset = 0;
    while offset < len {
        let n = usize::try_from(len - offset).map_or(HASH_READ_LEN, |left| left.min(HASH_READ_LEN));
        file.read_exact_at(&mut buffer[..n], offset)
            .map_err(|error| match error.kind() {
                ErrorKind::UnexpectedEof => SourceError::Changed,
                _ => SourceError::Unreadable(error),
            })?;
        hash.update(&buffer[..n]);
        offset += n as u64;
    }
    Ok(hash)
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    fn id() -> TransferId {
        TransferId([0xab; 16])
    }

    #[test]
    fn temp_names_are_hidden_and_recognised() {
        let name = temp_name(id(), 7);
        assert_eq!(name, format!(".link2linux-{}-7.part", "ab".repeat(16)));
        assert!(is_temp_name(OsStr::new(&name)));
        for other in [
            ".link2linux-ab-7.part",
            &format!(".link2linux-{}-.part", "ab".repeat(16)),
            &format!(".link2linux-{}-7.partial", "ab".repeat(16)),
            &format!("link2linux-{}-7.part", "ab".repeat(16)),
            &format!(".link2linux-{}-x.part", "zz".repeat(16)),
            "photo.jpg",
        ] {
            assert!(!is_temp_name(OsStr::new(other)), "{other}");
        }
    }

    #[test]
    fn published_files_never_replace_others_and_are_not_executable() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("photo.jpg"), b"theirs").unwrap();
        std::os::unix::fs::symlink("/nowhere", dir.path().join("photo (1).jpg")).unwrap();

        let temp = dir.path().join(temp_name(id(), 0));
        let file = create_temp(&temp).unwrap();
        let mut hash = digest::Context::new(&digest::SHA256);
        write_chunk(&file, 0, b"ours", &mut hash).unwrap();
        assert!(
            create_temp(&temp).is_err(),
            "a temporary file is always new"
        );

        let saved = publish(&file, &temp, dir.path(), "photo.jpg").unwrap();
        assert_eq!(saved, dir.path().join("photo (2).jpg"));
        assert_eq!(fs::read(&saved).unwrap(), b"ours");
        assert_eq!(fs::read(dir.path().join("photo.jpg")).unwrap(), b"theirs");
        assert!(!temp.exists());
        let mode = fs::metadata(&saved).unwrap().permissions().mode();
        assert_eq!(mode & 0o111, 0, "{mode:o}");
    }

    #[test]
    fn unfinished_files_are_taken_up_again_only_as_they_were_left() {
        let dir = tempfile::tempdir().unwrap();
        let temp = dir.path().join(temp_name(id(), 0));
        let file = open_temp(&temp, 0).unwrap();
        let mut hash = digest::Context::new(&digest::SHA256);
        write_chunk(&file, 0, b"abcdef", &mut hash).unwrap();
        drop(file);

        // Taken up after 4 bytes: the rest is cut, and its hash is the
        // same as of those 4 bytes.
        let again = open_temp(&temp, 4).unwrap();
        assert_eq!(again.metadata().unwrap().len(), 4);
        let mut expected = digest::Context::new(&digest::SHA256);
        expected.update(b"abcd");
        assert_eq!(
            hash_written(&again, 4).unwrap().finish().as_ref(),
            expected.finish().as_ref()
        );
        // Shorter than what was written, or gone, it is refused.
        assert!(open_temp(&temp, 10).is_err());
        fs::remove_file(&temp).unwrap();
        assert!(open_temp(&temp, 4).is_err());
        // A link planted in its place is not followed.
        fs::write(dir.path().join("target"), b"abcd").unwrap();
        std::os::unix::fs::symlink(dir.path().join("target"), &temp).unwrap();
        assert!(open_temp(&temp, 4).is_err());
        assert_eq!(fs::read(dir.path().join("target")).unwrap(), b"abcd");
    }

    #[test]
    fn renames_never_replace() {
        let dir = tempfile::tempdir().unwrap();
        let (from, to) = (dir.path().join("a"), dir.path().join("b"));
        fs::write(&from, b"a").unwrap();
        fs::write(&to, b"b").unwrap();
        let error = rename_new(&from, &to).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::AlreadyExists);
        assert_eq!(fs::read(&to).unwrap(), b"b");
    }

    #[test]
    fn leftovers_are_removed_and_nothing_else() {
        let dir = tempfile::tempdir().unwrap();
        let temp = dir.path().join(temp_name(id(), 3));
        fs::write(&temp, b"partial").unwrap();
        fs::write(dir.path().join("keep.txt"), b"mine").unwrap();
        // A directory with a temporary name is not ours to remove.
        fs::create_dir(dir.path().join(temp_name(TransferId([1; 16]), 0))).unwrap();

        assert_eq!(remove_leftovers(dir.path()).unwrap(), 1);
        assert!(!temp.exists());
        assert!(dir.path().join("keep.txt").exists());
        assert_eq!(remove_leftovers(&dir.path().join("missing")).unwrap(), 0);
    }

    #[test]
    fn sources_that_change_are_noticed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("data");
        fs::write(&path, vec![7; 1000]).unwrap();
        let modified = fs::metadata(&path).unwrap().modified().ok();

        let file = open_source(&path, 1000, modified).unwrap();
        let mut hash = digest::Context::new(&digest::SHA256);
        assert_eq!(
            read_chunk(&file, 0, 600, (1000, modified), &mut hash)
                .unwrap()
                .len(),
            600
        );
        let prefix = hash_prefix(&file, 600).unwrap();
        assert_eq!(prefix.finish().as_ref(), hash.clone().finish().as_ref());

        assert!(matches!(
            open_source(&path, 999, modified),
            Err(SourceError::Changed)
        ));
        // Rewritten in place at the same size: only its time tells.
        let rewritten = OpenOptions::new().write(true).open(&path).unwrap();
        rewritten.write_all_at(&[8; 1000], 0).unwrap();
        let later = modified.unwrap() + std::time::Duration::from_secs(5);
        rewritten.set_modified(later).unwrap();
        assert!(matches!(
            read_chunk(&file, 600, 400, (1000, modified), &mut hash),
            Err(SourceError::Changed)
        ));
        fs::write(&path, vec![7; 500]).unwrap();
        assert!(matches!(
            read_chunk(&file, 600, 400, (1000, Some(later)), &mut hash),
            Err(SourceError::Changed)
        ));
        assert!(matches!(
            open_source(&dir.path().join("gone"), 1, None),
            Err(SourceError::Unreadable(_))
        ));
    }

    #[test]
    fn free_space_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        assert!(free_space(dir.path()).unwrap() > 0);
        assert!(free_space(&dir.path().join("missing")).is_err());
    }
}
