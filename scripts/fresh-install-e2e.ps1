# A first run on a PC that has never had Flick: the downloaded exe is started from a download folder.
# Flick's folders, autostart value, uninstall entry and shortcut are set aside first and put back after.
# Run after `cargo build --release`; screenshots and logs land in .\fresh-install.
param([string]$Steps = 'baseline')
$ErrorActionPreference = 'Stop'
$target = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { 'target' }
$flick = Join-Path $target 'release\Flick.exe'
cargo build --release --locked --example drive
if ($LASTEXITCODE) { throw 'driver build failed' }
$drive = Join-Path $target 'release\examples\drive.exe'
$out = Join-Path (Get-Location) 'fresh-install'
$backup = Join-Path $out 'backup'
New-Item -ItemType Directory -Force $out, $backup | Out-Null

Add-Type -AssemblyName System.Drawing, System.Windows.Forms
Add-Type 'using System.Runtime.InteropServices; public static class FreshDpi { [DllImport("user32.dll")] public static extern bool SetProcessDPIAware(); }'
[FreshDpi]::SetProcessDPIAware() | Out-Null
function Shot([string]$name) {
    $b = [System.Windows.Forms.SystemInformation]::VirtualScreen
    $bmp = New-Object System.Drawing.Bitmap $b.Width, $b.Height
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.CopyFromScreen($b.Left, $b.Top, 0, 0, $bmp.Size)
    $bmp.Save((Join-Path $out "$name.png"), [System.Drawing.Imaging.ImageFormat]::Png)
    $g.Dispose(); $bmp.Dispose()
    Add-Content "$out\report.txt" "shot $name"
}
function Desktops {
    $ids = (Get-ItemProperty 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Explorer\VirtualDesktops' -Name VirtualDesktopIDs -ErrorAction SilentlyContinue).VirtualDesktopIDs
    if ($ids) { $ids.Length / 16 } else { 1 }
}
function Click([int]$ms = 120) { & $drive xbutton down; Start-Sleep -Milliseconds $ms; & $drive xbutton up }
function Key([string]$vk) { & $drive key down $vk; Start-Sleep -Milliseconds 60; & $drive key up $vk }
# reg.exe writes its success line to stderr, which PowerShell 5 turns into a terminating error under Stop.
function Reg([string]$arguments) { cmd /d /c "reg $arguments >nul 2>&1"; $LASTEXITCODE -eq 0 }

$roaming = Join-Path $env:APPDATA 'Flick'
$local = Join-Path $env:LOCALAPPDATA 'Flick'
$link = Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs\Flick.lnk'
$run = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
$uninstall = 'HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\Flick'

$was = Get-Process Flick -ErrorAction SilentlyContinue | Select-Object -First 1 -ExpandProperty Path
Get-Process Flick -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep 1
if (Test-Path $roaming) { Move-Item $roaming "$backup\roaming" }
if (Test-Path $local) { Move-Item $local "$backup\local" }
if (Test-Path $link) { Move-Item $link "$backup\Flick.lnk" }
$runValue = (Get-ItemProperty $run -Name Flick -ErrorAction SilentlyContinue).Flick
Remove-ItemProperty $run -Name Flick -ErrorAction SilentlyContinue
$hadUninstall = Reg "query `"$uninstall`""
if ($hadUninstall) { Reg "export `"$uninstall`" `"$backup\uninstall.reg`" /y" | Out-Null; Reg "delete `"$uninstall`" /f" | Out-Null }

$downloads = Join-Path $out 'Downloads'
New-Item -ItemType Directory -Force $downloads | Out-Null
Copy-Item $flick "$downloads\Flick.exe"
$screen = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
$middle = @([int]($screen.Left + $screen.Width / 2), [int]($screen.Top + $screen.Height / 2))
try {
    Add-Content "$out\report.txt" "steps $Steps; desktops before $(Desktops)"
    Start-Process "$downloads\Flick.exe"
    Start-Sleep 8
    Shot '1-first-run'
    & $drive mouse $middle[0] $middle[1]
    if ($Steps -eq 'baseline') {
        Click
        Start-Sleep 2
        Shot '2-trigger-click'
        Key 1B
        Start-Sleep 1
        & $drive xbutton down
        Start-Sleep -Milliseconds 300
        & $drive stroke 900 0 600
        Start-Sleep -Milliseconds 300
        Shot '3-hold-and-move'
        & $drive xbutton up
        Start-Sleep 2
        Shot '4-let-go'
    } else {
        # Each further step is "wait <s>", "shot <name>", "click", "key <vk hex>", "mouse <x> <y> [down|up]",
        # "hold <dx> <dy>" (trigger held through a stroke) or "drive <args...>", separated by ';'.
        foreach ($step in ($Steps -split ';')) {
            $words = @($step.Trim() -split '\s+')
            switch ($words[0]) {
                'wait' { Start-Sleep -Milliseconds ([double]$words[1] * 1000) }
                'shot' { Shot $words[1] }
                'click' { Click }
                'key' { Key $words[1] }
                'mouse' { & $drive @($words) }
                'hold' {
                    & $drive xbutton down; Start-Sleep -Milliseconds 300
                    & $drive stroke $words[1] $words[2] 600; Start-Sleep -Milliseconds 300
                    & $drive xbutton up
                }
                'drive' { & $drive @($words[1..($words.Count - 1)]) }
                default { Add-Content "$out\report.txt" "unknown step $step" }
            }
        }
    }
    Get-Process Flick -ErrorAction SilentlyContinue | ForEach-Object { Add-Content "$out\report.txt" "running $($_.Id) $($_.Path)" }
    foreach ($f in 'log.txt', 'config.toml', 'state.json') { if (Test-Path "$roaming\$f") { Copy-Item "$roaming\$f" "$out\after-$f" } }
    Add-Content "$out\report.txt" "installed exe $(Test-Path "$local\Flick.exe"); autostart $((Get-ItemProperty $run -Name Flick -ErrorAction SilentlyContinue).Flick); desktops after $(Desktops)"
} finally {
    Get-Process Flick -ErrorAction SilentlyContinue | Stop-Process -Force
    & $drive xbutton up
    Start-Sleep 1
    Remove-Item $roaming, $local -Recurse -Force -ErrorAction SilentlyContinue
    Remove-Item $link -ErrorAction SilentlyContinue
    Remove-ItemProperty $run -Name Flick -ErrorAction SilentlyContinue
    Reg "delete `"$uninstall`" /f" | Out-Null
    if (Test-Path "$backup\roaming") { Move-Item "$backup\roaming" $roaming }
    if (Test-Path "$backup\local") { Move-Item "$backup\local" $local }
    if (Test-Path "$backup\Flick.lnk") { Move-Item "$backup\Flick.lnk" $link }
    if ($runValue) { Set-ItemProperty $run -Name Flick -Value $runValue }
    if ($hadUninstall) { Reg "import `"$backup\uninstall.reg`"" | Out-Null }
    Remove-Item "$downloads\Flick.exe" -ErrorAction SilentlyContinue
    if ($was) { Start-Process $was }
    Add-Content "$out\report.txt" 'restored'
}
Get-Content "$out\report.txt"
