# dagger-rs 0.2.1 — V2grop maintenance release

Independent Rust tunnel, originally authored by ir_spoof. Original MIT license and dependency notices are preserved. The 4.2.8 reference version belongs to DaggerConnect; this release does not interoperate with original DaggerConnect peers.

Built from the commit attached to this release after Linux formatting, Clippy, Rust tests, installer regression tests, privileged TUN/raw carrier integration and packaging checks pass. The binary reports upstream source version 0.2.1; the release tag distinguishes these maintenance builds.

Fix: the installer replaces a symlink at dagger-setup without overwriting its target. Includes a Persian guide and installer regression tests.

For Ubuntu/Debian x86-64, download the .run and matching .run.sha256 asset into the same directory, verify with sha256sum -c, then run sudo bash dagger-rs-linux-x86_64.run. Python 3 is required for setup. The installation opens the menu; later use sudo /usr/local/bin/dagger-setup.

Network behavior depends on host privileges, firewall and routing. See README.fa.md and docs/security.md. No claim of universal network/CDN compatibility or independent cryptographic audit.


Adds a second, original-style menu (`dagger-setup-classic`) alongside `dagger-setup`.
Both menus share the Rust configuration, keys and hardened systemd services.
The root `setup.sh` installs a checksummed release and opens the second menu.
Numeric transport selection, validated config editing, live logs and service controls
are available. Existing configs are retained on install; running services are not
restarted automatically. No dependency changes are included: the previously reported
time 0.3.45 advisory and rustls-pemfile maintenance warning remain outstanding.
