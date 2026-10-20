Set-Location "$env:PUBLIC\Documents\tidepool"
$Host.UI.RawUI.WindowTitle = 'tidepool'
Clear-Host
function Say($cmd) { Write-Host -NoNewline "PS $(Get-Location)> "; Write-Host $cmd }
Say 'cargo run --release -- forecast --station incheon'
Write-Host -NoNewline '   Compiling' -ForegroundColor Green; Write-Host ' tidepool v0.4.2 (C:\Users\Public\Documents\tidepool)'
Write-Host -NoNewline '    Finished' -ForegroundColor Green; Write-Host ' `release` profile [optimized] target(s) in 6.84s'
Write-Host -NoNewline '     Running' -ForegroundColor Green; Write-Host ' `target\release\tidepool.exe forecast --station incheon`'
Write-Host 'station  incheon   (37.45N 126.59E)'
Write-Host '  time    height   trend' -ForegroundColor DarkGray
Write-Host '  04:12   8.31 m   ▲ high' -ForegroundColor Cyan
Write-Host '  10:37   0.62 m   ▼ low'
Write-Host '  16:48   8.05 m   ▲ high' -ForegroundColor Cyan
Write-Host '  23:02   0.88 m   ▼ low'
Write-Host 'next high in 2h 14m' -ForegroundColor Yellow
Write-Host ''
Say 'cargo test'
Write-Host -NoNewline '    Finished' -ForegroundColor Green; Write-Host ' `test` profile [unoptimized + debuginfo] target(s) in 3.10s'
Write-Host 'running 14 tests'
Write-Host -NoNewline 'test result: '; Write-Host -NoNewline 'ok' -ForegroundColor Green; Write-Host '. 14 passed; 0 failed; 0 ignored'
Write-Host ''
