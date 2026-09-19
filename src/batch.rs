use crate::internal::{canonical_bound, canonical_start, git_marker, is_bare_file_name};
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

/// A configured one-pass search for several config filenames.
///
/// Prefer the single-file [`crate::Search`] / [`crate::locate`] APIs for normal
/// library code. The batch API is probably more complicated to use because one
/// caller owns missing/error policy for several independent config concerns.
/// This builder exists for inventory/startup callers that deliberately own that
/// coupling and need one ancestor walk.
#[derive(Clone, Debug)]
pub struct BatchSearch<'a> {
    file_names: &'a [&'a str],
    bound: Option<PathBuf>,
    refuse_non_regular: bool,
}

impl<'a> BatchSearch<'a> {
    /// Creates a batch search with no fallback bound and default single-file
    /// semantics for non-regular candidates: skip them and keep walking.
    #[must_use]
    pub fn new(file_names: &'a [&'a str]) -> Self {
        Self {
            file_names,
            bound: None,
            refuse_non_regular: false,
        }
    }

    /// Ends the walk at `bound` when no Git boundary is met first. The bound
    /// directory itself is searched; its parents are not.
    #[must_use]
    pub fn bound(mut self, bound: impl Into<PathBuf>) -> Self {
        self.bound = Some(bound.into());
        self
    }

    /// Ends the walk at `$HOME`, when it is set, matching [`crate::Search`].
    #[must_use]
    pub fn bound_at_home(mut self) -> Self {
        self.bound = std::env::var_os("HOME").map(PathBuf::from);
        self
    }

    /// When enabled, a candidate that exists but is not a regular file is an
    /// error instead of being skipped. Symlinks are refused either way.
    ///
    /// This mirrors [`crate::Search::refuse_non_regular`] so strict callers do
    /// not lose a security policy merely because they switch to batching.
    #[must_use]
    pub fn refuse_non_regular(mut self, refuse: bool) -> Self {
        self.refuse_non_regular = refuse;
        self
    }

    /// Runs the batch search from the current working directory.
    ///
    /// # Errors
    ///
    /// See [`BatchSearch::from`]. An unreadable working directory yields an
    /// all-`None` result, matching [`crate::Search::from_cwd`].
    pub fn from_cwd(&self) -> Result<Vec<BatchResult>, DiscoveryError> {
        let Ok(start) = std::env::current_dir() else {
            return Ok(empty_results(self.file_names));
        };
        self.from(&start)
    }

    /// Runs one ancestor walk at or above `start` for every requested name.
    /// When `start` is a file, the walk begins in its directory.
    ///
    /// The output preserves input order, including duplicate names. Each name
    /// keeps nearest-wins semantics independently. The call is fail-fast: an
    /// unsafe or unreadable candidate for any unresolved name fails the whole
    /// batch rather than returning a partial success that could hide a policy
    /// violation.
    ///
    /// # Errors
    ///
    /// Fails on an invalid name, symlinked candidate, unreadable candidate,
    /// unreadable Git-boundary metadata, or (when configured) a non-regular
    /// candidate. A missing config is represented by `BatchResult { located:
    /// None, .. }`.
    pub fn from(&self, start: &Path) -> Result<Vec<BatchResult>, DiscoveryError> {
        for file_name in self.file_names {
            if !is_bare_file_name(file_name) {
                return Err(DiscoveryError::NotAFileName((*file_name).to_owned()));
            }
        }

        if self.file_names.is_empty() {
            return Ok(Vec::new());
        }

        let start = canonical_start(start);
        let bound = canonical_bound(self.bound.as_deref());

        let mut results = empty_results(self.file_names);

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
                    Ok(_) if self.refuse_non_regular => {
                        return Err(DiscoveryError::NotRegularFile(candidate));
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
/// Returns the same default-policy discovery errors as the single-file API.
pub fn discover_many(file_names: &[&str]) -> Result<Vec<BatchResult>, DiscoveryError> {
    BatchSearch::new(file_names).bound_at_home().from_cwd()
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
    BatchSearch::new(file_names).bound_at_home().from(start)
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
    let search = BatchSearch::new(file_names);
    match bound {
        Some(limit) => search.bound(limit).from(start),
        None => search.from(start),
    }
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
    fn duplicate_names_preserve_positional_results() {
        let tree = Tree::new("duplicates");
        tree.file(".git/HEAD");
        let config = tree.file(".ores-otel.toml");
        let start = tree.dir("src");
        let names = [".ores-otel.toml", ".ores-otel.toml"];

        let batch = discover_many_bounded(&start, &names, None).expect("batch succeeds");
        assert_eq!(batch.len(), 2);
        assert_eq!(batch[0].file_name, ".ores-otel.toml");
        assert_eq!(batch[1].file_name, ".ores-otel.toml");
        assert_eq!(
            batch[0].located.as_ref().map(|found| &found.path),
            Some(&config)
        );
        assert_eq!(batch[1].located, batch[0].located);
    }

    #[test]
    fn default_non_regular_policy_matches_single_search() {
        let tree = Tree::new("nonregular-default");
        tree.file(".git/HEAD");
        let root_config = tree.file(".ores-rl.toml");
        tree.dir("service/.ores-rl.toml");
        let start = tree.dir("service/src");
        let names = [".ores-rl.toml"];

        let batch = discover_many_bounded(&start, &names, None).expect("batch succeeds");
        let single = crate::locate_bounded(&start, names[0], None).expect("single succeeds");
        assert_eq!(batch[0].located, single);
        assert_eq!(
            batch[0].located.as_ref().map(|found| &found.path),
            Some(&root_config)
        );
    }

    #[test]
    fn strict_non_regular_policy_is_available_for_batch_callers() {
        let tree = Tree::new("nonregular-strict");
        tree.file(".git/HEAD");
        tree.file(".ores-rl.toml");
        let non_regular = tree.dir("service/.ores-rl.toml");
        let start = tree.dir("service/src");
        let names = [".ores-rl.toml"];

        assert!(matches!(
            BatchSearch::new(&names)
                .refuse_non_regular(true)
                .from(&start),
            Err(DiscoveryError::NotRegularFile(path)) if path == non_regular
        ));
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
    fn explicit_bound_is_searched_but_never_crossed() {
        let tree = Tree::new("bound");
        tree.file(".ores-compose.yaml");
        let bound = tree.dir("home/user");
        let start = tree.dir("home/user/project/deep");
        let names = [".ores-compose.yaml"];

        let first = discover_many_bounded(&start, &names, Some(&bound)).expect("batch succeeds");
        assert_eq!(first[0].located, None);

        let at_bound = tree.file("home/user/.ores-compose.yaml");
        let second = discover_many_bounded(&start, &names, Some(&bound)).expect("batch succeeds");
        assert_eq!(
            second[0].located.as_ref().map(|found| &found.path),
            Some(&at_bound)
        );
    }

    #[test]
    fn invalid_names_fail_before_traversal() {
        let tree = Tree::new("invalid");
        for bad in [
            "secret/.ores-mw.toml",
            "../.ores-mw.toml",
            "/.ores-mw.toml",
            "",
            ".",
            "..",
        ] {
            assert!(matches!(
                discover_many_bounded(&tree.0, &[".ores-otel.toml", bad], None),
                Err(DiscoveryError::NotAFileName(name)) if name == bad
            ));
        }
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
    fn symlinked_home_style_bound_is_canonicalized() {
        let tree = Tree::new("bound-link");
        tree.file(".ores-rl.toml");
        let project = tree.dir("realhome/project");
        let link = tree.0.join("home-link");
        std::os::unix::fs::symlink(tree.0.join("realhome"), &link).expect("symlink");
        let names = [".ores-rl.toml"];

        let batch = discover_many_bounded(&project, &names, Some(&link)).expect("batch succeeds");
        assert_eq!(batch[0].located, None);
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

    #[cfg(unix)]
    #[test]
    fn symlinked_git_entry_is_a_boundary_and_not_strict_root_placement() {
        let tree = Tree::new("git-link");
        tree.file(".ores-rl.toml");
        tree.file("realgit/HEAD");
        let repo = tree.dir("repo");
        std::os::unix::fs::symlink(tree.0.join("realgit"), repo.join(".git")).expect("symlink");
        tree.file("repo/.ores-otel.toml");
        let names = [".ores-otel.toml", ".ores-rl.toml"];

        let batch =
            discover_many_bounded(&tree.dir("repo/src"), &names, None).expect("batch succeeds");
        let found = batch[0].located.as_ref().expect("local config found");
        assert_eq!(found.git_marker, Some(GitMarker::File));
        assert!(found.at_repo_root);
        assert!(!found.beside_git_directory());
        assert_eq!(batch[1].located, None, "parent config must stay invisible");
    }
}
