# dagger-rs 0.2.1 — repaired two-host setup

Fixes menu options 1/2 completing without a usable setup. The Rust core,
Cargo dependencies, wire protocol and all carriers are unchanged from the
supplied dagger-rs 0.2.1 source. Attribution and licenses are preserved.

- Setup stops at failed commands; rejecting or cancelling a config never falls
  through to starting an older config. Failed edits preserve the existing JSON.
- Peer key exchange has an explicit key-only path; a machine's own public key
  is rejected. Option 15 displays/generates local keys.
- Client dial address and actual target are required. Port maps default to
  `0.0.0.0:18080` with an explicit explanation; SOCKS remains loopback-only by default.
- Options 1/2 offer service start by default (`yes`, Enter or `1`). Saving without
  starting is explicitly labelled. Option 14 installs/restarts saved configs.
- Applying configuration restarts existing services, checks process state, and
  enables core diagnostic logs. Failures do not print success.
- Option 13 explains routes, checks explicit TCP endpoints and shows service logs.
  TCP reachability and process state are not labelled as end-to-end tunnel success.

The release workflow builds the unchanged core, runs the existing Rust/privileged
TUN/raw suites, new real menu-generated forwarding tests, installer regressions
and packaging before publishing this release. See `docs/setup-repair.md`.

Run the repository's `setup.sh` on BOTH hosts for the latest verified release.
Existing config/key files are retained. Reinstalling the bundle alone does not
restart running services; use menu option 14 on each host to apply the new service
settings, or options 1/2 to correct the saved configuration.

The uploaded archive's outer name refers to DaggerConnect 4.2.8, but its contents
are the independent dagger-rs 0.2.1 implementation. Both peers must run dagger-rs;
it does not interoperate with original DaggerConnect peers. Actual host firewall,
routing and target services must be configured for the selected network path.
