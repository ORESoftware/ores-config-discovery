use crate::{DiscoveryError, GitMarker, Located, MAX_ANCESTORS};
use std::path::{Path, PathBuf};

/// One requested filename and the nearest config found for it, if any.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct BatchResult {
    /// The exact bare filename requested by the caller.
    pub file_name: String,
    /// The nearest matching config, or `None` when none exists before the
    /// applicable Git/home/filesystem boundary.
    pub located: Option<Located>,
}

/// Locates several config filenames in one ancestor walk from the current
/// working directory, bounded by `$HOME` outside any repository.
///
/// The output preserves the input order. The batch is fail-fast: an unsafe or
/// unreadable candidate for any requested filename fails the entire call.
///
/// Prefer the single-file [`crate::Search`] / [`crate::locate`] APIs for normal
/// library code. The batch API is probably more complicated to use because one
/// caller now owns missing/error policy for several independent config concerns;
/// use it mainly for inventory-style callers that truly benefit from one walk.
///
/// # Errors
///
/// Returns the same discovery errors as the single-file API, including invalid
/// names, symlinked candidates, unreadable candidates, and unreadable Git
/// boundary metadata.
pub fn discover_many(file_names: &[&str]) -> Result<Vec<BatchResult>, DiscoveryError> {
    let Ok(start) = std::env::current_dir() else {
        return Ok(empty_results(file_names));
    };
    discover_many_from(&start, file_names)
}

/// [`discover_many`] from an explicit starting path.
///
/// The walk is bounded at `$HOME` when it is set, matching [`crate::locate_from`].
///
/// # Errors
///
/// See [`discover_many`].
pub fn discover_many_from(
    start: &Path,
    file_names: &[&str],
) -> Result<Vec<BatchResult>, DiscoveryError> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    discover_many_bounded(start, file_names, home.as_deref())
}

/// Batch discovery with an explicit fallback bound. `None` walks toward the
/// filesystem root, still capped at [`MAX_ANCESTORS`].
///
/// The bound directory itself is searched; its parents are not.
///
/// # Errors
///
/// See [`discover_many`].
pub fn discover_many_bounded(
    start: &Path,
    file_names: &[&str],
    bound: Option<&Path>,
) -> Result<Vec<BatchResult>, DiscoveryError> {
    // Keep the single-file API as the preferred path. This loop deliberately
    // exists only for callers that need a multi-config inventory: batching
    // couples independent config concerns and therefore makes ownership of
    // missing/error policy more complicated than one Search/locate call each.
    for file_name in file_names {
        if !is_bare_file_name(file_name) {
            return Err(DiscoveryError::NotAFileName((*file_name).to_owned()));
        }
    }

    if file_names.is_empty() {
        return Ok(Vec::new());
    }

    let canonical = std::fs::canonicalize(start).unwrap_or_else(|_| start.to_path_buf());
    let start = if canonical.is_file() {
        canonical
            .parent()
            .map_or(canonical.clone(), Path::to_path_buf)
    } else {
        canonical
    };
    let bound =
        bound.map(|limit| std::fs::canonicalize(limit).unwrap_or_else(|_| limit.to_path_buf()));

    let mut results = empty_results(file_names);

    for directory in start.ancestors().take(MAX_ANCESTORS) {
        let marker = git_marker(directory)?;

        for result in &mut results {
            if result.located.is_some() {
                continue;
            }

            let candidate = directory.join(&result.file_name);
            match std::fs::symlink_metadata(&candidate) {
                Ok(meta) if meta.file_type().is_symlink() => {
                    return Err(DiscoveryError::Symlink(candidate));
                }
                Ok(meta) if meta.is_file() => {
                    result.located = Some(Located {
                        path: candidate,
                        directory: directory.to_path_buf(),
                        git_marker: marker,
                        at_repo_root: marker.is_some(),
                    });
                }
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(DiscoveryError::Unreadable {
                        path: candidate,
                        kind: error.kind(),
                    });
                }
            }
        }

        if results.iter().all(|result| result.located.is_some()) {
            break;
        }

        // Search the boundary directory itself, but never let an unresolved
        // batch entry escape into a parent checkout or above the explicit bound.
        if marker.is_some() || bound.as_deref().is_some_and(|limit| directory == limit) {
            break;
        }
    }

    Ok(results)
}

