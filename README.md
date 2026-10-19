<p align="center"><a href="README.md">English</a> · <a href="README.ko.md">한국어</a> · <a href="README.zh.md">中文</a></p>

<p align="center"><img src="assets/icon-256.png" width="96" alt="Flick"></p>

<p align="center"><strong><a href="https://github.com/philaxis/flick/releases/latest/download/Flick.exe">⬇ Download Flick for Windows</a></strong></p>

<p align="center">Hold a button, flick the mouse, and you are on the next desktop.</p>

<p align="center"><a href="https://philaxis.github.io/flick/">See how it works on the Flick website</a></p>

---

**Your desktops become a grid.** A shelf (row) holds the desktops for one job; up and down leads to other shelves. Only shelves have names.

**Hold and flick.** Hold the trigger, push the mouse, and you move one desktop that way. The pointer stays where it was.

**Tap to see everything.** Tap the trigger for the windows on this desktop and a small map of all of them. Drag a window onto the map to move it.

**Any button you like.** It starts as the mouse "forward" button. Right-click the tray icon → *Settings…* → *Change* and press whatever you want instead, even several keys at once.

**Small and free.** One exe under 1 MB. No account, no installer to click through. Open source, MIT.

**Three-finger touchpad.** Enable it in Settings after setting Windows’ three-finger swipes and taps to Nothing; tap for the board, or hold and move about 1 cm per desktop. By default the desktops go the other way from the fingers, as if pushing the screen like scrolling; the mouse follows the hand.

**Mouse and touchpad each their own way.** Settings has a tab for each, *Mouse* and *Touchpad*. In each you set sideways (along a shelf) and up and down (to another shelf) apart: how far to move for one desktop (raw mouse counts for the mouse (physical travel depends on mouse DPI, not screen pixels), millimetres for the touchpad) and how firmly each desktop holds (*Notch*; Off follows the hand evenly). *Reverse direction* turns that hand around.

**The screen follows the hand.** With *Follows the hand* on (the default, set for each way), the desktops slide along with the mouse or the fingers as on a Mac, the next ones coming into view, and the desktop changes once, when you let go: to the nearest one, or on to where a quick flick carries it. Windows pinned to every shelf stay put, and windows pinned to a shelf stay put sideways. Turned off, each desktop is gone to at once as the hand reaches it. The Mouse tab’s follow switches also control keyboard shortcut animations. The same settings are under `[mouse]` and `[touchpad]` in config.toml; a config.toml from an earlier version is read as the mouse settings.

**The screen shows where you are.** While you move, the screen steps back so that a desktop each way, slantwise too, is in sight; the one in the middle stands out, a little larger and brighter than the rest, and the shelf’s name shows at the top. Moving along a shelf slides that shelf only; the shelves above and below stay where they are. The small minimap that used to come up while moving is off by default: turn on *Minimap while moving* in Settings (or `minimap = true` in config.toml) to have it back.

**Pinning and always on top are separate.** The board cycles a window through *Off → This shelf → All shelves → Off*. The pin menu (`Ctrl+Alt+Win+P`) uses the same choices; select a checked choice to turn it off. *On top* is a separate button. An *App:* pin is inherited by every window of that app and can only be changed through the menu’s App choices; clicking a window never unpins its whole app.

**Settings with the keyboard.** Tab / Shift+Tab move the visible focus, arrow keys adjust a focused slider, and Space / Enter activate a button or toggle. Ctrl+Tab switches device tabs; 1 / 2 still work. The window fits the monitor’s work area and scrolls vertically when needed, with the wheel, scrollbar or Page Up / Page Down; keyboard focus scrolls into view. Settings and the guide use the Windows UI language (Korean, Chinese, or English otherwise), and the guide names the current trigger.

**Startup stays quiet.** Elevation is opt-in through *Run as administrator* in Settings, so declining or postponing it never triggers another UAC request on startup. Turning it off affects the next launch; the current process keeps its privileges until restarted. The tray-location balloon appears only when the first guide is completed. Hover previews still appear immediately: the obsolete `dwell_ms` key is ignored and can be removed from older config files.

## Get started

1. Download with the button above and run it. It installs itself and starts with Windows.
2. Hold the trigger and flick the mouse sideways.
3. Tap the trigger and press **+** on the map to add a desktop or a new shelf.

## Good to know

- For Windows 11 23H2 (build 22631.3085 or later), 24H2 (build 26100.2605 or later) and 25H2. On any other version it tells you so and does not start.
- To remove it: Settings → Apps → Installed apps → Flick.

## Build

`cargo build --release` on Windows, or `scripts/build-wsl.sh build --release` from WSL.

[MIT](LICENSE). `vendor/winvd` and `vendor/winvd-24h2` come from [VirtualDesktopAccessor](https://github.com/Ciantic/VirtualDesktopAccessor) (MIT).
