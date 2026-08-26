# Node2Socks implementation status

Updated: 2026-08-26

## Implemented

- M0/M1: Tauri/React/Rust workspace, SQLite migrations, pinned and checksummed Mihomo sidecar, lifecycle/crash recovery.
- M2: stable Slot/port repository, conservative allocator/cooldown, Windows PID/process conflict lookup, persistence and 100-Slot tests, hot selector switching.
- M3: encrypted subscription CRUD/cache, Clash/provider YAML, URI and Base64 detection, stable node identity, localhost token-authenticated Provider Bridge, and manual/cancellable/automatic refresh through one reconciliation path.
- M4: provider diff, disappeared-node reconciliation, confirmed selector `REJECT` before orphan state, and real SOCKS exit-IP/latency checks with timeout and cancellation.
- M5: live product pages for subscriptions, nodes, Slots, Core, diagnostics, settings and Cloud; Tauri commands, tray and autostart.
- M6: read-only Clash process, System Proxy, TUN/virtual adapter and physical adapter diagnostics; configurable outbound interface; no Clash mutation.
- M7: persistent Axum/SQLite WAL server, Argon2id auth, rotating refresh tokens, logout/device revoke, atomic password/Vault rewrap, opaque AES-GCM delta records, CAS conflicts, tombstones, persisted cursors, HTTPS/custom-base-URL client, and account-isolated encrypted Outbox.
- M8: DPAPI local/cloud key persistence, atomic SQLite backup/restore, centralized diagnostic redaction, deterministic child-process shutdown, pinned sidecar/checksum/license notice, and NSIS packaging.

## Verified engineering-candidate evidence

- Rust format, workspace tests, Clippy with warnings denied, frozen frontend install and Vite production build: PASS.
- Pinned Mihomo v1.19.30: 20/20 lifecycle cycles, background crash recovery, one-Core two-listener topology, real `REJECT` fail-closed behavior, and one-Core 100-Slot startup/shutdown: PASS.
- Native release Cloud HTTP flow including register/login, refresh rotation, devices, password change and Vault rewrap: PASS.
- Isolated release desktop startup, localhost-only listener, SQLite/DPAPI persistence, graceful shutdown and zero residual processes: PASS.
- Tauri release executable and NSIS installer generation: PASS. The NSIS tool emitted an updater-only `__TAURI_BUNDLE_TYPE` warning; no updater is implemented in this version.

Exact commands, runtime evidence and SHA-256 values are recorded in `docs/archive/reports/15_ACCEPTANCE_REPORT_2026-08-26.md`.

## Environment-dependent release evidence still required

- The two-upstream/different-exit test needs two usable user-supplied proxy nodes. DIRECT/REJECT topology proves independent ports/selectors but cannot prove two countries.
- Clash Verge + Mihomo TUN matrix requires an actively configured TUN environment plus Wi-Fi/Ethernet transitions and sleep/wake.
- Clean-VM NSIS install/uninstall/reinstall and cloud recovery require a clean Windows VM.
- Docker acceptance is run only when Docker is installed and available on the current host.
- Automated visual inspection could not run because the Windows UI helpers were blocked by the workspace deny-read ACL; functional desktop startup was still verified independently.

These items are deliberately reported as `SKIPPED`, not inferred as passing. The current output is an unsigned engineering candidate, not a production release.
