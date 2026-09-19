# ores-config-discovery

Finds the nearest fleet config file — `.ores-mw.toml`, `.ores-rl.toml`,
`.ores-lru.toml`, `.auth-shared.toml`, `.opto-sync.toml`, `.ores-otel.toml`,
and the rest — by walking up from the working directory, so a library reads
the same configuration no matter which subdirectory its process started in.

```rust
let located = ores_config_discovery::locate_and_report(".ores-rl.toml")?;
```

## Rules

- **Nearest wins.** The first match walking upward is used; a nested copy
  overrides one at the root.
- **The walk never crosses a repository boundary.** The first directory holding
  a `.git` entry (directory, worktree file, or symlink marker) is searched and
  then the walk ends. Outside any repository it ends at `$HOME`, matching
  flags-2-env.
- **A config that is not at the repository root is used, and reported.** The
  warning goes through the ores-otel logger (`oresoftware-next-loggers`) when
  the opt-in `otel-warning` feature is enabled, and to stderr otherwise — it is
  never silently dropped.
- **Symlinked candidates are refused.** A file that points elsewhere is not the
  config at that path.
- **Only a bare file name is accepted.** A separator turns discovery into a path
  probe, so it is rejected.

## Features and MSRV

The default feature set is empty and supports the crate's declared Rust 1.78
MSRV. `otel-warning` is explicitly opt-in because the pinned fleet logger
currently requires Rust 1.88. Consumers that already own a logger should keep
`default-features = false` (or use the empty defaults) and emit their own
structured warning from `Located` metadata. Consumers that enable
`otel-warning` intentionally accept the Rust 1.88 feature floor.
