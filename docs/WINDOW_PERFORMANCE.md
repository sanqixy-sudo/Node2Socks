# Window responsiveness — 2026-09-10

## Findings and changes

Every dashboard snapshot previously ran tasklist and a new PowerShell
Get-NetAdapter process. One local measurement took 483 ms and 1320 ms
respectively (about 1.8 seconds combined); this is scanner time, not a measured
end-to-end window startup time.

Dashboard snapshots now return without awaiting that system scan. A shared,
single-flight background cache refreshes environment information after 60
seconds on demand and notifies the UI when ready. The diagnostics page can
explicitly request a fresh scan. Core health and Slot state still refresh live;
the cache only covers the system/Clash/adapter summary.

The settings adapter command and diagnostic export previously also scanned
synchronously. Their scans now use background blocking workers through async
commands, so the native window event loop does not wait for PowerShell.

Frontend refresh bursts merge by scope and run sequential batches. An event
arriving during a read queues a later read instead of using a pre-mutation
snapshot. Unchanged snapshots retain their identity. Pages are isolated from
toast/busy-state updates, and the title bar keeps one stable window listener.

Node search uses deferred filtering, and unchanged card trees are reused
between progress events. Offscreen cards use Chromium content-visibility
with an estimated height; they remain in the DOM for keyboard/search access.

Closing to tray still destroys the WebView. Opening from tray still has the
WebView cold-start cost; no hidden WebView or permanent node-rendering loop
was added.

## Automated checks

- Frontend: 22 tests, including 1000 nodes over ten idle snapshot refreshes,
  changed-data propagation, merged scopes, refresh recovery, and ten progress
  updates without reformatting/rebuilding unchanged node cards.
- Workspace Rust: 75 tests (21 desktop), including single-flight scan,
  expiry, forced scan, worker refill/cancellation, deferred Core application,
  explicit port allocation, and independent network validation.
- TypeScript and production frontend build passed.

Commands:

```powershell
pnpm --filter @node2socks/desktop test
pnpm build
cargo fmt --check
cargo test --workspace
cargo clippy -p node2socks-desktop --all-targets -- -D warnings
```

## Native UI verification still required

Measure the new executable's end-to-end startup/restore time and memory with
the same subscription data. Check scrolling at 820×560 and 150% DPI, long
names, both themes, keyboard focus on offscreen cards, and rapid search.
The DOM regression tests do not measure native WebView frame rate or memory.

## Follow-up optimizations

- Latency jobs are owned by Rust and have a status command. Reopening the node
  page or recreating the WebView reattaches to the active job and its cancel
  action. Status replies cannot overwrite newer progress events, and a fast
  completion cannot be reset by a late start reply. A shared backend lock also
  protects the legacy test commands from concurrent selector use.
- Six worker selectors are continuously refilled, instead of waiting for the
  slowest member of each six-node batch. Cancellation aborts and drains active
  requests before releasing selector ownership. The existing target URLs,
  per-request timeouts, fallback attempts, and Slot bindings are unchanged.
- Manual refresh-all and the automatic due-subscription loop apply committed
  data once at the end. Post-commit failures still flag data for application.
  Disappeared nodes are blocked immediately; if REJECT cannot be confirmed,
  Core is stopped rather than leaving stale outbound selectors running.
  Core application warnings are surfaced independently of download results.
- Appearance and grouped drafts use separate save payloads. Saving theme does
  not submit or discard port/network drafts, and a failed group rolls back only
  that group. Overlapping saves are blocked. Unchanged manual network settings
  no longer trigger system scans; changed adapters are validated off-thread.
- Explicit Slot ports show the configured range, with inline syntax, range,
  existing Slot, and selection-count errors. Rust retains authoritative system
  occupancy and cooldown checks. Explicit ports never call automatic allocation.

The new regression tests use synthetic node/subscription identifiers; no real
subscription token is included. Workspace tests compile the smoke-test binaries
but do not execute their real Mihomo/network scenarios. No installer/portable
package or installed application has been replaced by these checks.
