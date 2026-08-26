# Acceptance report — 2026-08-25

## Automated and local Windows results

| Check | Result | Evidence |
|---|---|---|
| Rust format | PASS | `cargo fmt --all -- --check` |
| Rust tests | PASS | Workspace suites passed; auth, crypto, migration, subscription, Slot, recovery and fail-closed covered |
| Rust lint | PASS | `cargo clippy --workspace --all-targets -- -D warnings` |
| Frontend install/build | PASS | frozen pnpm install; TypeScript and Vite production build |
| Mihomo checksum | PASS | v1.19.30 executable SHA-256 `f55b3028d9160beb9044f21b05dd7405b46524614a19642d6291492f5f985761` |
| Core lifecycle | PASS | 20 start/stop cycles; no residual process/port |
| Crash recovery | PASS | background monitor recovered a killed Core and shut down cleanly |
| Two-listener topology | PASS | independent selectors changed without Core PID change |
| Fail closed | PASS | fixed Mihomo confirmed `REJECT`, PID unchanged, listener released on shutdown |
| 100 Slot stress | PASS | one Core, 100 listeners, 512 ms startup, 100 ports released |
| Cloud real HTTP | PASS | release server `/healthz`, server-info, register and authenticated devices request |
| NSIS bundle | PASS | `target/release/bundle/nsis/Node2Socks_0.1.0_x64-setup.exe` |
| Residual processes | PASS | zero desktop/cloud/Mihomo test processes after acceptance |

## Environment-gated results

| Check | Result | Reason |
|---|---|---|
| Two real upstream exit countries | SKIPPED | No usable JP/US proxy credentials were supplied; DIRECT/REJECT cannot prove two external exits |
| Clash System Proxy exit invariance | SKIPPED | No configured upstream pair and controlled running Clash profile were available |
| Clash Verge Mihomo TUN matrix | SKIPPED | Requires active TUN plus Wi-Fi/Ethernet and sleep/wake transitions |
| Docker Compose deployment | SKIPPED | Docker is not installed on this host; the native release HTTP service was tested instead |
| Clean-VM installer lifecycle | SKIPPED | No clean Windows VM was attached |
| A/B-device and fresh-install cloud recovery | SKIPPED | Protocol tests pass, but two-device/clean-install environment was not attached |

Per `docs/09_TEST_AND_ACCEPTANCE.md`, the environment-gated release checks are not silently treated as PASS. The build is an engineering candidate, not a signed production release.
