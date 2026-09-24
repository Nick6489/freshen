# Security policy

Freshen is pre-1.0 software. Treat installation and recovery changes as security-sensitive: they replace executable code.

Report vulnerabilities privately using [GitHub security advisories](https://github.com/Nick6489/freshen/security/advisories/new). Include the affected version, platform, and a minimal reproduction without production signing keys.

## Trust boundaries

Publisher public keys and local installation ownership policy are supplied by the host application. HTTPS discovery metadata is not sufficient authorization. Freshen verifies a detached Ed25519 signature before parsing the manifest, then verifies the signed package and file hashes before replacement. The helper rechecks staged files after the application exits.

The helper and journal run with the application's user privileges. Protect the installation directory and helper from modification by other users. Recovery metadata is not an independent security boundary against another process running as the same user. Never invoke the helper with elevated privileges solely to work around destination permissions.

Network transport enforces HTTPS including redirects, checks HTTP errors, limits downloaded bytes, and uses platform certificate validation. A custom Transport is trusted application code and must uphold its size and cancellation contract.

Archive extraction rejects traversal, alternate streams, reserved names, case collisions, unknown files, special files, and symlinks. Schema 1 deliberately limits path syntax and archive entry types; broadening those requires additional tests.

Signature verification provides authenticity, not freshness. Current-version comparison rejects downgrades, but this initial protocol has no signed expiration or independent mechanism to detect a server withholding releases. Keep signing keys offline or in a protected release environment and plan rotation before removing an old key.
