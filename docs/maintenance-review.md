# Maintenance review — 2026-10-07

## Scope

Reviewed the source ZIP inside the supplied DaggerConnect-4.2.8-stable-resync archive. Package metadata identifies dagger-rs 0.2.1, an independent Rust implementation informed by the original 4.2.8 core. Original authorship, MIT license and dependency notices remain intact. Historical test reports in this tree are supplied evidence, not tests executed during this review.

## Confirmed installer defect and fix

Before the change, placing a symlink at PREFIX/bin/dagger-setup caused the shell redirection to truncate its unrelated target. The subsequent chmod also changed the target's permissions. This was reproduced using a harmless executable fixture and a disposable prefix.

The installer now writes the launcher into a mktemp file within the destination directory, sets permissions on that file, and replaces the destination using mv -fT. This replaces a destination symlink instead of following it, avoids partial launcher contents, and rejects a destination directory. The EXIT trap cleans up a failed replacement. Installation directories must still be controlled by the administrator; this is not a comprehensive installer security audit.

## Other changes

- Added regression tests for symlink preservation, paths with spaces, reinstallation/config preservation and directory collisions.
- Added those tests to the Linux CI workflow and enabled manual workflow execution.
- Ignored generated Noise private.key, TLS key.pem, PKCS#12 files and Python cache directories.
- Added a Persian setup guide and explicit version/interoperability boundaries.

Git ignore patterns only affect untracked files and are not a substitute for secret scanning.

## Validation performed here

- All three Python installer regression tests passed.
- Bash syntax checks passed for all 11 installer/package/test scripts.
- JSON parsing passed for all 16 configuration examples.
- No supplied release executable was run; installer tests used an explicitly identified fixture.

Rust and Cargo are unavailable in this execution environment. Compilation, rustfmt, clippy, Rust unit/integration tests and privileged TUN/Quantum tests were not run. No protocol implementation or dependency version was changed. Run the checked-in Linux CI before making a release; do not label supplied binaries as builds of these edited sources.

## Publication

Suggested new repository: V2grop/dagger-connect-rs. Source publication should include LICENSE, dependency notices, vendor patches and the verification limitations above. It should not include operational private keys or represent this code as wire-compatible with the original DaggerConnect.
