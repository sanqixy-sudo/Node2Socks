$ErrorActionPreference = 'Stop'
Write-Host 'Node2Socks development prerequisites checker'

$cmds = @('git','node','pnpm','cargo','rustc')
foreach ($c in $cmds) {
  $x = Get-Command $c -ErrorAction SilentlyContinue
  if ($null -eq $x) { Write-Warning "$c not found" } else { Write-Host "$c -> $($x.Source)" }
}

Write-Host ''
Write-Host 'Tauri 2 on Windows also requires Microsoft C++ Build Tools and WebView2.'
Write-Host 'After prerequisites are ready, Codex should scaffold the workspace according to docs/01_ARCHITECTURE.md.'
