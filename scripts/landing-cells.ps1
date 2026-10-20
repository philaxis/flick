# Captures the desktops the landing page's demo shows (docs/index.html), on the
# Windows test PC: real apps placed desktop by desktop, the whole screen taken,
# each window cut out on its own. Three shelves, each one piece of work:
# building tidepool, writing the urban forest report, the team's talk and week.
# Puts the user's wallpaper, theme, Notepad tabs and windows back afterwards.
# Results in cells-out\: d<row><col>.png, w-<scene>-<i>.png, icons\, layout.json.
$ErrorActionPreference = 'Continue'
$src = Join-Path (Get-Location) 'scripts\landing-cells'
$out = Join-Path (Get-Location) 'cells-out'
New-Item -ItemType Directory -Force $out, "$out\icons" | Out-Null
Add-Type -AssemblyName System.Drawing
Add-Type -ReferencedAssemblies System.Drawing -TypeDefinition @'
using System; using System.Collections.Generic; using System.Drawing; using System.Drawing.Imaging;
using System.Runtime.InteropServices; using System.Text;
public static class W {
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc f, IntPtr l);
  [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetWindowText(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] static extern int GetWindowLong(IntPtr h, int i);
  [DllImport("user32.dll")] static extern IntPtr GetWindow(IntPtr h, uint c);
  [DllImport("user32.dll")] static extern bool SetWindowPos(IntPtr h, IntPtr after, int x, int y, int w, int hh, uint f);
  [DllImport("user32.dll")] static extern bool ShowWindow(IntPtr h, int c);
  [DllImport("user32.dll")] static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] static extern bool PostMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] static extern int GetSystemMetrics(int i);
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern bool SystemParametersInfo(uint a, uint p, string v, uint f);
  [DllImport("user32.dll")] static extern bool SystemParametersInfo(uint a, uint p, ref RECT v, uint f);
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern IntPtr SendMessageTimeout(IntPtr h, uint m, IntPtr w, string l, uint f, uint t, out IntPtr r);
  [DllImport("dwmapi.dll")] static extern int DwmGetWindowAttribute(IntPtr h, int a, out RECT r, int s);
  [DllImport("dwmapi.dll")] static extern int DwmGetWindowAttribute(IntPtr h, int a, out int v, int s);

  // Windows a person would see on the taskbar.
  public static List<IntPtr> Tops() {
    var list = new List<IntPtr>();
    EnumWindows((h, l) => {
      if (!IsWindowVisible(h) || GetWindow(h, 4) != IntPtr.Zero) return true;
      if ((GetWindowLong(h, -20) & 0x80) != 0) return true;
      int cloaked; if (DwmGetWindowAttribute(h, 14, out cloaked, 4) == 0 && cloaked != 0) return true;
      if (Title(h).Length == 0) return true;
      RECT f = Frame(h); if (f.R - f.L < 200 || f.B - f.T < 120) return true;
      list.Add(h); return true;
    }, IntPtr.Zero);
    return list;
  }
  public static string Title(IntPtr h) { var s = new StringBuilder(512); GetWindowText(h, s, 512); return s.ToString(); }
  public static RECT Frame(IntPtr h) { RECT r; if (DwmGetWindowAttribute(h, 9, out r, 16) != 0) GetWindowRect(h, out r); return r; }
  // Puts the window's visible frame at x,y,w,h: the outer rect has invisible borders round it.
  public static void Place(IntPtr h, int x, int y, int w, int hh) {
    ShowWindow(h, 9);
    SetWindowPos(h, IntPtr.Zero, x, y, w, hh, 0x14);
    RECT o; GetWindowRect(h, out o); RECT f = Frame(h);
    SetWindowPos(h, IntPtr.Zero, x - (f.L - o.L), y - (f.T - o.T), w + (f.L - o.L) + (o.R - f.R), hh + (f.T - o.T) + (o.B - f.B), 0x14);
  }
  public static void Raise(IntPtr h) { SetWindowPos(h, (IntPtr)(-1), 0, 0, 0, 0, 0x13); SetWindowPos(h, (IntPtr)(-2), 0, 0, 0, 0, 0x13); }
  public static void Close(IntPtr h) { PostMessage(h, 0x10, IntPtr.Zero, IntPtr.Zero); }
  public static Bitmap Screen() {
    int w = GetSystemMetrics(0), hh = GetSystemMetrics(1);
    var b = new Bitmap(w, hh, PixelFormat.Format24bppRgb);
    using (var g = Graphics.FromImage(b)) g.CopyFromScreen(0, 0, 0, 0, new Size(w, hh));
    return b;
  }
  public static RECT WorkArea() { var r = new RECT(); SystemParametersInfo(48, 0, ref r, 0); return r; }
  public static void Wallpaper(string p) { SystemParametersInfo(20, 0, p, 3); }
  public static void ThemeChanged() { IntPtr r; SendMessageTimeout((IntPtr)0xffff, 0x1A, IntPtr.Zero, "ImmersiveColorSet", 2, 3000, out r); }
}
'@
[W]::SetProcessDPIAware() | Out-Null
function Log($s) { Add-Content "$out\log.txt" $s; Write-Host $s }

