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
//! Two boundaries stop the walk, and they exist for the same reason: a config
//! above the repository belongs to nobody, and letting it in would let an
//! unrelated parent checkout or a home-directory file govern this process.
//!
//! * The walk ends at the first **repository root** (a directory holding a
//!   `.git` entry). That directory is searched; its parents are not.
//! * Outside any repository, the walk ends at `$HOME`, matching what flags-2-env
//!   already does for `.cli-flags.toml`.
//!
//! The catch is that "nearest" and "correct" are not the same thing. A config
//! found somewhere other than the repository root is usually a mistake — a
//! stray copy in a subdirectory that now shadows the real one. Discovery still
//! honours that file (refusing would be worse: the process would silently fall
//! back to defaults), but it reports the situation so the mistake is visible
//! rather than mysterious.
//!
//! A symlinked candidate is refused outright. A config file that points
//! somewhere else is not "the config at this path", and following it would
//! defeat the boundaries above.

#![forbid(unsafe_code)]

use std::fmt;
use std::path::{Path, PathBuf};

/// Ancestors examined before giving up, so a pathological path cannot turn
/// discovery into an unbounded scan.
pub const MAX_ANCESTORS: usize = 64;

/// A config file located by walking up from a starting directory.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Located {
    /// Full path to the config file.
    pub path: PathBuf,
    /// Directory the file was found in.
    pub directory: PathBuf,
    /// Whether that directory is a repository root (a `.git` entry beside it).
    pub at_repo_root: bool,
}

impl Located {
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
pub enum DiscoveryError {
    /// The name contained a path separator, or was empty. Only a bare file
    /// name is a config name; anything else would turn discovery into an
    /// arbitrary path probe.
    NotAFileName(String),
    /// A candidate at this path is a symbolic link, which is refused.
    Symlink(PathBuf),
    /// A candidate could not be inspected, for a reason other than absence.
    Unreadable(PathBuf),
}

impl fmt::Display for DiscoveryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotAFileName(name) => write!(f, "{name:?} is not a bare config file name"),
            Self::Symlink(path) => write!(f, "refusing symlinked config at {}", path.display()),
            Self::Unreadable(path) => write!(f, "cannot inspect config at {}", path.display()),
        }
    }
}

impl std::error::Error for DiscoveryError {}

/// Locates the nearest `file_name`, starting at the current working directory.
///
/// # Errors
///
/// Returns an error when the name is not a bare file name, or when a candidate
/// is a symlink or cannot be inspected. A config that is simply absent is
/// `Ok(None)`, because absence is a normal state that callers resolve with
/// defaults.
pub fn locate(file_name: &str) -> Result<Option<Located>, DiscoveryError> {
    let Ok(start) = std::env::current_dir() else {
        return Ok(None);
    };
    locate_from(&start, file_name)
}

/// Locates the nearest `file_name` at or above `start`.
///
/// The first match wins. The walk stops after searching the first repository
/// root it meets, or `$HOME` when no repository root is met first.
///
/// # Errors
///
/// See [`locate`].
pub fn locate_from(start: &Path, file_name: &str) -> Result<Option<Located>, DiscoveryError> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    locate_bounded(start, file_name, home.as_deref())
}

