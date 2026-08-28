param(
    [switch]$SkipBuild
)

$ErrorActionPreference = 'Stop'
$projectRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$releaseRoot = Join-Path $projectRoot 'target\release'
$portableRoot = Join-Path $releaseRoot 'Node2Socks-Portable'
$archivePath = Join-Path $releaseRoot 'Node2Socks-Portable-win-x64.zip'
$desktopExe = Join-Path $releaseRoot 'node2socks-desktop.exe'
$mihomoExe = Join-Path $projectRoot 'sidecar\windows-x64\node2socks-mihomo.exe'
$mihomoNotice = Join-Path $projectRoot 'sidecar\LICENSES\MIHOMO_NOTICE.md'
$expectedMihomoHash = 'F55B3028D9160BEB9044F21B05DD7405B46524614A19642D6291492F5F985761'

if (-not $SkipBuild) {
    Push-Location (Join-Path $projectRoot 'apps\desktop')
    try {
        pnpm tauri build
    }
    finally {
        Pop-Location
    }
}

foreach ($requiredFile in @($desktopExe, $mihomoExe, $mihomoNotice)) {
    if (-not (Test-Path -LiteralPath $requiredFile -PathType Leaf)) {
        throw "Required portable artifact is missing: $requiredFile"
    }
}

$actualMihomoHash = (Get-FileHash -LiteralPath $mihomoExe -Algorithm SHA256).Hash
if ($actualMihomoHash -ne $expectedMihomoHash) {
    throw "Mihomo checksum mismatch: $actualMihomoHash"
}

if (Test-Path -LiteralPath $portableRoot) {
    $resolvedPortable = (Resolve-Path -LiteralPath $portableRoot).Path
    if ((Split-Path -Parent $resolvedPortable) -ne $releaseRoot) {
        throw "Unsafe portable output path: $resolvedPortable"
    }
    Remove-Item -LiteralPath $resolvedPortable -Recurse -Force
}

New-Item -ItemType Directory -Path $portableRoot | Out-Null
Copy-Item -LiteralPath $desktopExe -Destination (Join-Path $portableRoot 'Node2Socks.exe')
Copy-Item -LiteralPath $mihomoExe -Destination (Join-Path $portableRoot 'node2socks-mihomo.exe')
Copy-Item -LiteralPath $mihomoNotice -Destination (Join-Path $portableRoot 'MIHOMO_NOTICE.md')
New-Item -ItemType File -Path (Join-Path $portableRoot 'portable.flag') | Out-Null
Set-Content -LiteralPath (Join-Path $portableRoot 'README.txt') -Encoding UTF8 -Value @(
    'Node2Socks 绿色便携版',
    '',
    '双击 Node2Socks.exe 即可运行。',
    '数据、数据库和本机密钥保存在本目录 data\ 中。',
    '关闭窗口会进入托盘；右键托盘图标并选择“退出”才会真正退出。',
    '适用于 Windows 10/11 x64。'
)

if (Test-Path -LiteralPath $archivePath) {
    Remove-Item -LiteralPath $archivePath -Force
}
Compress-Archive -LiteralPath $portableRoot -DestinationPath $archivePath -CompressionLevel Optimal

Get-FileHash -LiteralPath $archivePath -Algorithm SHA256