# ---------- the work the desktops show, as files on disk ----------
$docs = Join-Path $env:PUBLIC 'Documents'
$proj = Join-Path $docs 'tidepool'
$report = Join-Path $docs '도시숲 보고서'
$stuff = Join-Path $report '자료'
$serve = Join-Path $env:TEMP 'flick-cells-site'
foreach ($d in $proj, "$proj\src", "$proj\data", "$proj\docs", $stuff, $serve) { New-Item -ItemType Directory -Force $d | Out-Null }
$utf8 = New-Object System.Text.UTF8Encoding $false
function Put($path, $text) { [IO.File]::WriteAllText($path, $text, $utf8) }
Copy-Item "$src\files\main.rs" "$proj\src\main.rs" -Force
Put "$proj\src\tide.rs" "pub struct Reading;`n"
Put "$proj\Cargo.toml" "[package]`nname = `"tidepool`"`nversion = `"0.4.2`"`nedition = `"2021`"`n"
Put "$proj\README.md" "# tidepool`n`nTide tables for a harbour.`n"
Put "$proj\docs\release-0.5.md" "# 0.5`n"
Put "$proj\data\incheon.csv" "time,height`n"
Put "$proj\data\mokpo.csv" "time,height`n"
Put "$proj\.gitignore" "/target`n"
Copy-Item "$src\files\draft.txt" "$report\초안 v3.txt" -Force
Copy-Item "$src\files\refs.txt" "$report\참고문헌.txt" -Force
$meeting = Join-Path $docs '주간회의 메모.txt'
Copy-Item "$src\files\meeting.txt" $meeting -Force
New-Item -ItemType Directory -Force "$serve\site" | Out-Null
Copy-Item "$src\site\*" "$serve\site" -Force
Copy-Item "$src\files\server.js" $serve -Force
# Field photos: the lock screen's own landscapes, whichever this PC has.
$photos = @(Get-ChildItem 'C:\Windows\Web\Screen\*.jpg', 'C:\Windows\Web\Wallpaper\*\*.jpg' -ErrorAction SilentlyContinue | Select-Object -First 6)
for ($i = 0; $i -lt $photos.Count; $i++) { Copy-Item $photos[$i].FullName (Join-Path $stuff ('현장사진_{0:00}.jpg' -f ($i + 1))) -Force }
# The report's chart: shade against temperature at the three places.
function Chart($path) {
  $b = New-Object Drawing.Bitmap 1200, 760; $g = [Drawing.Graphics]::FromImage($b)
  $g.SmoothingMode = 'AntiAlias'; $g.TextRenderingHint = 'ClearTypeGridFit'; $g.Clear([Drawing.Color]::White)
  $f = New-Object Drawing.Font 'Malgun Gothic', 22, ([Drawing.FontStyle]::Bold); $s = New-Object Drawing.Font 'Malgun Gothic', 15
  $ink = [Drawing.Brushes]::Black; $grey = New-Object Drawing.SolidBrush ([Drawing.Color]::FromArgb(110, 110, 110))
  $g.DrawString('그림 1. 14시 기온 비교 (7일 평균)', $f, $ink, 60, 40)
  $line = New-Object Drawing.Pen ([Drawing.Color]::FromArgb(225, 225, 225)), 2
  for ($k = 0; $k -le 4; $k++) { $y = 640 - $k * 120; $g.DrawLine($line, 140, $y, 1120, $y); $g.DrawString(('{0} ℃' -f (26 + $k * 2)), $s, $grey, 60, $y - 12) }
  $bars = @(@('공원 둘레길', 29.2, '#3f9d5a'), @('주택가 골목', 31.6, '#c9a227'), @('대로변', 32.3, '#d1553f'))
  for ($k = 0; $k -lt 3; $k++) {
    $v = $bars[$k][1]; $h = ($v - 26) * 60; $x = 230 + $k * 300
    $c = New-Object Drawing.SolidBrush ([Drawing.ColorTranslator]::FromHtml($bars[$k][2]))
    $g.FillRectangle($c, $x, 640 - $h, 160, $h)
    $g.DrawString(('{0:0.0} ℃' -f $v), $f, $ink, $x + 22, 640 - $h - 44)
    $g.DrawString($bars[$k][0], $s, $grey, $x + 20, 660)
  }
  $b.Save($path, [Drawing.Imaging.ImageFormat]::Png); $g.Dispose(); $b.Dispose()
}
Chart (Join-Path $stuff '기온비교.png')
Copy-Item (Join-Path $stuff '기온비교.png') (Join-Path $stuff '녹지율 지도.png') -Force

