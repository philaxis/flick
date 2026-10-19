# Keep the measured pre-change executable before the shared target is rebuilt.
$ErrorActionPreference = 'Stop'
$target = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { 'target' }
$old = Join-Path $target 'release\Flick.exe'
$expected = '35cb76e3077c1a6de594eba9d80e4e7c322cc3a935065320fc0340c08585834d'
if ((Get-FileHash $old -Algorithm SHA256).Hash.ToLowerInvariant() -ne $expected) {
    throw 'Baseline cache changed; use the verified executable from bridge-60f3cd329fcc419ea5b5da3f before running this comparison.'
}
Copy-Item $old 'baseline.exe'
& cmd.exe /d /c 'copy /b assets\app.res +,, assets\app.res & cargo test --release && cargo build --release --locked --features motion-probe'
if ($LASTEXITCODE) { throw 'release tests or probe build failed' }
& powershell.exe -NoProfile -File scripts\navigation-e2e.ps1 -Mode matrix
exit $LASTEXITCODE
