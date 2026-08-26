param(
    [ValidateRange(1, 100)]
    [int]$Cycles = 20
)

$ErrorActionPreference = 'Stop'
$Root = Split-Path -Parent $PSScriptRoot
$Cargo = Join-Path $env:USERPROFILE '.cargo\bin\cargo.exe'
$Mihomo = Join-Path $Root 'sidecar\windows-x64\node2socks-mihomo.exe'
$Runtime = Join-Path $Root '.test-runtime\mihomo-lifecycle'

if (-not (Test-Path -LiteralPath $Cargo -PathType Leaf)) {
    throw "cargo not found at $Cargo"
}
if (-not (Test-Path -LiteralPath $Mihomo -PathType Leaf)) {
    throw "Mihomo sidecar not found at $Mihomo"
}

& $Cargo run --locked --package node2socks-core-adapter --bin core-smoke -- $Mihomo $Runtime $Cycles
if ($LASTEXITCODE -ne 0) {
    throw "Mihomo lifecycle test failed with exit code $LASTEXITCODE"
}
