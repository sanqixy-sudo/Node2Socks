# Mihomo sidecar

Node2Socks runs Mihomo as an independent sidecar process. The executable is intentionally
not committed to this source repository. Download and verify the pinned release with:

~~~powershell
.\scripts\prepare_sidecar.ps1
~~~

Pinned release:

- Mihomo v1.19.30
- Windows amd64 archive SHA-256: 22c09fd67673895ef7cd6b1820563918275c3d316f2462b306208675118db3c0
- Extracted executable SHA-256: f55b3028d9160beb9044f21b05dd7405b46524614a19642d6291492f5f985761

Mihomo is GPL-3.0-or-later. See sidecar/LICENSES/MIHOMO_NOTICE.md and
sidecar/LICENSES/MIHOMO_GPL-3.0.txt. Node2Socks does not link or vendor Mihomo source.