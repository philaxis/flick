# Public path, the new default (composed normal view) and the composed board, in one job.
# -Mode final-follow repeats only the normal-view cases against the public path.
param([string]$Mode = 'final')
$ErrorActionPreference = 'Stop'
& cmd.exe /d /c 'copy /b assets\app.res +,, assets\app.res & cargo test --release && cargo build --release --locked --features motion-probe'
if ($LASTEXITCODE) { throw 'release tests or probe build failed' }
& powershell.exe -NoProfile -File scripts\navigation-e2e.ps1 -Mode $Mode
$probe = $LASTEXITCODE
# Deliver the ordinary application, with no test input hook or diagnostic marker.
& cargo build --release --locked
if ($LASTEXITCODE) { exit $LASTEXITCODE }
$target = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { 'target' }
Start-Process -FilePath (Join-Path $target 'release\Flick.exe') -ArgumentList '--render-settings', (Join-Path (Get-Location) 'navigation-e2e\settings.raw') -Wait
exit $probe
