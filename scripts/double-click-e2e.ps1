# A double click of the trigger goes back to the desktop rested on before:
# after a hold past several cells, to the cell it began on, not one passed on
# the way; the next double click comes back again; a switch made by Windows
# counts as a move too; a single click still opens the board. Needs Flick and
# drive built with the debug-log feature:
#   cargo build --release --features debug-log --bin Flick --example drive
$ErrorActionPreference = 'Stop'
$target = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { 'target' }
$work = Join-Path $env:TEMP 'flick-double-click'
New-Item -ItemType Directory -Force $work | Out-Null
Copy-Item (Join-Path $target 'release\Flick.exe') (Join-Path $work 'Flick.exe')
Copy-Item (Join-Path $target 'release\examples\drive.exe') (Join-Path $work 'drive.exe')
$flick = Join-Path $work 'Flick.exe'
$drive = Join-Path $work 'drive.exe'
if ((& $drive wins) -match '\|flick\.main\|') { throw 'another Flick is running' }

$dir = Join-Path $env:APPDATA 'Flick'
New-Item -ItemType Directory -Force $dir | Out-Null
$state = Join-Path $dir 'state.json'
$saved = if (Test-Path $state) { Get-Content -Raw $state } else { $null }
$log = Join-Path $dir 'debug-log.txt'
Remove-Item $log -ErrorAction SilentlyContinue

function Desk { ((& $drive desk) -replace '.*Ok\("(.*)"\).*', '$1') }
function Prefix([string]$id) { $id.Trim('{').Substring(0, 8) }
$home_id = Desk
$one = ((& $drive mkdesk) -replace '.*Ok\("(.*)"\).*', '$1')
$two = ((& $drive mkdesk) -replace '.*Ok\("(.*)"\).*', '$1')
$names = @{ $home_id = 'home'; $one = 'one'; $two = 'two' }
@{ rows = @(@{ name = 'Back'; cells = @($home_id, $one, $two); last = $null }); guide_seen = $true; board_hints_seen = $true } |
    ConvertTo-Json -Depth 5 | Set-Content -Encoding UTF8 $state
$script:failed = $false
function Expect([string]$what, [string]$want) {
    Start-Sleep -Milliseconds 1500
    $on = Desk
    $ok = $on -eq $want
    Write-Output "RESULT $what -> on $($names[$on]) (want $($names[$want])): $ok"
    if (-not $ok) { $script:failed = $true }
}
function Opened { @(Get-Content $log | Where-Object { $_ -match 'ui: board opened' }).Count }

$app = $null
try {
    $app = Start-Process $flick -ArgumentList '--portable' -PassThru
    Start-Sleep -Seconds 4
    # Held past the middle cell to the end of the row: a move from home to two.
    & $drive xbutton down; Start-Sleep -Milliseconds 300
    & $drive stroke 2400 0 900; Start-Sleep -Milliseconds 300
    & $drive xbutton up
    Expect 'a hold past one cell' $two
    & $drive xclick 2 90
    Expect 'double click' $home_id
    & $drive xclick 2 90
    Expect 'double click again' $two
    # Clicks apart: the first opens the board, the second goes to its cell.
    $before = Opened
    & $drive xclick 1 0; Start-Sleep -Milliseconds 1200
    $opened = (Opened) -eq $before + 1
    Write-Output "RESULT a single click opens the board: $opened"
    if (-not $opened) { $script:failed = $true }
    & $drive xclick 1 0
    Expect 'a click on the board stays' $two
    # Gone to by Windows, not Flick: still a move to go back from.
    & $drive todesk (Prefix $one) | Out-Null
    Expect 'a switch by Windows' $one
    & $drive xclick 2 90
    Expect 'double click after it' $two
    & $drive xclick 2 90
    Expect 'and back' $one
    Get-Content $log | Where-Object { $_ -match 'double click|at rest' } | Select-Object -Last 14
} finally {
    & $drive todesk (Prefix $home_id) | Out-Null
    Start-Sleep -Milliseconds 500
    foreach ($made in $one, $two) { & $drive rmdesk (Prefix $made) | Out-Null }
    if ($null -ne $saved) { Set-Content -NoNewline -Encoding UTF8 $state $saved } else { Remove-Item $state -ErrorAction SilentlyContinue }
}
if ($script:failed) { exit 1 }
