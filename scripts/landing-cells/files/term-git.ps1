Set-Location "$env:PUBLIC\Documents\flick"
$Host.UI.RawUI.WindowTitle = 'flick'
Clear-Host
function Say($cmd) { Write-Host -NoNewline "PS $(Get-Location)> "; Write-Host $cmd }
Say 'git log --format="%h %<(58,trunc)%s" -8'
$log = @(
  @('5b74e24', 'Flick 0.6.0'),
  @('ff4e450', 'A window over the whole of its desktop, maximized ones t..'),
  @('0185400', 'Flick 0.6.0-pre.8'),
  @('dc65ad5', 'Previews get their icons over them as soon as the icons ..'),
  @('ca898f3', 'The board-capture test windows carry an icon as apps do'),
  @('5027773', 'A row of the map lies on the spine by its selected cell ..'),
  @('277f828', 'An app''s icon sits on its preview''s lower left corner, o..'),
  @('3421c72', 'Merge the board skin into the smooth strip, drawing the ..')
)
foreach ($l in $log) { Write-Host -NoNewline $l[0] -ForegroundColor Yellow; Write-Host " $($l[1])" }
Write-Host ''
Say 'git switch -c release/0.6.1'
Write-Host "Switched to a new branch 'release/0.6.1'"
Write-Host ''
