param(
    [string]$Version = 'v1.19.30'
)

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

$projectRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$sidecarRoot = Join-Path $projectRoot 'sidecar\windows-x64'
$downloadRoot = Join-Path $projectRoot 'references\upstream_downloads'
$archive = Join-Path $downloadRoot "mihomo-windows-amd64-$Version.zip"
$expectedArchive = '22c09fd67673895ef7cd6b1820563918275c3d316f2462b306208675118db3c0'
$expectedExecutable = 'f55b3028d9160beb9044f21b05dd7405b46524614a19642d6291492f5f985761'

if ($Version -ne 'v1.19.30') {
    throw 'Only the pinned Mihomo v1.19.30 release is supported by this project.'
}

New-Item -ItemType Directory -Force -Path $downloadRoot, $sidecarRoot | Out-Null
$url = "https://github.com/MetaCubeX/mihomo/releases/download/$Version/mihomo-windows-amd64-$Version.zip"
if (-not (Test-Path -LiteralPath $archive -PathType Leaf)) {
    Write-Host "Downloading pinned Mihomo $Version..."
    Invoke-WebRequest -Uri $url -OutFile $archive -UseBasicParsing
}

$actualArchive = (Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash.ToLowerInvariant()
if ($actualArchive -ne $expectedArchive) {
    throw "Mihomo archive checksum mismatch. expected=$expectedArchive actual=$actualArchive"
}

$temp = Join-Path ([IO.Path]::GetTempPath()) ("node2socks-mihomo-" + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $temp | Out-Null
try {
    Expand-Archive -LiteralPath $archive -DestinationPath $temp -Force
    $candidate = Get-ChildItem -LiteralPath $temp -Recurse -File -Filter '*.exe' |
        Where-Object { $_.Name -notlike '*gui*' } |
        Select-Object -First 1
    if ($null -eq $candidate) {
        throw 'The Mihomo archive did not contain a Windows executable.'
    }
    $actualExecutable = (Get-FileHash -LiteralPath $candidate.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($actualExecutable -ne $expectedExecutable) {
        throw "Mihomo executable checksum mismatch. expected=$expectedExecutable actual=$actualExecutable"
    }
    Copy-Item -LiteralPath $candidate.FullName -Destination (Join-Path $sidecarRoot 'node2socks-mihomo.exe') -Force
    Copy-Item -LiteralPath $candidate.FullName -Destination (Join-Path $sidecarRoot 'node2socks-mihomo-x86_64-pc-windows-msvc.exe') -Force
} finally {
    if (Test-Path -LiteralPath $temp) {
        Remove-Item -LiteralPath $temp -Recurse -Force
    }
}
Write-Host "Mihomo $Version is ready in $sidecarRoot"