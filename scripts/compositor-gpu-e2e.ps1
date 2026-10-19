$ErrorActionPreference = 'Stop'
$expected = '35cb76e3077c1a6de594eba9d80e4e7c322cc3a935065320fc0340c08585834d'
if ((Get-FileHash 'baseline.exe' -Algorithm SHA256).Hash.ToLowerInvariant() -ne $expected) { throw 'Wrong baseline executable' }
& cmd.exe /d /c 'copy /b assets\app.res +,, assets\app.res & cargo test --release && cargo test --release --example drive && cargo build --release --locked --features motion-probe'
if ($LASTEXITCODE) { throw 'release tests or probe build failed' }
& powershell.exe -NoProfile -File scripts\navigation-e2e.ps1 -Mode gpu
if ($LASTEXITCODE) { throw 'GPU probe failed' }
# Deliver the ordinary application, with no test input hook or diagnostic marker.
& cargo build --release --locked
exit $LASTEXITCODE
