# Source-aligned setup repair

## Source comparison

The supplied `DaggerConnect-4.2.8-stable-resync (1).zip` contains
`files-to-upload/dagger-rs-source.zip` and a Linux dagger-rs 0.2.1 binary.
It is not the original DaggerConnect 4.2.8 source.

Compared with repository commit `5ddb682958c6d7c4d6c7ff191b88d8a9f1735cab`,
all 25 files checked (`src/*.rs`, `vendor/**/*.rs`, `Cargo.toml`, `Cargo.lock`)
match after CRLF/LF normalization. None are changed by this repair. Functional
differences from the supplied source were in installation/menu/CI integration.

Supplied outer ZIP SHA256:
`00095e6dfce394962ce14042718d801433bda916bb00f17cbe25fe9ca6b63b5f`

Supplied binary SHA256:
`fccb58880f6dfd53e74349b5780f662b3b2c9f1fea6c21d9e3c738b830457eb7`

## Failures fixed

The classic menu invoked whole actions as `(function) || handler`. Bash disables
`errexit` throughout those functions in that context. A failing keygen, config
write or systemctl command could therefore be followed by later commands.
Additionally, configure returned success on some validation/cancellation paths,
and the caller only checked whether a role config existed. An old config could
be offered for service start even though the current attempt had failed.

Each action now runs in an isolated subshell with active `errexit`, outside
conditional/OR evaluation. The parent records its exit status and keeps the
menu open. Validation errors are nonzero; cancellation is explicit; temporary
configs are cleaned up. No core failure is converted into successful setup.

Other operational issues were implicit loopback dial/target defaults, saving
without starting by default, and `enable --now` leaving already running services
on old configurations. Required addresses, route summaries, explicit save-only
output, restart/check steps and diagnostic logs address these cases.

## Local verification

18 Python tests passed using the **actual supplied binary**, not a mock core for
the forwarding tests. Both peers and destination echo services ran locally.
Configurations were created through options 1/2 of the repaired menu.

| Check | Result |
| --- | --- |
| TCP data integrity through tcp, ws, http, wss, https | Passed |
| TCP data integrity through kcp, quantum+, xhttp, xhttps, dc6 | Passed |
| Reversed xhttp and xhttps | Passed |
| UDP port forwarding and SOCKS5 CONNECT | Passed |
| New connections after client process restart | Passed |
| Keygen failure, own-key rejection, mid-config EOF | Passed |
| Rejected/replacement-cancelled config never starts old config | Passed |
| Missing dial address and invalid port are not accepted as defaults | Passed |
| systemctl reload/enable/restart/state failures | Passed with isolated fixtures |
| Existing installer and checksum regressions | Passed |
| Repacked offline installer, installed menu/keygen, unchanged binary checksum | Passed |

Real transfer checks compare payload bytes, not merely open ports or process
exit codes. systemd orchestration tests use fixtures and do not claim a live
systemd installation was tested locally. The Rust toolchain and privileged
network-namespace suite are handled by the repository's Linux CI before release.
Loopback tests do not establish reachability through a user's ISP, firewall,
datacenter, or external CDN. A real target service must already exist.

Reproduce after building the core:

```bash
cargo build --locked --release
python3 -m unittest discover -s tests -p 'test_*.py' -v
```

Or set `DAGGER_TEST_BIN=/absolute/path/to/dagger-rs` to test an existing binary.
The release workflow runs the real menu tests after its Rust build, in addition
to the existing privileged TUN/raw integration suites.
