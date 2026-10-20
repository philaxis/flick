# Drags a window's preview on the board onto another cell of the map and
# checks that the window left this desktop (it is cloaked once it is on
# another). Needs Flick and drive built with the debug-log feature, for the
# board's layout in screen coordinates:
#   cargo build --release --features debug-log --bin Flick --example drive
$ErrorActionPreference = 'Stop'
$target = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { 'target' }
$Flick = Join-Path $target 'release\Flick.exe'
$Drive = Join-Path $target 'release\examples\drive.exe'
$work = Join-Path $env:TEMP 'flick-board-drop'
New-Item -ItemType Directory -Force $work | Out-Null
Copy-Item $Flick (Join-Path $work 'Flick.exe')
Copy-Item $Drive (Join-Path $work 'drive.exe')
$flick = Join-Path $work 'Flick.exe'
$drive = Join-Path $work 'drive.exe'
if ((& $drive wins) -match '\|flick\.main\|') { throw 'another Flick is running' }

$dir = Join-Path $env:APPDATA 'Flick'
New-Item -ItemType Directory -Force $dir | Out-Null
$state = Join-Path $dir 'state.json'
$saved = if (Test-Path $state) { Get-Content -Raw $state } else { $null }
$log = Join-Path $dir 'debug-log.txt'
Remove-Item $log -ErrorAction SilentlyContinue

$home_id = ((& $drive desk) -replace '.*Ok\("(.*)"\).*', '$1')
$made = ((& $drive mkdesk) -replace '.*Ok\("(.*)"\).*', '$1')
@{ rows = @(@{ name = 'Drop'; cells = @($home_id, $made); last = $null }); guide_seen = $true; board_hints_seen = $true } |
    ConvertTo-Json -Depth 5 | Set-Content -Encoding UTF8 $state
$hold = Start-Process $drive -ArgumentList 'hold', '255', '255', '0', 'DropProbe', '200', '200', '900', '600', '90' -PassThru
$app = $null
try {
    Start-Sleep -Milliseconds 1500
    $probe = ((& $drive wins) | Where-Object { $_ -match '\|flick\.test\.hold\|DropProbe\|' } | Select-Object -First 1).Split('|')[0]
    $app = Start-Process $flick -ArgumentList '--portable' -PassThru
    Start-Sleep -Seconds 4
    & $drive click | Out-Null
    Start-Sleep -Seconds 2

    $lines = Get-Content $log
    $layout = ($lines | Where-Object { $_ -match 'board: layout' } | Select-Object -Last 1)
    $prefix = $made.Substring(0, 4)
    if ($layout -notmatch ([regex]::Escape($prefix) + '@(\d+),(\d+)x')) { throw "no cell $prefix in: $layout" }
    $to = @([int]$Matches[1], [int]$Matches[2])
    $tile = ($lines | Where-Object { $_ -match "board: tile $probe top (\d+),(\d+)" } | Select-Object -Last 1)
    if ($tile -notmatch "board: tile $probe top (\d+),(\d+)") { throw "no tile for $probe" }
    # Above the tile's buttons is its preview.
    $from = @([int]$Matches[1], ([int]$Matches[2] - 80))
    Write-Output "drag $probe from $($from -join ',') to cell $prefix at $($to -join ',')"

    & $drive mouse $from[0] $from[1] down | Out-Null
    foreach ($i in 1..12) {
        $x = [int]($from[0] + ($to[0] - $from[0]) * $i / 12)
        $y = [int]($from[1] + ($to[1] - $from[1]) * $i / 12)
        & $drive mouse $x $y | Out-Null
        Start-Sleep -Milliseconds 40
    }
    Start-Sleep -Milliseconds 300
    & $drive mouse $to[0] $to[1] up | Out-Null
    Start-Sleep -Seconds 2

    $after = (& $drive wins all) | Where-Object { $_ -match "\|$probe\|" -or $_ -match "^v\dc\d+\|$probe\|" }
    Write-Output "after: $after"
    Get-Content $log | Where-Object { $_ -match 'board: down on|app: .*move|move_window' } | Select-Object -Last 5
    $cloaked = $after -match '^v\dc[1-9]'
    Write-Output "RESULT moved to the other desktop: $([bool]$cloaked)"
    if (-not $cloaked) { $script:failed = $true }
} finally {
    if ($app) { Stop-Process -Id $app.Id -Force -ErrorAction SilentlyContinue }
    Stop-Process -Id $hold.Id -Force -ErrorAction SilentlyContinue
    & $drive rmdesk $made.Trim('{').Substring(0, 8) | Out-Null
    if ($null -ne $saved) { Set-Content -NoNewline -Encoding UTF8 $state $saved } else { Remove-Item $state -ErrorAction SilentlyContinue }
}
if ($script:failed) { exit 1 }
