# Freshen

Freshen is a Rust library for authenticated desktop application updates without a GUI of its own. Applications retain control of their windows, scheduling, prompts, unsaved work, and shutdown.

Development is in progress. The planned lifecycle is discover, download, authenticate, validate, prepare, hand off, replace, restart, confirm, and clean up. Windows and Linux portable installations use an explicit list of owned files; macOS installations use application bundles and publisher identity verification.

Every implementation increment is committed with an explanation of what changed and why. Crates.io publication is separate from development of this repository.

The design draws on Andre Louis's [ElevenLabs Music Generator updater](https://github.com/OnjLouis/ElevenLabsMusicGenerator) and [Clipman updater](https://github.com/OnjLouis/Clipman). Freshen is an independent Rust implementation.

