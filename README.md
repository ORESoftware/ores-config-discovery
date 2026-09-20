# ores-config-discovery

Finds the nearest fleet config file — `.ores-mw.toml`, `.ores-rl.toml`,
`.ores-lru.toml`, `.auth-shared.toml`, `.opto-sync.toml`, `.ores-otel.toml`,
and the rest — by walking up from the working directory, so a library reads
the same configuration no matter which subdirectory its process started in.

For normal library code, prefer one single-file lookup per config concern:

```rust
let located = ores_config_discovery::locate_and_report(".ores-rl.toml")?;
```

The single-file `Search` / `locate` APIs are intentionally the preferred path.
Each library keeps ownership of what its missing config means and how its
config-specific errors/warnings are handled.

## Batch discovery

Inventory/startup tooling that deliberately owns several config concerns can
scan them in one ancestor walk:

```rust
let results = ores_config_discovery::discover_many(&[
    ".ores-rl.toml",
    ".ores-otel.toml",
    ".ores-compose.yaml",
])?;
```

Or use `BatchSearch` when the caller needs explicit bounds or strict
non-regular-file policy:

```rust
let names = [".ores-rl.toml", ".ores-otel.toml"];
let results = ores_config_discovery::BatchSearch::new(&names)
    .refuse_non_regular(true)
    .from(&start)?;
```

**Prefer the single-file API unless one caller really owns the whole batch.**
The batch API is probably more complicated to use because missing/error policy
for several independent config concerns becomes coupled to one caller. Batch
lookup is an additive optimization, not a singleton/global registry and not a
replacement for per-library discovery.

Batch lookup preserves input order (including duplicate names), applies
nearest-wins independently to each name, walks ancestors only once, probes the
Git boundary once per directory, and fails the whole batch if any unresolved
entry hits an unsafe or unreadable path.

## Rules

- **Nearest wins.** The first match walking upward is used; a nested copy
  overrides one at the root.
- **The walk never crosses a repository boundary.** The first directory holding
  a `.git` entry (directory, worktree file, or symlink marker) is searched and
  then the walk ends. Outside any repository the convenience APIs end at
  `$HOME`, matching flags-2-env.
- **Traversal failures fail closed.** If the start path, explicit/HOME bound, or
  current working directory cannot be resolved, discovery returns an error.
  Only a real absence of the requested config is represented as `Ok(None)`;
  path-resolution failures never masquerade as "config missing".
- **A config that is not at the repository root is used, and reported.** The
  warning goes through the ores-otel logger (`oresoftware-next-loggers`) when
  the opt-in `otel-warning` feature is enabled, and to stderr otherwise — it is
  never silently dropped.
- **Symlinked candidates are refused.** A file that points elsewhere is not the
  config at that path.
- **Non-regular candidates are caller policy.** Default discovery skips a
  directory/socket/etc. named like the config and keeps walking;
  `Search::refuse_non_regular(true)` and
  `BatchSearch::refuse_non_regular(true)` make such a candidate an error.
- **Only a bare file name is accepted.** A separator, prefix, `.` or `..` turns
  discovery into a path probe, so it is rejected before traversal.
- **The walk is bounded.** At most 64 ancestors are examined.

## Features and MSRV

The default feature set is empty and supports the crate's declared Rust 1.78
MSRV. `otel-warning` is explicitly opt-in because the pinned fleet logger
currently requires Rust 1.88. Consumers that already own a logger should keep
`default-features = false` (or use the empty defaults) and emit their own
structured warning from `Located` metadata. Consumers that enable
`otel-warning` intentionally accept the Rust 1.88 feature floor.

The package is currently `0.2.2`. Fleet consumers pin an immutable Git revision
so a later traversal/security change cannot silently alter a fresh resolution;
repins should be reviewed and tested in each consumer's own runtime/contract
suite.
