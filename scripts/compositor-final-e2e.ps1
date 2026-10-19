# Public path, the new default (composed normal view) and the composed board, in one job.
$ErrorActionPreference = 'Stop'
& cmd.exe /d /c 'copy /b assets\app.res +,, assets\app.res & cargo test --release && cargo build --release --locked --features motion-probe'
if ($LASTEXITCODE) { throw 'release tests or probe build failed' }
& powershell.exe -NoProfile -File scripts\navigation-e2e.ps1 -Mode final
$probe = $LASTEXITCODE
# Deliver the ordinary application, with no test input hook or diagnostic marker.
& cargo build --release --locked
if ($LASTEXITCODE) { exit $LASTEXITCODE }
exit $probe
