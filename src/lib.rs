//! Locate the nearest fleet config file by walking up from the working directory.
//!
//! Every fleet library is configured by a dotfile that belongs at the root of
//! the repository it governs — `.ores-mw.toml`, `.ores-rl.toml`, and so on. A
//! process, though, does not necessarily run from that root: a test runs from a
//! crate subdirectory, a server may be started from `deploy/`, a tool may be
//! invoked from anywhere. Resolving the file relative to `$PWD` alone would
//! silently apply different configuration depending on where you stood.
//!
//! So discovery walks upward from the starting directory and takes the **first**
//! match. Nearest wins, which is what makes a nested override work at all.
//!
//! # What this crate owns, and what it leaves to the caller
//!
//! This is the traversal primitive for the whole fleet, so it owns exactly the
//! part that must be identical everywhere: the ancestor walk, its boundaries,
//! and the refusal of unsafe leaves. Everything a library legitimately decides
//! for itself stays with the library:
//!
//! * **What a missing file means.** Discovery returns `Ok(None)`. A rate limiter
//!   turns that into an error (running with no policy must never be silent); a
//!   telemetry loader resolves defaults.
//! * **What counts as "at the repository root".** [`Located::git_marker`] says
//!   whether the adjacent `.git` is a directory or a file, so a library can
//!   accept either ([`Located::at_repo_root`]) or insist on a real clone
//!   ([`Located::beside_git_directory`]).
//! * **How a misplaced file is reported.** Libraries with their own logger and
//!   event schema emit their own warning. [`locate_and_report`] exists for the
//!   ones that do not.
//!
//! # Boundaries
//!
//! A config above the repository belongs to nobody, and letting it in would let
//! an unrelated parent checkout or a home-directory file govern this process.
//!
//! * The walk ends at the first **Git boundary** — a directory holding a `.git`
//!   entry of any kind. That directory is searched; its parents are not.
//! * An optional bound (by default `$HOME` for the convenience functions) ends
//!   the walk outside any repository, matching flags-2-env.
//! * At most [`MAX_ANCESTORS`] directories are examined.
//!
//! A symlinked candidate is refused outright: a file that points somewhere else
//! is not "the config at this path", and following it would defeat the
//! boundaries above.

#![forbid(unsafe_code)]

use std::fmt;
use std::path::{Path, PathBuf};

/// Ancestors examined before giving up, so a pathological path cannot turn
/// discovery into an unbounded scan.
pub const MAX_ANCESTORS: usize = 64;

/// The kind of `.git` entry beside a located config.
///
/// A normal clone has a `.git` directory; a worktree or submodule has a `.git`
/// *file* pointing at the real gitdir. Both end the walk. Whether both count as
/// repository-root *placement* is the caller's policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum GitMarker {
    /// `.git` is a directory: a normal clone.
    Directory,
    /// `.git` is a file, or anything else that is not a real directory: a
    /// worktree, a submodule — or a *symlink*, even one pointing at a directory.
    /// The entry is inspected without following links, because a link's target
    /// is not evidence about the directory the link sits in.
    File,
}

/// A config file located by walking up from a starting directory.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct Located {
    /// Full path to the config file.
    pub path: PathBuf,
    /// Directory the file was found in.
    pub directory: PathBuf,
    /// The `.git` entry beside the file, if any.
    pub git_marker: Option<GitMarker>,
    /// Whether any `.git` entry sits beside the file. The lenient reading of
    /// "at the repository root"; see [`Located::beside_git_directory`] for the
    /// strict one.
    pub at_repo_root: bool,
}

impl Located {
    /// The strict placement test: only an adjacent `.git` *directory* counts.
    #[must_use]
    pub fn beside_git_directory(&self) -> bool {
        self.git_marker == Some(GitMarker::Directory)
    }

    /// Human-readable explanation of why a non-root location is suspect.
    ///
    /// Returns `None` when the file sits at a repository root, which is the
    /// expected case and needs no explanation.
    #[must_use]
    pub fn misplacement_warning(&self, file_name: &str) -> Option<String> {
        if self.at_repo_root {
            return None;
        }
        Some(format!(
            "{file_name} was located at {}, which is not a repository root (no .git beside it); \
             it will still be used, but it shadows any copy at the repository root and is \
             probably not the file you meant to edit",
            self.directory.display()
        ))
    }
}