fn empty_results(file_names: &[&str]) -> Vec<BatchResult> {
    file_names
        .iter()
        .map(|file_name| BatchResult {
            file_name: (*file_name).to_owned(),
            located: None,
        })
        .collect()
}

fn is_bare_file_name(file_name: &str) -> bool {
    let mut components = Path::new(file_name).components();
    matches!(
        (components.next(), components.next()),
        (Some(std::path::Component::Normal(component)), None)
            if component == std::ffi::OsStr::new(file_name)
    )
}

fn git_marker(directory: &Path) -> Result<Option<GitMarker>, DiscoveryError> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Tree(PathBuf);

    impl Tree {
        fn new(tag: &str) -> Self {
            static COUNTER: AtomicUsize = AtomicUsize::new(0);
            let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir().join(format!(
                "ores-config-discovery-batch-{tag}-{}-{unique}",
                std::process::id()
            ));
            fs::create_dir_all(&root).expect("temp tree is creatable");
            Self(fs::canonicalize(&root).expect("temp tree canonicalises"))
        }

        fn dir(&self, relative: &str) -> PathBuf {
            let path = self.0.join(relative);
            fs::create_dir_all(&path).expect("subdirectory is creatable");
            path
        }

        fn file(&self, relative: &str) -> PathBuf {
            let path = self.0.join(relative);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).expect("parent is creatable");
            }
            fs::write(&path, b"# fixture\n").expect("file is writable");
            path
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn batch_matches_repeated_single_discovery_and_preserves_order() {
        let tree = Tree::new("parity");
        tree.file(".git/HEAD");
        tree.file(".ores-otel.toml");
        tree.file("service/.ores-rl.toml");
        let start = tree.dir("service/src/deep");
        let names = [".ores-rl.toml", ".ores-otel.toml", ".missing.toml"];

        let batch = discover_many_bounded(&start, &names, None).expect("batch succeeds");
        assert_eq!(batch.len(), names.len());

        for (entry, name) in batch.iter().zip(names) {
            assert_eq!(entry.file_name, name);
            assert_eq!(
                entry.located,
                crate::locate_bounded(&start, name, None).expect("single discovery succeeds"),
                "batch semantics must match repeated single-file discovery"
            );
        }
    }

    #[test]
    fn unresolved_entries_do_not_cross_a_git_boundary() {
        let tree = Tree::new("boundary");
        tree.file(".ores-compose.yaml");
        tree.file("vendor/inner/.git/HEAD");
        let start = tree.dir("vendor/inner/src");

        let batch =
            discover_many_bounded(&start, &[".ores-compose.yaml"], None).expect("batch succeeds");
        assert_eq!(batch[0].located, None);
    }

    #[test]
    fn invalid_name_fails_the_batch_before_traversal() {
        let tree = Tree::new("invalid");
        assert!(matches!(
            discover_many_bounded(&tree.0, &[".ores-otel.toml", "../secret.toml"], None),
            Err(DiscoveryError::NotAFileName(name)) if name == "../secret.toml"
        ));
    }

    #[test]
    fn empty_batch_is_empty() {
        let tree = Tree::new("empty");
        assert_eq!(
            discover_many_bounded(&tree.0, &[], None).expect("empty batch succeeds"),
            Vec::<BatchResult>::new()
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_candidate_fails_the_entire_batch() {
        let tree = Tree::new("symlink");
        tree.file(".git/HEAD");
        tree.file(".ores-otel.toml");
        let real = tree.file("elsewhere/real.toml");
        std::os::unix::fs::symlink(&real, tree.0.join(".ores-rl.toml")).expect("symlink");

        assert!(matches!(
            discover_many_bounded(
                &tree.dir("src"),
                &[".ores-otel.toml", ".ores-rl.toml"],
                None
            ),
            Err(DiscoveryError::Symlink(_))
        ));
    }
}
