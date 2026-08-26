# Mihomo reference snapshot

Observed stable version: **v1.19.30**, released 2026-08-16.

Windows amd64 generic asset:

```text
https://github.com/MetaCubeX/mihomo/releases/download/v1.19.30/mihomo-windows-amd64-v1.19.30.zip
SHA-256: 22c09fd67673895ef7cd6b1820563918275c3d316f2462b306208675118db3c0
```

Key docs:

- https://wiki.metacubex.one/en/config/inbound/
- https://wiki.metacubex.one/en/config/proxy-providers/
- https://wiki.metacubex.one/en/config/proxy-providers/content/
- https://wiki.metacubex.one/en/config/general/
- https://wiki.metacubex.one/en/api/

Confirmed capabilities needed by Node2Socks:

1. Multiple `listeners`; a SOCKS listener can specify `proxy`.
2. Proxy providers support `override.additional-prefix`.
3. Provider content supports YAML, URI lines, and Base64 URI lists (formats are separate, not mixed in one provider file).
4. Selector API: `PUT /proxies/{group}` with `{"name":"..."}`.
5. Provider update API: `PUT /providers/proxies/{provider}`.
6. `interface-name` controls Mihomo outbound interface.
7. Keep Controller on localhost and protect with `secret`.

The actual Mihomo binary is intentionally fetched during development/build via `scripts/fetch_upstream_references.ps1` so checksum validation is reproducible.
