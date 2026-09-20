# Consumer pin conformance

`consumer-pins.toml` is the machine-readable registry of reviewed Rust consumers
that rely on this crate's traversal/security semantics.

The registry records the **code-authority revision**, not whatever commit happens
to be at `main`. Documentation-only commits must not create fleet-wide repin
churn. A new code-authority revision is warranted when traversal, boundary,
error, security, feature, or MSRV behavior changes and the change has passed the
crate's own gates.

For every registered consumer, a rollout audit should verify:

1. the manifest pins the exact `code_authority_revision`;
2. `default-features = false` remains present where registered, so the optional
   OTel warning feature cannot unexpectedly raise the consumer's MSRV or create
   a self-dependency;
3. `lock_policy = "cargo-generated"` consumers regenerate their lockfile with
   Cargo under the repository's supported Rust toolchain rather than editing it
   by hand;
4. the generated lock delta is limited to what the resolver requires;
5. the consumer's runtime/config/contract tests pass, not merely `cargo check`;
6. repository-specific policy stays in the consumer while ancestor traversal,
   Git boundaries, canonicalization, symlink refusal, and path failures stay in
   `ores-config-discovery`.

Adding a new consumer should add a registry entry in the same PR that adopts the
shared primitive. Removing a consumer should explain where config discovery
moved; silently deleting an entry makes fleet audits incomplete.

The current code authority is `1ffd94663a110f77fcd5e1143fd10f89b1f284e0`
(v0.2.2, merged in #7). The later README-only #8 commit intentionally does not
change that authority revision.