/// Why discovery refused to continue.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum DiscoveryError {
    /// The name contained a path separator, or was empty. Only a bare file
    /// name is a config name; anything else would turn discovery into an
    /// arbitrary path probe.
    NotAFileName(String),
    /// A candidate at this path is a symbolic link, which is refused.
    Symlink(PathBuf),
    /// A candidate exists but is not a regular file, and the search was
    /// configured to refuse rather than skip it.
    NotRegularFile(PathBuf),
    /// A candidate could not be inspected, for a reason other than absence.
    Unreadable {
        /// The candidate path.
        path: PathBuf,
        /// The I/O error kind, so a caller can rebuild an `io::Error`.
        kind: std::io::ErrorKind,
    },
}

impl fmt::Display for DiscoveryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotAFileName(name) => write!(f, "{name:?} is not a bare config file name"),
            Self::Symlink(path) => write!(f, "refusing symlinked config at {}", path.display()),
            Self::NotRegularFile(path) => {
                write!(f, "config at {} is not a regular file", path.display())
            }
            Self::Unreadable { path, kind } => {
                write!(f, "cannot inspect config at {}: {kind}", path.display())
            }
        }
    }
}

impl std::error::Error for DiscoveryError {}

/// A configured upward search for one config file name.
///
/// ```no_run
/// # fn main() -> Result<(), ores_config_discovery::DiscoveryError> {
/// let located = ores_config_discovery::Search::new(".ores-rl.toml")
///     .refuse_non_regular(true)
///     .from_cwd()?;
/// # Ok(()) }
/// ```
#[derive(Clone, Debug)]
pub struct Search<'a> {
    file_name: &'a str,
    bound: Option<PathBuf>,
    refuse_non_regular: bool,
}