# ---------- the PC as it was, to put back ----------
$personalize = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize'
$theme = Get-ItemProperty $personalize -ErrorAction SilentlyContinue
$wallpaper = (Get-ItemProperty 'HKCU:\Control Panel\Desktop').WallPaper
$tabs = Join-Path $env:LOCALAPPDATA 'Packages\Microsoft.WindowsNotepad_8wekyb3d8bbwe\LocalState\TabState'
$tabsBackup = Join-Path $env:TEMP 'flick-cells-tabstate'
if (Test-Path $tabs) { Remove-Item $tabsBackup -Recurse -Force -ErrorAction SilentlyContinue; Copy-Item $tabs $tabsBackup -Recurse -Force }
function ForgetTabs { if (Test-Path $tabs) { Get-ChildItem $tabs | Remove-Item -Recurse -Force -ErrorAction SilentlyContinue } }
$shell = New-Object -ComObject Shell.Application
$shell.MinimizeAll(); Start-Sleep 2
# Dark, as the page around the pictures.
Set-ItemProperty $personalize AppsUseLightTheme 0 -Type DWord
Set-ItemProperty $personalize SystemUsesLightTheme 0 -Type DWord
[W]::ThemeChanged(); Start-Sleep 2
ForgetTabs

$edge = @("${env:ProgramFiles(x86)}\Microsoft\Edge\Application\msedge.exe", "$env:ProgramFiles\Microsoft\Edge\Application\msedge.exe") | Where-Object { Test-Path $_ } | Select-Object -First 1
$edgeData = Join-Path $env:TEMP 'flick-cells-edge'
$node = Get-Command node -ErrorAction SilentlyContinue
$server = $null
$base = 'http://localhost:5173'
if ($node) { $server = Start-Process $node.Source -ArgumentList "`"$serve\server.js`"" -WindowStyle Hidden -PassThru }
else { $base = 'file:///' + ($serve -replace '\\', '/') + '/site' }
function Url($page) { if ($node) { "$base/$page" } else { "$base/$page.html" } }
Log "screen $([W]::Screen().Size) work $(([W]::WorkArea()).R)x$(([W]::WorkArea()).B) edge=$([bool]$edge) node=$([bool]$node)"

# ---------- launching and placing ----------
function Start-App($app, $arg) {
  switch ($app) {
    'notepad'  { Start-Process notepad.exe -ArgumentList "`"$arg`"" }
    'explorer' { Start-Process explorer.exe -ArgumentList "`"$arg`"" }
    'paint'    { Start-Process mspaint.exe -ArgumentList "`"$arg`"" }
    'calc'     { Start-Process calc.exe }
    'terminal' { Start-Process powershell.exe -ArgumentList '-NoLogo', '-NoExit', '-ExecutionPolicy', 'Bypass', '-File', "`"$src\files\$arg`"" }
    'edge'     { Start-Process $edge -ArgumentList (@('--new-window', '--no-first-run', '--no-default-browser-check', '--hide-crash-restore-bubble', "--user-data-dir=`"$edgeData`"") + @($arg | ForEach-Object { Url $_ })) }
  }
}
function Launch($app, $arg) {
  $before = [W]::Tops()
  Start-App $app $arg
  $clock = [Diagnostics.Stopwatch]::StartNew()
  while ($clock.Elapsed.TotalSeconds -lt 20) {
    Start-Sleep -Milliseconds 400
    $new = @([W]::Tops() | Where-Object { $before -notcontains $_ })
    if ($new.Count) {
      Start-Sleep 2
      $new = @([W]::Tops() | Where-Object { $before -notcontains $_ })
      return ($new | Sort-Object { $f = [W]::Frame($_); - ($f.R - $f.L) * ($f.B - $f.T) } | Select-Object -First 1)
    }
  }
  Log "  no window for $app $arg"
  return [IntPtr]::Zero
}
function CloseAll($hs) {
  foreach ($h in $hs) { if ($h -ne [IntPtr]::Zero) { [W]::Close($h) } }
  Start-Sleep 3
  Get-CimInstance Win32_Process -Filter "Name='msedge.exe'" | Where-Object { $_.CommandLine -like '*flick-cells-edge*' } | ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }
  Get-Process Notepad, mspaint -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
  Start-Sleep 1
  ForgetTabs
}
$Rect = { param($r) [ordered]@{ x = $r.L; y = $r.T; w = $r.R - $r.L; h = $r.B - $r.T } }
function Crop($bmp, $r, $path) {
  $rect = New-Object Drawing.Rectangle ([Math]::Max(0, $r.L)), ([Math]::Max(0, $r.T)), ($r.R - [Math]::Max(0, $r.L)), ($r.B - [Math]::Max(0, $r.T))
  $c = $bmp.Clone($rect, $bmp.PixelFormat); $c.Save($path, [Drawing.Imaging.ImageFormat]::Png); $c.Dispose()
}

