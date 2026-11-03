# Setup window review on Windows, after `cargo build --release`: every wizard page and Settings look
# rendered to .\onboarding-review, a first install on a one-desktop PC walked through the wizard
# (.\fresh-install), then the unit tests. Never throws, so the evidence always comes back.
$target = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { 'target' }
$flick = Join-Path $target 'release\Flick.exe'
$out = Join-Path (Get-Location) 'onboarding-review'
New-Item -ItemType Directory -Force $out | Out-Null
foreach ($page in 'trigger', 'touchpad', 'try', 'tour') {
    foreach ($sample in '1', '2') {
        $env:FLICK_GUIDE_PAGE = $page
        $env:FLICK_GUIDE_SAMPLE = $sample
        Start-Process -Wait -FilePath $flick -ArgumentList '--render-guide', "$out\guide-$page-$sample.raw", '9'
    }
}
Remove-Item Env:FLICK_GUIDE_PAGE, Env:FLICK_GUIDE_SAMPLE -ErrorAction SilentlyContinue
foreach ($status in 'absent', 'off', 'conflict', 'active') {
    $env:FLICK_TOUCHPAD = $status
    $env:FLICK_THREE_FINGER = if ($status -in 'conflict', 'active') { '1' } else { '0' }
    Start-Process -Wait -FilePath $flick -ArgumentList '--render-settings', "$out\settings-$status.raw"
}
Remove-Item Env:FLICK_TOUCHPAD, Env:FLICK_THREE_FINGER -ErrorAction SilentlyContinue

$steps = 'shot 1-trigger; click; wait 1; shot 2-heard; key 0D; wait 1; shot 3-next; key 0D; wait 1; shot 4-next2; ' +
    'guideclick 133 109; wait 2; shot 5-made; hold 900 0; wait 2; shot 6-moved; click; wait 2; shot 7-board; ' +
    'key 1B; wait 1.5; guideclick 280 30; wait 0.5; shot 8-ticks; key 0D; wait 1; shot 9-tour; key 0D; wait 2; shot 10-closed'
try {
    & (Join-Path $PSScriptRoot 'fresh-install-e2e.ps1') -Steps $steps -Desktops 1
} catch {
    Add-Content "$out\errors.txt" "fresh install: $_"
}

cargo test --release --locked *> "$out\test.txt"
Set-Content "$out\test-exit.txt" $LASTEXITCODE
Get-Content "$out\test.txt" | Select-String 'test result|FAILED|panicked' | ForEach-Object { $_.Line }
exit 0
