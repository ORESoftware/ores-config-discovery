use crate::{DiscoveryError, GitMarker};
use std::path::{Path, PathBuf};

pub(crate) fn is_bare_file_name(file_name: &str) -> bool {
    let mut components = Path::new(file_name).components();
    matches!(
        (components.next(), components.next()),
        (Some(std::path::Component::Normal(component)), None)
            if component == std::ffi::OsStr::new(file_name)
    )
}

pub(crate) fn canonical_start(start: &Path) -> PathBuf {
    let canonical = std::fs::canonicalize(start).unwrap_or_else(|_| start.to_path_buf());
    if canonical.is_file() {
        canonical
            .parent()
            .map_or(canonical.clone(), Path::to_path_buf)
    } else {
        canonical
    }
}

pub(crate) fn canonical_bound(bound: Option<&Path>) -> Option<PathBuf> {
    bound.map(|limit| std::fs::canonicalize(limit).unwrap_or_else(|_| limit.to_path_buf()))
}

/// The `.git` entry in `directory`, if any. Inspected without following
/// symlinks, so a dangling or symlinked `.git` still marks a boundary. Errors
/// other than absence are propagated so a trust-boundary probe cannot fail open.
pub(crate) fn git_marker(directory: &Path) -> Result<Option<GitMarker>, DiscoveryError> {
    let path = directory.join(".git");
    match std::fs::symlink_metadata(&path) {
        Ok(meta) => Ok(Some(if meta.is_dir() {
            GitMarker::Directory
        } else {
            GitMarker::File
        })),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(DiscoveryError::Unreadable {
            path,
            kind: error.kind(),
        }),
    }
}
