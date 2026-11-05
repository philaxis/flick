# Pictures of the board's rules toggle and list, with the home monitor at the corner and with a
# monitor left of it (FLICK_LEFT_MONITOR). Run after `cargo build --release`; output in .\board-rules.
$target = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { 'target' }
$flick = Join-Path $target 'release\Flick.exe'
$out = Join-Path (Get-Location) 'board-rules'
New-Item -ItemType Directory -Force $out | Out-Null
$env:FLICK_RULES = '2'
foreach ($left in '', '1920x1080') {
    $name = if ($left) { 'left-monitor' } else { 'one-monitor' }
    if ($left) { $env:FLICK_LEFT_MONITOR = $left } else { Remove-Item Env:FLICK_LEFT_MONITOR -ErrorAction SilentlyContinue }
    Start-Process -Wait -FilePath $flick -ArgumentList '--render-board', "$out\$name.raw"
}
Get-ChildItem $out | ForEach-Object { "$($_.Name) $($_.Length)" }
exit 0