# Edge's first start in a new profile has its own pages; get it over with.
if ($edge) { $h = Launch 'edge' @('docs'); Start-Sleep 4; CloseAll @($h) }

# One wallpaper per shelf: each piece of work has its own place.
$walls = @(Get-ChildItem 'C:\Windows\Web\Wallpaper\*\*.jpg' -ErrorAction SilentlyContinue | Sort-Object FullName)
Log "wallpapers: $($walls.Name -join ', ')"
function Wall($i) { if ($walls.Count) { $walls[[Math]::Min($i, $walls.Count - 1)].FullName } }
$shelfWall = @((Wall 0), (Wall ([Math]::Floor($walls.Count / 2))), (Wall ($walls.Count - 1)))

# Each desktop: its windows bottom to top, as fractions of the work area.
$scenes = @(
  @{ id = 'd00'; row = 0; wins = @(
      @{ app = 'notepad'; arg = "$proj\src\main.rs"; at = @(.07, .06, .6, .86) },
      @{ app = 'terminal'; arg = 'term-build.ps1'; at = @(.5, .42, .45, .52) }) },
  @{ id = 'd01'; row = 0; wins = @(
      @{ app = 'edge'; arg = @('index', 'docs'); at = @(.1, .05, .8, .9) }) },
  @{ id = 'd02'; row = 0; wins = @(
      @{ app = 'explorer'; arg = $proj; at = @(.05, .08, .52, .78) },
      @{ app = 'terminal'; arg = 'term-git.ps1'; at = @(.48, .2, .47, .62) }) },
  @{ id = 'd10'; row = 1; wins = @(
      @{ app = 'edge'; arg = @('article'); at = @(.04, .06, .55, .88) },
      @{ app = 'notepad'; arg = "$report\초안 v3.txt"; at = @(.5, .2, .45, .74) },
      @{ app = 'explorer'; arg = $stuff; at = @(.62, .04, .35, .42) }) },
  @{ id = 'd11'; row = 1; wins = @(
      @{ app = 'paint'; arg = "$stuff\기온비교.png"; at = @(.2, .06, .62, .86) }) },
  @{ id = 'd20'; row = 2; wins = @(
      @{ app = 'edge'; arg = @('chat'); at = @(.12, .07, .72, .84) }) },
  @{ id = 'd21'; row = 2; wins = @(
      @{ app = 'edge'; arg = @('calendar'); at = @(.05, .07, .6, .84) },
      @{ app = 'notepad'; arg = $meeting; at = @(.6, .14, .35, .66) }) },
  # Pinned: the calculator on every desktop, the report's references on its shelf.
  @{ id = 'pins'; row = 1; wins = @(
      @{ app = 'calc'; arg = ''; at = @(.78, .3, .19, .62) },
      @{ app = 'notepad'; arg = "$report\참고문헌.txt"; at = @(.03, .56, .26, .4) }) }
)

