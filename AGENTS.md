# Freshen development instructions

- Keep the library independent of every GUI toolkit. Return structured events and errors; the host owns scheduling, prompts, saving work, and shutdown.
- Commit each completed, coherent change and push it to this repository. Include a commit body explaining what changed and why, with relevant validation. Do not leave implementation changes uncommitted at the end of a task.
- Never commit signing keys, credentials, downloaded application packages, or machine-specific build-cache settings.
- Preserve publisher authentication, explicit local ownership, bounded extraction, helper readiness, and recoverable transaction states. Do not weaken these to make a test pass.
- Tests must exercise behavior and failure recovery. Run formatting, strict Clippy, relevant tests, documentation tests, and crate packaging for implementation changes. Native CI covers Windows, Linux, and macOS; a real Developer ID-signed bundle test needs separate signing credentials.
- Do not publish to crates.io or create a production release without an explicit request.

