//! Files Brama writes beside its target: a staging file whose write or
//! rename failed, or a key file a child process has read, removed and said;
//! the exclusive lock that holds a shared record for one writer across every
//! Brama process; and the private whole-file replace those records are
//! written with.
//!
//! A removal that fails leaves the file on disk, and the next write under the
//! same name (`create_new`) is refused by its existence rather than by its
//! real cause. So the failure is said, never dropped; a file that is already
//! gone is not a failure.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

/// Hold `record` (named `what` in refusals) for this caller alone across
/// every Brama process on the same state: an exclusive lock on the file
/// `<record>.lock` beside it, which the operating system releases with the
/// returned file even when the holder dies. A process-local mutex let two
/// Brama processes read the record before either wrote it. The wait for a
/// holder is named while it lasts.
pub fn held_beside(record: &Path, what: &str) -> Result<File, String> {
    let parent = record
        .parent()
        .ok_or_else(|| format!("{what} {} names no directory", record.display()))?;
    std::fs::create_dir_all(parent).map_err(|error| {
        format!(
            "the directory of {what} {} cannot be made: {error}",
            parent.display()
        )
    })?;
    let lock = record.with_extension("lock");
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&lock)
        .map_err(|error| {
            format!(
                "the lock of {what} {} cannot be opened: {error}",
                lock.display()
            )
        })?;
    stado_wait::blocking(stado_wait::Kind::Lock, what, lock.display(), || file.lock()).map_err(
        |error| {
            format!(
                "the lock of {what} {} cannot be taken: {error}",
                lock.display()
            )
        },
    )?;
    Ok(file)
}

/// `bytes` as the whole of `path` (named `what` in refusals), through a
/// staged file readable by its owner alone from the moment it exists, synced
/// before it is renamed over `path`. Callers hold [`held_beside`], so one
/// staging name serves; a staging file a failed step leaves is named.
pub fn replace_private(path: &Path, bytes: &[u8], what: &str) -> Result<(), String> {
    let staged = path.with_extension("staging");
    let written = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(u32::from(libc::S_IRUSR | libc::S_IWUSR))
        .open(&staged)
        .and_then(|mut file| file.write_all(bytes).and_then(|()| file.sync_all()))
        .and_then(|()| std::fs::rename(&staged, path));
    written.map_err(|error| {
        failed_with(
            format!("{what} {} cannot be written: {error}", path.display()),
            &staged,
        )
    })
}

/// Remove `path`. `Some` is the sentence that says the file is still there
/// and why; `None` when it is gone, including when it never existed.
pub fn remove(path: &Path) -> Option<String> {
    match std::fs::remove_file(path) {
        Ok(()) => None,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => Some(format!("{} is left behind: {error}", path.display())),
    }
}

/// `error`, followed by the sentence of a staging file its removal left.
pub fn failed_with(error: String, staging: &Path) -> String {
    match remove(staging) {
        None => error,
        Some(left) => format!("{error}; {left}"),
    }
}
