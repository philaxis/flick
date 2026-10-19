# Run after building Flick; stage, run and restore within this same bridge job.
$ErrorActionPreference = 'Stop'
$target = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { 'target' }
$flick = Join-Path $target 'release\Flick.exe'
cargo build --release --locked --example drive
if ($LASTEXITCODE) { throw 'driver build failed' }
$drive = Join-Path $target 'release\examples\drive.exe'
$out = Join-Path (Get-Location) 'navigation-e2e'
New-Item -ItemType Directory -Force $out | Out-Null
$cfg = Join-Path $env:APPDATA 'Flick'
$backup = Join-Path $out 'backup'
New-Item -ItemType Directory -Force $backup | Out-Null
$was = Get-Process Flick -ErrorAction SilentlyContinue | Select-Object -First 1 -ExpandProperty Path
Get-Process Flick -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep 1
foreach ($f in 'config.toml', 'state.json') { if (Test-Path "$cfg\$f") { Copy-Item "$cfg\$f" $backup } }
$probeExit = 1
$logStart = if (Test-Path "$cfg\log.txt") { (Get-Content "$cfg\log.txt").Count } else { 0 }
try {
    & $drive navigation stage $out
    if ($LASTEXITCODE) { throw 'stage failed' }
    & $drive navigation run $out $flick
    $probeExit = $LASTEXITCODE
    if ($probeExit) { Write-Warning 'Probe failed; collecting its trace before reporting the result' }
} finally {
    Get-Process Flick -ErrorAction SilentlyContinue | Stop-Process -Force
    Start-Sleep 1
    & $drive navigation unstage $out
    foreach ($f in 'config.toml', 'state.json') {
        Remove-Item "$cfg\$f" -ErrorAction SilentlyContinue
        if (Test-Path "$backup\$f") { Copy-Item "$backup\$f" $cfg }
    }
    if (Test-Path "$cfg\log.txt") { Get-Content "$cfg\log.txt" | Select-Object -Skip $logStart | Set-Content "$out\log.txt" }
    if ($was) { Start-Process $was }
}
Get-Content "$out\report.txt"

# Transport succeeds even on an assertion failure so its evidence is retrievable.
Set-Content (Join-Path $out 'probe-exit.txt') $probeExit
Write-Output "PROBE_EXIT=$probeExit"
