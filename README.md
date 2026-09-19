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
  a `.git` entry (directory, or worktree file) is searched and then the walk
  ends. Outside any repository it ends at `$HOME`, matching flags-2-env.
- **A config that is not at the repository root is used, and reported.** The
  warning goes through the ores-otel logger (`oresoftware-next-loggers`) when
  the default `otel-warning` feature is on, and to stderr otherwise — it is
  never silently dropped.
- **Symlinked candidates are refused.** A file that points elsewhere is not the
  config at that path.
- **Only a bare file name is accepted.** A separator turns discovery into a path
  probe, so it is rejected.

## Features

`otel-warning` (default) pulls in the fleet logger. Libraries that do not want
the telemetry stack use `default-features = false` and act on
`Located::at_repo_root` themselves.