impl<'a> Search<'a> {
    /// A search for `file_name` with no fallback bound: outside any repository
    /// the walk continues to the filesystem root (capped at [`MAX_ANCESTORS`]).
    #[must_use]
    pub fn new(file_name: &'a str) -> Self {
        Self {
            file_name,
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

    /// Ends the walk at `$HOME`, when it is set, as flags-2-env does.
    #[must_use]
    pub fn bound_at_home(mut self) -> Self {
        self.bound = std::env::var_os("HOME").map(PathBuf::from);
        self
    }

    /// When set, a candidate that exists but is not a regular file (a
    /// directory named like the config, a socket) is an error instead of being
    /// skipped. Symlinks are refused either way.
    #[must_use]
    pub fn refuse_non_regular(mut self, refuse: bool) -> Self {
        self.refuse_non_regular = refuse;
        self
    }

    /// Runs the search from the current working directory.
    ///
    /// # Errors
    ///
    /// See [`Search::from`]. An unreadable working directory is `Ok(None)`.
    pub fn from_cwd(&self) -> Result<Option<Located>, DiscoveryError> {
        let Ok(start) = std::env::current_dir() else {
            return Ok(None);
        };
        self.from(&start)
    }

    /// Runs the search at or above `start`. When `start` is a file, the walk
    /// begins in its directory.
    ///
    /// # Errors
    ///
    /// Fails when the name is not a bare file name, when a candidate is a
    /// symlink or cannot be inspected, or — with [`Search::refuse_non_regular`]
    /// — when a candidate is not a regular file. A config that is simply
    /// absent is `Ok(None)`, because what absence means is the caller's call.
    pub fn from(&self, start: &Path) -> Result<Option<Located>, DiscoveryError> {
        let file_name = self.file_name;
        if file_name.is_empty() || file_name.contains(['/', '\\']) {
            return Err(DiscoveryError::NotAFileName(file_name.to_owned()));
        }
        // Canonicalise so the ancestor chain is the real one, not one routed
        // through a symlinked working directory; keep the given path if it
        // does not exist, so the outcome is "absent" rather than a panic.
        let canonical = std::fs::canonicalize(start).unwrap_or_else(|_| start.to_path_buf());
        let start = if canonical.is_file() {
            canonical
                .parent()
                .map_or(canonical.clone(), Path::to_path_buf)
        } else {
            canonical
        };
        // The bound must be compared in the same canonical form as the
        // ancestor chain. `$HOME` often reaches its directory through a
        // symlink; compared raw, it would never equal a canonical ancestor and
        // the walk would escape it.
        let bound = self
            .bound
            .as_deref()
            .map(|limit| std::fs::canonicalize(limit).unwrap_or_else(|_| limit.to_path_buf()));

        for directory in start.ancestors().take(MAX_ANCESTORS) {
            let candidate = directory.join(file_name);
            let marker = git_marker(directory);
            match std::fs::symlink_metadata(&candidate) {
                Ok(meta) if meta.file_type().is_symlink() => {
                    return Err(DiscoveryError::Symlink(candidate));
                }
                Ok(meta) if meta.is_file() => {
                    return Ok(Some(Located {
                        path: candidate,
                        directory: directory.to_path_buf(),
                        git_marker: marker,
                        at_repo_root: marker.is_some(),
                    }));
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
            // The boundary directory itself was searched above; a missing file
            // there ends the walk, so nothing above the repository governs it.
            if marker.is_some() || bound.as_deref().is_some_and(|limit| directory == limit) {
                break;
            }
        }
        Ok(None)
    }
}

/// The `.git` entry in `directory`, if any. Inspected without following
/// symlinks, so a dangling or symlinked `.git` still marks a boundary.
fn git_marker(directory: &Path) -> Option<GitMarker> {
    let meta = std::fs::symlink_metadata(directory.join(".git")).ok()?;
    Some(if meta.is_dir() {
        GitMarker::Directory
    } else {
        GitMarker::File
    })
}

/// Locates the nearest `file_name` from the working directory, bounded by
/// `$HOME` outside any repository.
///
/// # Errors
///
/// See [`Search::from`].
pub fn locate(file_name: &str) -> Result<Option<Located>, DiscoveryError> {
    Search::new(file_name).bound_at_home().from_cwd()
}

/// Locates the nearest `file_name` at or above `start`, bounded by `$HOME`
/// outside any repository.
///
/// # Errors
///
/// See [`Search::from`].
pub fn locate_from(start: &Path, file_name: &str) -> Result<Option<Located>, DiscoveryError> {
    Search::new(file_name).bound_at_home().from(start)
}

/// [`locate_from`] with an explicit fallback bound instead of `$HOME`; `None`
/// walks to the filesystem root (still capped at [`MAX_ANCESTORS`]).
///
/// # Errors
///
/// See [`Search::from`].
pub fn locate_bounded(
    start: &Path,
    file_name: &str,
    bound: Option<&Path>,
) -> Result<Option<Located>, DiscoveryError> {
    let search = Search::new(file_name);
    match bound {
        Some(limit) => search.bound(limit).from(start),
        None => search.from(start),
    }
}

/// Locates the nearest `file_name` and reports a misplaced one through the
/// fleet logger.
///
/// For libraries without a logger and event schema of their own. Without the
/// `otel-warning` feature the warning goes to stderr instead, so the diagnostic
/// is never simply lost.
///
/// # Errors
///
/// See [`Search::from`].
pub fn locate_and_report(file_name: &str) -> Result<Option<Located>, DiscoveryError> {
    let located = locate(file_name)?;
    warn_if_misplaced(located.as_ref(), file_name);
    Ok(located)
}

/// [`locate_and_report`] from an explicit starting directory.
///
/// # Errors
///
/// See [`Search::from`].
pub fn locate_and_report_from(
    start: &Path,
    file_name: &str,
) -> Result<Option<Located>, DiscoveryError> {
    let located = locate_from(start, file_name)?;
    warn_if_misplaced(located.as_ref(), file_name);
    Ok(located)
}

fn warn_if_misplaced(located: Option<&Located>, file_name: &str) {
    if let Some(warning) = located.and_then(|found| found.misplacement_warning(file_name)) {
        report(&warning);
    }
}

/// Emits a warning through the ores-otel logger when it is compiled in.
///
/// The logger is constructed per call: discovery runs once at startup, and a
/// long-lived logger here would outlive the configuration it was reporting on.
/// If the logger itself cannot send, the message still reaches stderr — a
/// warning about configuration must not depend on telemetry being healthy.
#[cfg(feature = "otel-warning")]
fn report(message: &str) {
    // The package is `oresoftware-next-loggers`; its library target is `next_loggers`.
    use next_loggers::{Logger, Options};
    let logger = Logger::new(Options {
        app_name: "ores-config-discovery".to_owned(),
        console: true,
        ..Options::default()
    });
    let event = logger.warn(vec![message.into()]);
    if event.send().is_err() {
        eprintln!("warning: {message}");
    }
    let _ = logger.flush(true);
}

/// Fallback used when the telemetry stack is compiled out.
#[cfg(not(feature = "otel-warning"))]
fn report(message: &str) {
    eprintln!("warning: {message}");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A throwaway directory tree; removed when the guard drops.
    struct Tree(PathBuf);

    impl Tree {
        fn new(tag: &str) -> Self {
            static COUNTER: AtomicUsize = AtomicUsize::new(0);
            let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir().join(format!(
                "ores-config-discovery-{tag}-{}-{unique}",
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

    /// Search with no fallback bound; the temp tree lives outside `$HOME` anyway.
    fn search(start: &Path, name: &str) -> Option<Located> {
        Search::new(name).from(start).expect("no discovery error")
    }

    #[test]
    fn locates_a_config_at_the_repository_root_from_a_nested_directory() {
        let tree = Tree::new("root");
        tree.file(".git/HEAD");
        let config = tree.file(".ores-mw.toml");
        let deep = tree.dir("crates/server/src");

        let found = search(&deep, ".ores-mw.toml").expect("config is located");
        assert_eq!(found.path, config);
        assert!(found.at_repo_root, "a .git beside it makes it a root");
        assert!(found.beside_git_directory());
        assert!(found.misplacement_warning(".ores-mw.toml").is_none());
    }

    #[test]
    fn the_nearest_config_wins() {
        let tree = Tree::new("nearest");
        tree.file(".git/HEAD");
        tree.file(".ores-rl.toml");
        let nested = tree.file("service/.ores-rl.toml");

        let found = search(&tree.dir("service/src"), ".ores-rl.toml").expect("config is located");
        assert_eq!(found.path, nested, "a nested override takes precedence");
    }

    #[test]
    fn a_config_outside_a_repository_root_is_reported() {
        let tree = Tree::new("stray");
        tree.file(".git/HEAD");
        tree.file("service/.ores-lru.toml");

        let found = search(&tree.dir("service/src"), ".ores-lru.toml").expect("config is located");
        assert!(!found.at_repo_root, "no .git beside it");
        assert_eq!(found.git_marker, None);
        let warning = found
            .misplacement_warning(".ores-lru.toml")
            .expect("a misplaced config warns");
        assert!(warning.contains("not a repository root"));
        assert!(
            warning.contains("will still be used"),
            "the warning must say the file is honoured, or a reader will assume it was ignored"
        );
    }

    #[test]
    fn a_missing_config_is_absent_rather_than_an_error() {
        let tree = Tree::new("missing");
        assert!(search(&tree.dir("a/b"), ".auth-shared.toml").is_none());
    }

    #[test]
    fn a_non_regular_candidate_is_skipped_by_default_and_refused_on_request() {
        let tree = Tree::new("dir");
        tree.file(".git/HEAD");
        tree.dir(".opto-sync.toml");
        let sub = tree.dir("sub");
        assert!(
            search(&sub, ".opto-sync.toml").is_none(),
            "skipped by default"
        );
        assert!(matches!(
            Search::new(".opto-sync.toml")
                .refuse_non_regular(true)
                .from(&sub),
            Err(DiscoveryError::NotRegularFile(_))
        ));
    }

    #[test]
    fn a_worktree_git_file_is_a_root_leniently_but_not_strictly() {
        // In a worktree or submodule, .git is a file pointing at the real
        // gitdir. It ends the walk and counts as a root for lenient callers;
        // strict callers can tell the difference.
        let tree = Tree::new("worktree");
        tree.file(".git");
        tree.file(".ores-otel.toml");
        let found = search(&tree.dir("src"), ".ores-otel.toml").expect("config is located");
        assert!(found.at_repo_root);
        assert_eq!(found.git_marker, Some(GitMarker::File));
        assert!(!found.beside_git_directory());
    }

    #[test]
    fn the_walk_never_crosses_a_git_boundary_of_either_kind() {
        let tree = Tree::new("boundary");
        tree.file(".git/HEAD");
        tree.file(".ores-compose.yaml");
        tree.file("vendor/inner/.git/HEAD");
        tree.file("vendor/worktree/.git");
        assert!(search(&tree.dir("vendor/inner/src"), ".ores-compose.yaml").is_none());
        assert!(search(&tree.dir("vendor/worktree/src"), ".ores-compose.yaml").is_none());
    }

    #[test]
    fn a_start_that_is_a_file_begins_in_its_directory() {
        let tree = Tree::new("startfile");
        tree.file(".git/HEAD");
        let config = tree.file(".ores-rl.toml");
        let source = tree.file("src/main.rs");
        assert_eq!(
            search(&source, ".ores-rl.toml").expect("located").path,
            config
        );
    }

    #[test]
    fn outside_any_repository_the_walk_stops_at_the_bound() {
        // Matches flags-2-env's "before HOME" rule. The bound itself is searched.
        let tree = Tree::new("bound");
        tree.file(".ores-compose.yaml");
        let home = tree.dir("home/user");
        let project = tree.dir("home/user/proj");

        assert_eq!(
            locate_bounded(&project, ".ores-compose.yaml", Some(&home)),
            Ok(None)
        );
        tree.file("home/user/.ores-compose.yaml");
        assert!(locate_bounded(&project, ".ores-compose.yaml", Some(&home))
            .expect("no discovery error")
            .is_some());
    }

    #[test]
    fn a_path_is_not_a_config_name() {
        // Discovery must not be usable as an arbitrary path probe.
        let tree = Tree::new("path");
        tree.file(".git/HEAD");
        tree.file("secret/.ores-mw.toml");
        for bad in ["secret/.ores-mw.toml", "../.ores-mw.toml", ""] {
            assert!(matches!(
                Search::new(bad).from(&tree.0),
                Err(DiscoveryError::NotAFileName(_))
            ));
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_git_directory_is_a_boundary_but_not_root_placement() {
        // `.git` is inspected without following symlinks, so a `.git` symlink
        // that points at a real directory still ends the walk but is NOT
        // `GitMarker::Directory`. Strict-placement consumers (opto-sync, the
        // OTel SDK) previously used `join(".git").is_dir()`, which followed the
        // link; this is the deliberate, security-conservative replacement: a
        // link's target is not evidence about the directory it sits in.
        let tree = Tree::new("gitlink");
        tree.file(".ores-rl.toml");
        tree.file("realgit/HEAD");
        let repo = tree.dir("repo");
        std::os::unix::fs::symlink(tree.0.join("realgit"), repo.join(".git")).expect("symlink");
        tree.file("repo/.opto-sync.toml");

        let found = search(&tree.dir("repo/src"), ".opto-sync.toml").expect("located");
        assert_eq!(found.git_marker, Some(GitMarker::File));
        assert!(found.at_repo_root, "lenient callers still see a root");
        assert!(!found.beside_git_directory(), "strict callers do not");
        // And it is a boundary: the config above the linked repo stays invisible.
        assert!(search(&tree.dir("repo/src"), ".ores-rl.toml").is_none());
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_home_still_bounds_the_walk() {
        // $HOME commonly reaches its directory through a symlink. The start is
        // canonicalised, so a raw bound would never match and the walk would
        // escape HOME and adopt a config above it.
        let tree = Tree::new("homelink");
        tree.file(".ores-rl.toml");
        let project = tree.dir("realhome/proj");
        let link = tree.0.join("homelink");
        std::os::unix::fs::symlink(tree.0.join("realhome"), &link).expect("symlink");
        assert_eq!(
            locate_bounded(&project, ".ores-rl.toml", Some(&link)),
            Ok(None)
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_config_is_refused() {
        // A file that points elsewhere is not "the config at this path".
        let tree = Tree::new("symlink");
        tree.file(".git/HEAD");
        let real = tree.file("elsewhere/real.toml");
        std::os::unix::fs::symlink(&real, tree.0.join(".ores-rl.toml")).expect("symlink");
        assert!(matches!(
            Search::new(".ores-rl.toml").from(&tree.dir("src")),
            Err(DiscoveryError::Symlink(_))
        ));
    }
}
