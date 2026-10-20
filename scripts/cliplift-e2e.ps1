# End-to-end check of the screen following the hand, on the Windows test PC
# (examples/glide_e2e.rs cliplift). Puts the user's Flick settings back afterwards.
# Results in e2e\: report.txt, log.txt and shots.zip (half-size BMPs).
$ErrorActionPreference = 'Continue'
$out = Join-Path (Get-Location) 'e2e'
New-Item -ItemType Directory -Force $out | Out-Null
cmd /d /c "copy /b assets\app.res +,, assets\app.res" | Out-Null
cargo build --release --locked 2>&1 | Select-Object -Last 3
cargo build --release --example glide_e2e 2>&1 | Select-Object -Last 3
$target = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { 'target' }
$flick = Join-Path $target 'release\Flick.exe'
$e2e = Join-Path $target 'release\examples\glide_e2e.exe'
$row = Join-Path $out 'glide_row.exe'
Copy-Item $e2e $row -Force
$cfg = Join-Path $env:APPDATA 'Flick'
$backup = Join-Path $out 'backup'
New-Item -ItemType Directory -Force $backup | Out-Null
foreach ($f in 'config.toml', 'state.json') { if (Test-Path "$cfg\$f") { Copy-Item "$cfg\$f" $backup } }
$was = Get-Process Flick -ErrorAction SilentlyContinue | Select-Object -First 1 -ExpandProperty Path
Get-Process Flick -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep 1
$logStart = if (Test-Path "$cfg\log.txt") { (Get-Content "$cfg\log.txt").Count } else { 0 }
& $e2e prepare $out
$env:FLICK_GLIDE_STATS = '1'
$app = Start-Process $flick -ArgumentList '--portable' -PassThru
Start-Sleep 4
& $e2e cliplift $out
Stop-Process $app -Force -ErrorAction SilentlyContinue
Start-Sleep 1
& $e2e cleanup $out
Remove-Item "$cfg\config.toml", "$cfg\state.json" -ErrorAction SilentlyContinue
foreach ($f in 'config.toml', 'state.json') { if (Test-Path "$backup\$f") { Copy-Item "$backup\$f" $cfg } }
if (Test-Path "$cfg\log.txt") { Get-Content "$cfg\log.txt" | Select-Object -Skip $logStart | Set-Content "$out\log.txt" }
if ($was) { Start-Process $was }
Compress-Archive -Force -Path "$out\c*.bmp", "$out\report.txt", "$out\log.txt" -DestinationPath "$out\shots.zip"
Get-Content "$out\report.txt"
