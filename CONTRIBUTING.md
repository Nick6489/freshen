# Contributing to Freshen

Explain the problem, the resulting behavior, and the reason for the approach in each commit. Include relevant validation in the commit body. Keep changes focused and preserve the GUI-independent API boundary.

Before submitting implementation changes, run:

```text
cargo fmt --all -- --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --all-targets --locked
cargo test --doc --locked
cargo package --locked
```

Tests use temporary installations and test-only signing keys. Never exercise an updater test against a real application installation. Place dependency caches and build output wherever your machine has room; those paths are local environment settings and must not be committed.

Changes to package ownership, archive entry types, signing, subprocess handling, or recovery must include failure cases as well as successful updates. Preserve backward compatibility for signed metadata and persisted journals, or explicitly version the protocol and document migration.

For vulnerabilities, follow the [security reporting policy](SECURITY.md).
