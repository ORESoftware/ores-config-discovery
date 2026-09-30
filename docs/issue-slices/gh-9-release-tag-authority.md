# release tag authority

Driver: `ORESoftware/ores-config-discovery#9`

This document defines one bounded, independently reviewable contract slice. It advances the driver issue without claiming full implementation.

## Invariants

- Create the release tag only from the reviewed hardened code-authority commit.
- Do not retag or move an existing published tag.
- Record the exact commit and verification evidence used for promotion.
- Consumers should pin immutable release identity rather than a mutable branch.

## Verification

- Test/verify the exact PR head.
- Preserve fail-closed behavior for malformed or untrusted inputs.
- Treat skipped/zero-step CI as missing evidence.
- Bind release or runtime evidence to immutable source identity.

## Non-goals

This slice does not add secrets, weaken repository protection, or silently rewrite consumer state.
