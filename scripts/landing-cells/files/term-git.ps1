Set-Location "$env:PUBLIC\Documents\tidepool"
$Host.UI.RawUI.WindowTitle = 'tidepool'
Clear-Host
function Say($cmd) { Write-Host -NoNewline "PS $(Get-Location)> "; Write-Host $cmd }
Say 'git log --oneline -7'
$log = @(
  @('a41c9e2', '(HEAD -> main) forecast: show when the next high tide comes'),
  @('7d03b18', 'tide: find highs and lows from the readings'),
  @('52e0f6a', 'data: add the incheon and mokpo stations'),
  @('e91b2c4', 'web: draw the day as one curve'),
  @('3b7a5d0', 'parse the station CSV, skip broken rows'),
  @('0c4f218', 'cli: --station picks the harbour'),
  @('9a11e7f', 'first cut')
)
foreach ($l in $log) { Write-Host -NoNewline $l[0] -ForegroundColor Yellow; Write-Host " $($l[1])" }
Write-Host ''
Say 'git status --short'
Write-Host ' M src/main.rs' -ForegroundColor Red
Write-Host ' M src/tide.rs' -ForegroundColor Red
Write-Host '?? docs/release-0.5.md' -ForegroundColor Red
Write-Host ''