/// [`locate_from`] with an explicit fallback bound instead of `$HOME`.
///
/// A repository root still ends the walk first when one is met; `bound` only
/// applies outside any repository. Passing `None` walks to the filesystem root
/// (still capped at [`MAX_ANCESTORS`]).
///
/// # Errors
///
/// See [`locate`].
pub fn locate_bounded(
    start: &Path,
    file_name: &str,
    bound: Option<&Path>,
) -> Result<Option<Located>, DiscoveryError> {
    if file_name.is_empty() || file_name.contains(['/', '\\']) {
        return Err(DiscoveryError::NotAFileName(file_name.to_owned()));
    }
    // Canonicalise so the ancestor chain is the real one, not one routed
    // through a symlinked working directory; keep the given path if it does
    // not exist, so the error surfaces as "absent" rather than as a panic.
    let start = std::fs::canonicalize(start).unwrap_or_else(|_| start.to_path_buf());
    // The bound must be compared in the same canonical form as the ancestor
    // chain. `$HOME` often reaches its directory through a symlink; compared
    // raw, it would never equal a canonical ancestor and the walk would escape it.
    let bound =
        bound.map(|limit| std::fs::canonicalize(limit).unwrap_or_else(|_| limit.to_path_buf()));

    for directory in start.ancestors().take(MAX_ANCESTORS) {
        let candidate = directory.join(file_name);
        match std::fs::symlink_metadata(&candidate) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(DiscoveryError::Symlink(candidate));
            }
            Ok(meta) if meta.is_file() => {
                return Ok(Some(Located {
                    path: candidate,
                    at_repo_root: is_repo_root(directory),
                    directory: directory.to_path_buf(),
                }));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(DiscoveryError::Unreadable(candidate)),
        }
        // The root itself was searched above; a missing file there ends the
        // walk, so nothing above the repository can govern it.
        if is_repo_root(directory) || bound.as_deref().is_some_and(|limit| directory == limit) {
            break;
        }
    }
    Ok(None)
}

/// A directory is a repository root when it holds a `.git` entry — a directory
/// for a normal checkout, a file for a worktree.
fn is_repo_root(directory: &Path) -> bool {
    directory.join(".git").exists()
}

/// Locates the nearest `file_name` and reports a misplaced one through the
/// fleet logger.
///
/// This is what a library should call: it behaves exactly like [`locate`], and
/// additionally emits a warning when the file was not found at a repository
/// root. Without the `otel-warning` feature the warning goes to stderr instead,
/// so the diagnostic is never simply lost.
///
/// # Errors
///
/// See [`locate`].
pub fn locate_and_report(file_name: &str) -> Result<Option<Located>, DiscoveryError> {
    let located = locate(file_name)?;
    if let Some(warning) = located
        .as_ref()
        .and_then(|found| found.misplacement_warning(file_name))
    {
        report(&warning);
    }
    Ok(located)
}

/// [`locate_and_report`] from an explicit starting directory.
///
/// # Errors
///
/// See [`locate`].
pub fn locate_and_report_from(
    start: &Path,
    file_name: &str,
) -> Result<Option<Located>, DiscoveryError> {
    let located = locate_from(start, file_name)?;
    if let Some(warning) = located
        .as_ref()
        .and_then(|found| found.misplacement_warning(file_name))
    {
        report(&warning);
    }
    Ok(located)
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

    /// Search with no `$HOME` bound; the temp tree lives outside it anyway.
    fn search(start: &Path, name: &str) -> Option<Located> {
        locate_bounded(start, name, None).expect("no discovery error")
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
    fn a_directory_named_like_the_config_is_not_a_match() {
        let tree = Tree::new("dir");
        tree.file(".git/HEAD");
        tree.dir(".opto-sync.toml");
        assert!(
            search(&tree.dir("sub"), ".opto-sync.toml").is_none(),
            "only a regular file counts"
        );
    }

    #[test]
    fn a_worktree_git_file_still_counts_as_a_root() {
        // In a git worktree, .git is a file pointing at the real gitdir.
        let tree = Tree::new("worktree");
        tree.file(".git");
        tree.file(".ores-otel.toml");
        let found = search(&tree.dir("src"), ".ores-otel.toml").expect("config is located");
        assert!(found.at_repo_root, "a .git file marks a worktree root");
    }

    #[test]
    fn the_walk_never_crosses_a_repository_boundary() {
        // A parent checkout must not govern a nested repository. The config
        // sits above the inner repo's root and must stay invisible to it.
        let tree = Tree::new("boundary");
        tree.file(".git/HEAD");
        tree.file(".ores-compose.yaml");
        tree.file("vendor/inner/.git/HEAD");
        assert!(search(&tree.dir("vendor/inner/src"), ".ores-compose.yaml").is_none());
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
                locate_bounded(&tree.0, bad, None),
                Err(DiscoveryError::NotAFileName(_))
            ));
        }
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
            locate_bounded(&tree.dir("src"), ".ores-rl.toml", None),
            Err(DiscoveryError::Symlink(_))
        ));
    }
}
