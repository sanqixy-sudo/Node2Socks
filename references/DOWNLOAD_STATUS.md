# Reference download status

The public repository contains only source notes, official license notices, and checksums.
Large upstream archives and downloaded executables are intentionally excluded from Git.

Run `scripts/prepare_sidecar.ps1` on Windows to download the pinned Mihomo v1.19.30
amd64 release from the official GitHub repository and verify both archive and executable
SHA-256 values before placing the sidecar files in `sidecar/windows-x64/`.