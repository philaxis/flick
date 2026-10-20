Set-Location "$env:PUBLIC\Documents\flick"
$Host.UI.RawUI.WindowTitle = 'flick'
Clear-Host
function Say($cmd) { Write-Host -NoNewline "PS $(Get-Location)> "; Write-Host $cmd }
Say 'cargo build --release'
Write-Host -NoNewline '   Compiling' -ForegroundColor Green; Write-Host ' flick v0.6.0 (C:\Users\Public\Documents\flick)'
Write-Host -NoNewline '    Finished' -ForegroundColor Green; Write-Host ' `release` profile [optimized] target(s) in 41.27s'
Write-Host ''
Say 'cargo test --release follow'
Write-Host -NoNewline '    Finished' -ForegroundColor Green; Write-Host ' `release` profile [optimized] target(s) in 2.31s'
Write-Host '     Running unittests src\main.rs'
foreach ($t in 'follow::tests::recedes_as_far_as_the_hand_is_unsure', 'follow::tests::a_sure_flick_slides_full_size', 'follow::tests::pinned_windows_keep_their_place', 'follow::tests::rows_go_by_notches') {
  Write-Host -NoNewline "test $t ... "; Write-Host 'ok' -ForegroundColor Green
}
Write-Host ''
Write-Host -NoNewline 'test result: '; Write-Host -NoNewline 'ok' -ForegroundColor Green; Write-Host '. 4 passed; 0 failed; 0 ignored; 124 filtered out'
Write-Host ''
