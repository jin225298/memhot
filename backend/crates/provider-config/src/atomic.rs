//! Crash-safe file replacement shared by the durable repository and vault.

use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
};

/// Writes `contents` to `path` via a staging file plus rename, so a crash
/// leaves either the previous version or the new one, never a truncated file.
/// `fs::rename` replaces an existing destination on both POSIX and Windows.
///
/// When `owner_only` is set the staging file is created with owner-only
/// permissions *before* any bytes are written, so secret material is never
/// briefly readable by other users. Windows has no POSIX mode bits; there the
/// file inherits directory ACLs, matching how `.secrets/` is already treated.
pub(crate) fn atomic_write(path: &Path, contents: &[u8], owner_only: bool) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }

    let staging = staging_path(path);
    // A leftover staging file from an earlier crash must not block the write.
    let _ = fs::remove_file(&staging);

    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        if owner_only {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
    }
    #[cfg(not(unix))]
    {
        let _ = owner_only;
    }

    // Scoped so the handle closes before the rename; Windows refuses to rename
    // a file that still has an open handle.
    let staged = (|| -> io::Result<()> {
        let mut file = options.open(&staging)?;
        file.write_all(contents)?;
        file.sync_all()
    })();

    let outcome = staged.and_then(|()| fs::rename(&staging, path));
    if outcome.is_err() {
        let _ = fs::remove_file(&staging);
    }
    outcome
}

fn staging_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".tmp");
    path.with_file_name(name)
}
