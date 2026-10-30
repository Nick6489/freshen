# Contributing to Freshen

Explain the problem, the resulting behavior, and the reason for the approach in each commit. Include relevant validation in the commit body. Keep changes focused and preserve the GUI-independent API boundary.

Before submitting implementation changes, run:

```text
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --all-targets --locked
cargo test --workspace --doc --locked
cargo package --locked -p freshen
```

Tests use temporary installations and test-only signing keys. Never exercise an updater test against a real application installation. Place dependency caches and build output wherever your machine has room; those paths are local environment settings and must not be committed.

The [testing and integration guide](TESTING.md) lists binding-specific checks and
the validation that needs a disposable copy of a real packaged application.
Documentation-only changes need link, heading, and diff checks; rerun code checks
when examples or implementation behavior change.

Changes to package ownership, archive entry types, signing, subprocess handling, or recovery must include failure cases as well as successful updates. Preserve backward compatibility for signed metadata and persisted journals, or explicitly version the protocol and document migration.

For vulnerabilities, follow the [security reporting policy](SECURITY.md).
