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

fn unreadable(path: &Path, error: std::io::Error) -> DiscoveryError {
    DiscoveryError::Unreadable {
        path: path.to_path_buf(),
        kind: error.kind(),
    }
}

/// Canonicalizes the traversal start so ancestor discovery follows the real
/// filesystem hierarchy rather than a lexical path containing symlinks or `..`.
///
/// Canonicalization failure is an error, never a request to fall back to the
/// lexical path. Falling back can move the trust boundary the caller believes it
/// is using and can let an unrelated parent config govern the process.
pub(crate) fn canonical_start(start: &Path) -> Result<PathBuf, DiscoveryError> {
    let canonical = std::fs::canonicalize(start).map_err(|error| unreadable(start, error))?;
    Ok(if canonical.is_file() {
        canonical
            .parent()
            .map_or(canonical.clone(), Path::to_path_buf)
    } else {
        canonical
    })
}

/// Canonicalizes an explicit fallback bound in the same coordinate system as
/// the traversal start. An operator-supplied bound that cannot be resolved is an
/// error: silently using its lexical spelling can make the equality check miss
/// and allow discovery to escape above the intended boundary.
pub(crate) fn canonical_bound(
    bound: Option<&Path>,
) -> Result<Option<PathBuf>, DiscoveryError> {
    bound
        .map(|limit| std::fs::canonicalize(limit).map_err(|error| unreadable(limit, error)))
        .transpose()
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
