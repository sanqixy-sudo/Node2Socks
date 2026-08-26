# Node2Socks acceptance report — 2026-08-26

## Outcome

M0-M8 are implemented as an engineering candidate. The automated quality gates, fixed-Mihomo integration tests, native Cloud HTTP flow, isolated release-desktop smoke test, and NSIS bundle completed successfully. Checks requiring proxy credentials, active Clash/TUN networking, Docker, a clean VM, or a second machine remain `SKIPPED`; therefore this report does not classify the build as a signed production release.

## Build and automated gates

| Check | Result | Evidence |
|---|---|---|
| Rust format | PASS | `cargo fmt --all -- --check` |
| Rust tests | PASS | `cargo test --workspace --locked`; includes allocation/conflict/cooldown, parsing and stable identity, migrations, crypto tamper/wrong-key cases, sync/CAS/tombstones, Outbox partial acceptance/account isolation, Vault/password rotation, and Stable-Key restore cases |
| Rust lint | PASS | `cargo clippy --workspace --all-targets --locked -- -D warnings` |
| Frontend dependencies | PASS | `pnpm install --frozen-lockfile` |
| TypeScript/Vite build | PASS | production build completed |
| NSIS bundle | PASS | `target/release/bundle/nsis/Node2Socks_0.1.0_x64-setup.exe` |

The NSIS build emitted `__TAURI_BUNDLE_TYPE variable not found`. This is an updater metadata warning; the installer was generated and the v1 updater is not implemented.

## Fixed Mihomo v1.19.30 integration

| Check | Result | Runtime evidence |
|---|---|---|
| Sidecar checksum | PASS | Pinned executable SHA-256 matched `f55b3028d9160beb9044f21b05dd7405b46524614a19642d6291492f5f985761` |
| Lifecycle | PASS | `result=PASS cycles=20/20` |
| Crash recovery | PASS | `result=PASS mode=background-monitor crash_pid=21816 recovered_pid=22312 state=Running process_gone=true` |
| Two-listener topology | PASS | `result=PASS pid=22944 listeners=1736,1737 first=REJECT second=DIRECT pid_unchanged=true ports_released=true` |
| Missing-node fail closed | PASS | `result=PASS pid=23712 slot=1732 target=REJECT pid_unchanged=true port_released=true` |
| 100 Slot stress | PASS | `result=PASS pid=8084 cores=1 slots=100 startup_ms=518 ports_released=100` |

The topology test proves two independent localhost listeners/selectors under one owned Core and hot selector switching without changing the Core PID. It does not substitute for the two-real-upstream exit-country gate listed below.

## Release runtime smoke tests

| Check | Result | Runtime evidence |
|---|---|---|
| Cloud native release HTTP flow | PASS | `result=PASS health=ok api=1 devices=1 refresh_rotated=True old_password_rejected=True new_password_login=True vault_rewrapped=true` |
| Isolated release desktop | PASS | `result=PASS pid=21384 responding=True listeners=127.0.0.1:20957 localhost_only=True db_bytes=110592 master_key_bytes=316 graceful_exit=True` |
| Residual processes | PASS | `residual_processes=0` after the isolated release run |

The isolated desktop used an explicit absolute `NODE2SOCKS_DATA_DIR`, so acceptance data did not overwrite the normal user profile.

## Release artifacts

| Artifact | Bytes | SHA-256 |
|---|---:|---|
| `target/release/node2socks-desktop.exe` | 18,615,808 | `51ae0466e3b19b417dd01bdac4680ab7bbc8e58e9bc2bf5628886e684cfb6287` |
| `target/release/bundle/nsis/Node2Socks_0.1.0_x64-setup.exe` | 17,894,833 | `57a5e7411c79252f9b44ad1fe1242756d6665d7068525c4e978eacacdc3ccca3` |
| `sidecar/windows-x64/node2socks-mihomo-x86_64-pc-windows-msvc.exe` | 50,078,720 | `f55b3028d9160beb9044f21b05dd7405b46524614a19642d6291492f5f985761` |

The same release hashes are available in `RELEASE_SHA256SUMS.txt`.

## Environment-gated acceptance matrix

| Check | Result | Reason / required environment |
|---|---|---|
| Two real JP/US upstreams with distinct exit IPs | SKIPPED | No usable upstream credentials were supplied |
| Clash System Proxy exit invariance | SKIPPED | Requires controlled real upstreams and an actively configured Clash profile |
| Clash Verge Rev + Mihomo TUN: automatic physical interface | SKIPPED | Requires active TUN and real network-interface routing |
| Clash Verge Rev + Mihomo TUN: manual Wi-Fi/Ethernet and switching | SKIPPED | Requires both physical interfaces and controlled transitions |
| Clash Verge Rev + Mihomo TUN: sleep/wake | SKIPPED | Requires an interactive Windows power-cycle test |
| Docker Compose deployment | SKIPPED | Docker CLI/runtime is unavailable on this host; the native release service was tested instead |
| Clean-VM install/uninstall/reinstall | SKIPPED | No clean Windows VM was attached |
| True A/B-machine synchronization and fresh-install recovery | SKIPPED | Protocol/integration coverage passed, but two independent machines were not attached |
| Automated visual UI inspection | SKIPPED | Browser/Computer Use helpers failed under the workspace deny-read ACL (`helper_unknown_error: apply deny-read ACLs`) |

## Release decision

The generated installer is suitable for engineering evaluation. It must not be called a signed production release until the applicable environment-gated checks above pass, the clean-VM installer lifecycle is verified, and the release is code-signed. No skipped item has been inferred as `PASS`.