$work = [W]::WorkArea(); $ww = $work.R - $work.L; $wh = $work.B - $work.T
$layout = [ordered]@{ screen = $null; work = (& $Rect $work); scenes = [ordered]@{} }
foreach ($sc in $scenes) {
  Log "scene $($sc.id)"
  [W]::Wallpaper($shelfWall[$sc.row]); Start-Sleep 1
  $hs = @()
  foreach ($w in $sc.wins) {
    $h = Launch $w.app $w.arg
    if ($h -ne [IntPtr]::Zero) {
      $a = $w.at
      [W]::Place($h, [int]($work.L + $a[0] * $ww), [int]($work.T + $a[1] * $wh), [int]($a[2] * $ww), [int]($a[3] * $wh))
      Log "  $($w.app): $([W]::Title($h))"
    }
    $hs += $h
  }
  foreach ($h in $hs) { if ($h -ne [IntPtr]::Zero) { [W]::Raise($h) } }
  Start-Sleep 3
  $wins = @()
  if ($sc.id -ne 'pins') {
    $shot = [W]::Screen(); $layout.screen = [ordered]@{ w = $shot.Width; h = $shot.Height }
    $shot.Save("$out\$($sc.id).png", [Drawing.Imaging.ImageFormat]::Png); $shot.Dispose()
  }
  # Each window by itself, brought to the top to be cut out whole.
  for ($i = 0; $i -lt $hs.Count; $i++) {
    $h = $hs[$i]; if ($h -eq [IntPtr]::Zero) { continue }
    [W]::Raise($h); Start-Sleep -Milliseconds 700
    $f = [W]::Frame($h); $shot = [W]::Screen()
    Crop $shot $f "$out\w-$($sc.id)-$i.png"; $shot.Dispose()
    $wins += [ordered]@{ app = $sc.wins[$i].app; title = [W]::Title($h); frame = (& $Rect $f) }
  }
  $layout.scenes[$sc.id] = [ordered]@{ row = $sc.row; wallpaper = $shelfWall[$sc.row]; windows = $wins }
  CloseAll $hs
}

# The apps' icons, for the board's map.
$icons = @{ notepad = "$env:WINDIR\notepad.exe"; explorer = "$env:WINDIR\explorer.exe"; paint = "$env:WINDIR\System32\mspaint.exe"; calc = "$env:WINDIR\System32\calc.exe"; terminal = "$env:WINDIR\System32\WindowsPowerShell\v1.0\powershell.exe"; edge = $edge }
foreach ($k in $icons.Keys) {
  try { $ic = [Drawing.Icon]::ExtractAssociatedIcon($icons[$k]); $ic.ToBitmap().Save("$out\icons\$k.png", [Drawing.Imaging.ImageFormat]::Png) } catch { Log "  no icon for $k" }
}
$layout | ConvertTo-Json -Depth 8 | Set-Content "$out\layout.json" -Encoding UTF8

# ---------- put everything back ----------
if ($server) { Stop-Process $server -Force -ErrorAction SilentlyContinue }
if ($theme) {
  Set-ItemProperty $personalize AppsUseLightTheme $theme.AppsUseLightTheme -Type DWord -ErrorAction SilentlyContinue
  Set-ItemProperty $personalize SystemUsesLightTheme $theme.SystemUsesLightTheme -Type DWord -ErrorAction SilentlyContinue
  [W]::ThemeChanged()
}
if ($wallpaper) { [W]::Wallpaper($wallpaper) }
ForgetTabs
if (Test-Path $tabsBackup) { Copy-Item "$tabsBackup\*" $tabs -Recurse -Force -ErrorAction SilentlyContinue }
Remove-Item $proj, $report, $meeting, $serve, $edgeData -Recurse -Force -ErrorAction SilentlyContinue
$shell.UndoMinimizeAll()
Get-ChildItem $out -Recurse | Measure-Object | ForEach-Object { Log "files: $($_.Count)" }
