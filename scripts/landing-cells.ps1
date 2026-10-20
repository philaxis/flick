# Captures the desktops the landing page's demo shows (docs/index.html), on the
# Windows test PC: real apps and real sites (signed out) placed desktop by
# desktop, the whole screen taken, each window cut out on its own. The person
# builds and markets Flick 0.6: a shelf for building it, one for its launch,
# and one for a break between, a video kept playing there.
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
  [DllImport("user32.dll")] static extern bool IsIconic(IntPtr h);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetClassName(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern IntPtr FindWindowEx(IntPtr p, IntPtr a, string c, string t);
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
  // Whatever else is open and not minimized, owned dialogs too: hidden while the pictures are taken.
  public static List<IntPtr> Strays() {
    var list = new List<IntPtr>();
    EnumWindows((h, l) => {
      if (!IsWindowVisible(h) || IsIconic(h) || Title(h).Length == 0) return true;
      var c = new StringBuilder(256); GetClassName(h, c, 256);
      string k = c.ToString(); if (k == "Shell_TrayWnd" || k == "Shell_SecondaryTrayWnd" || k == "Progman" || k == "WorkerW") return true;
      int cloaked; if (DwmGetWindowAttribute(h, 14, out cloaked, 4) == 0 && cloaked != 0) return true;
      RECT f = Frame(h); if (f.R - f.L < 100 || f.B - f.T < 60) return true;
      list.Add(h); return true;
    }, IntPtr.Zero);
    return list;
  }
  // The desktop's icons: the list view in the shell's desktop window.
  public static IntPtr DesktopIcons() {
    IntPtr view = FindWindowEx(FindWindowEx(IntPtr.Zero, IntPtr.Zero, "Progman", null), IntPtr.Zero, "SHELLDLL_DefView", null);
    IntPtr w = IntPtr.Zero;
    while (view == IntPtr.Zero && (w = FindWindowEx(IntPtr.Zero, w, "WorkerW", null)) != IntPtr.Zero) view = FindWindowEx(w, IntPtr.Zero, "SHELLDLL_DefView", null);
    return view == IntPtr.Zero ? IntPtr.Zero : FindWindowEx(view, IntPtr.Zero, "SysListView32", null);
  }
  public static void Show(IntPtr h, bool on) { ShowWindow(h, on ? 8 : 0); }
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
$proj = Join-Path $docs 'flick'
$launch = Join-Path $docs 'Flick 0.6 출시'
$serve = Join-Path $env:TEMP 'flick-cells-site'
foreach ($d in $proj, $launch, "$serve\site") { New-Item -ItemType Directory -Force $d | Out-Null }
$utf8 = New-Object System.Text.UTF8Encoding $false
# Flick's own source, as this job was given it.
foreach ($f in 'src', 'examples', 'scripts', 'assets', 'Cargo.toml', 'Cargo.lock', 'README.md', 'LICENSE', 'build.rs') { if (Test-Path $f) { Copy-Item $f $proj -Recurse -Force } }
Copy-Item "$src\files\notice.txt" "$launch\출시 공지 초안.txt" -Force
Copy-Item "$src\files\sns.txt" "$launch\SNS 게시물.txt" -Force
Copy-Item 'docs\og.png' "$launch\랜딩 대표 이미지.png" -Force
Copy-Item 'docs\icon.png' "$launch\아이콘.png" -Force
Copy-Item 'docs\drafts\shots\real-cells.png' "$launch\스크린샷 1.png" -Force -ErrorAction SilentlyContinue
Get-ChildItem 'docs\drafts\shots\*.png' | Select-Object -First 2 | ForEach-Object -Begin { $n = 2 } -Process { Copy-Item $_.FullName "$launch\스크린샷 $n.png" -Force; $n++ }
Copy-Item "$src\site\*" "$serve\site" -Force
Copy-Item "$src\files\server.js" $serve -Force

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
$strays = @([W]::Strays()); foreach ($h in $strays) { [W]::Show($h, $false) }
$iconsView = [W]::DesktopIcons(); if ($iconsView -ne [IntPtr]::Zero) { [W]::Show($iconsView, $false) }
Log "hidden: $($strays.Count) windows, desktop icons $($iconsView -ne [IntPtr]::Zero)"
# Dark, as the page around the pictures.
Set-ItemProperty $personalize AppsUseLightTheme 0 -Type DWord
Set-ItemProperty $personalize SystemUsesLightTheme 0 -Type DWord
[W]::ThemeChanged(); Start-Sleep 2
ForgetTabs

$edge = @("${env:ProgramFiles(x86)}\Microsoft\Edge\Application\msedge.exe", "$env:ProgramFiles\Microsoft\Edge\Application\msedge.exe") | Where-Object { Test-Path $_ } | Select-Object -First 1
$edgeData = Join-Path $env:TEMP 'flick-cells-edge'
# A new profile nobody is signed in to; videos play by themselves, without sound.
$edgeFlags = @('--no-first-run', '--no-default-browser-check', '--hide-crash-restore-bubble', '--autoplay-policy=no-user-gesture-required', '--mute-audio', '--disable-features=Translate,msTranslate', "--user-data-dir=`"$edgeData`"")
$node = Get-Command node -ErrorAction SilentlyContinue
$server = $null
$base = 'http://localhost:5173'
if ($node) { $server = Start-Process $node.Source -ArgumentList "`"$serve\server.js`"" -WindowStyle Hidden -PassThru }
else { $base = 'file:///' + ($serve -replace '\\', '/') + '/site' }
function Url($page) { if ($page -match '^https?:') { $page } elseif ($node) { "$base/$page" } else { "$base/$page.html" } }
Log "screen $([W]::Screen().Size) work $(([W]::WorkArea()).R)x$(([W]::WorkArea()).B) edge=$([bool]$edge) node=$([bool]$node)"

# ---------- launching and placing ----------
function Start-App($app, $arg) {
  switch ($app) {
    'notepad'  { Start-Process notepad.exe -ArgumentList "`"$arg`"" }
    'explorer' { Start-Process explorer.exe -ArgumentList "`"$arg`"" }
    'paint'    { Start-Process mspaint.exe -ArgumentList "`"$arg`"" }
    'calc'     { Start-Process calc.exe }
    'terminal' { Start-Process powershell.exe -ArgumentList '-NoLogo', '-NoExit', '-ExecutionPolicy', 'Bypass', '-File', "`"$src\files\$arg`"" }
    'edge'     { Start-Process $edge -ArgumentList ($edgeFlags + '--new-window' + @($arg | ForEach-Object { "`"$(Url $_)`"" })) }
    # A page as an app of its own: its window, no address bar.
    'app'      { Start-Process $edge -ArgumentList ($edgeFlags + "--app=`"$(Url $arg)`"") }
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
if ($edge) {
  $h = Launch 'edge' @('https://github.com/philaxis/flick'); Start-Sleep 6; CloseAll @($h)
  # No offer to translate the English pages over them.
  $prefs = Join-Path $edgeData 'Default\Preferences'
  if (Test-Path $prefs) {
    $p = Get-Content $prefs -Raw -Encoding UTF8 | ConvertFrom-Json
    $p | Add-Member -Force translate ([pscustomobject]@{ enabled = $false })
    [IO.File]::WriteAllText($prefs, ($p | ConvertTo-Json -Depth 64 -Compress), $utf8)
  }
}

# One wallpaper per shelf: each piece of work has its own place.
# Windows' own wallpapers only, none of a maker's.
$walls = @(Get-ChildItem 'C:\Windows\Web\Wallpaper\Theme*\*.jpg', 'C:\Windows\Web\Wallpaper\Windows\*.jpg' -ErrorAction SilentlyContinue | Sort-Object FullName)
Log "wallpapers: $(($walls | ForEach-Object { $_.Directory.Name + '\' + $_.Name }) -join ', ')"
function Wall($pattern) { ($walls | Where-Object { $_.FullName -like $pattern } | Select-Object -First 1).FullName }
$shelfWall = @((@((Wall '*\ThemeD\*'), (Wall '*\ThemeA\*'), (Wall '*\ThemeC\*'), (Wall '*\img0.jpg')) | Where-Object { $_ } | Select-Object -First 1), (Wall '*\img19.jpg'), (Wall '*\img27.jpg'))

# Each desktop: its windows bottom to top, as fractions of the work area.
# Shelves: 0 building Flick, 1 a break, 2 marketing its launch. `frames` takes the screen again every `every` seconds.
$video = 'https://www.youtube.com/watch?v=aqz-KE-bpKQ&t=95'
$scenes = @(
  @{ id = 'd00'; row = 0; wins = @(
      @{ app = 'notepad'; arg = "$proj\src\follow.rs"; at = @(.2, .05, .56, .86) },
      @{ app = 'terminal'; arg = 'term-build.ps1'; at = @(.52, .44, .44, .5) }) },
  @{ id = 'd01'; row = 0; wins = @(
      @{ app = 'edge'; arg = @('https://github.com/philaxis/flick/releases', 'https://github.com/philaxis/flick/pulls?q=is%3Apr'); at = @(.2, .04, .76, .9) }) },
  @{ id = 'd02'; row = 0; wins = @(
      @{ app = 'explorer'; arg = $proj; at = @(.22, .08, .46, .74) },
      @{ app = 'terminal'; arg = 'term-git.ps1'; at = @(.5, .22, .46, .6) }) },
  @{ id = 'd10'; row = 1; frames = 3; every = 18; wins = @(
      @{ app = 'edge'; arg = @($video); at = @(.06, .04, .88, .92) }) },
  @{ id = 'd11'; row = 1; wins = @(
      @{ app = 'edge'; arg = @('https://www.youtube.com/results?search_query=lofi+%EA%B3%B5%EB%B6%80+%EC%9D%8C%EC%95%85'); at = @(.1, .05, .8, .88) }) },
  @{ id = 'd20'; row = 2; wins = @(
      @{ app = 'edge'; arg = @('https://www.google.com/search?q=%EC%9C%88%EB%8F%84%EC%9A%B0+11+%EA%B0%80%EC%83%81+%EB%8D%B0%EC%8A%A4%ED%81%AC%ED%86%B1+%EB%8B%A8%EC%B6%95%ED%82%A4&hl=ko'); at = @(.2, .04, .5, .9) },
      @{ app = 'notepad'; arg = "$launch\출시 공지 초안.txt"; at = @(.6, .14, .37, .74) }) },
  @{ id = 'd21'; row = 2; wins = @(
      @{ app = 'app'; arg = 'analytics'; at = @(.2, .05, .6, .72) },
      @{ app = 'notepad'; arg = "$launch\SNS 게시물.txt"; at = @(.64, .4, .33, .56) }) },
  @{ id = 'd22'; row = 2; wins = @(
      @{ app = 'paint'; arg = "$launch\랜딩 대표 이미지.png"; at = @(.2, .04, .56, .8) },
      @{ app = 'explorer'; arg = $launch; at = @(.6, .42, .37, .52) }) },
  # Pinned: the calculator on every shelf; the team's chat on the two shelves of work.
  @{ id = 'pins'; row = 0; wins = @(
      @{ app = 'calc'; arg = ''; at = @(.77, .03, .21, .58) },
      @{ app = 'app'; arg = 'chat'; at = @(.01, .4, .18, .58) }) }
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
  Start-Sleep $(if ($sc.wins | Where-Object { $_.app -eq 'edge' }) { 8 } else { 3 })
  $wins = @()
  if ($sc.id -ne 'pins') {
    for ($n = 0; $n -lt $(if ($sc.frames) { $sc.frames } else { 1 }); $n++) {
      if ($n) { Start-Sleep ($sc.every - 2) }
      if ($sc.frames) {
        # The pointer over the playing video brings up its controls, the bar of how far it has played.
        $f = [W]::Frame($hs[0]); [W]::SetCursorPos($f.L + [int](($f.R - $f.L) * .3), $f.T + [int](($f.B - $f.T) * .4)); Start-Sleep -Milliseconds 300
        [W]::SetCursorPos($f.L + [int](($f.R - $f.L) * .32), $f.T + [int](($f.B - $f.T) * .42)); Start-Sleep 2
      }
      $shot = [W]::Screen(); $layout.screen = [ordered]@{ w = $shot.Width; h = $shot.Height }
      $shot.Save("$out\$($sc.id)$(if ($n) { [char](97 + $n) }).png", [Drawing.Imaging.ImageFormat]::Png); $shot.Dispose()
    }
    [W]::SetCursorPos(0, 0)
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
Remove-Item $proj, $launch, $serve, $edgeData -Recurse -Force -ErrorAction SilentlyContinue
if ($iconsView -ne [IntPtr]::Zero) { [W]::Show($iconsView, $true) }
foreach ($h in $strays) { [W]::Show($h, $true) }
$shell.UndoMinimizeAll()
Get-ChildItem $out -Recurse | Measure-Object | ForEach-Object { Log "files: $($_.Count)" }
