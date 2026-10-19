<p align="center"><a href="README.md">English</a> · <a href="README.ko.md">한국어</a> · <a href="README.zh.md">中文</a></p>

<p align="center"><img src="assets/icon-256.png" width="96" alt="Flick"></p>

<p align="center"><strong><a href="https://github.com/philaxis/flick/releases/latest/download/Flick.exe">⬇ Download Flick for Windows</a></strong></p>

<p align="center">Hold a button, flick the mouse, and you are on the next desktop.</p>

<p align="center"><a href="https://philaxis.github.io/flick/">See how it works on the Flick website</a></p>

---

**Your desktops become a grid.** Left and right are the screens of one job. Up and down are other jobs.

**Hold and flick.** Hold the trigger, push the mouse, and you move one desktop that way. The pointer stays where it was.

**Tap to see everything.** Tap the trigger for the windows on this desktop and a small map of all of them. Drag a window onto the map to move it.

**Any button you like.** It starts as the mouse "forward" button. Right-click the tray icon → *Settings…* → *Change* and press whatever you want instead, even several keys at once.

**Small and free.** One exe under 1 MB. No account, no installer to click through. Open source, MIT.

**Three-finger touchpad.** Enable it in Settings after setting Windows’ three-finger swipes and taps to Nothing; tap for the board, or hold and move about 1 cm per desktop. By default the desktops go the other way from the fingers, as if pushing the screen like scrolling; the mouse follows the hand.

**Mouse and touchpad each their own way.** Settings has a tab for each, *Mouse* and *Touchpad*. In each you set sideways (along a row) and up and down (to another row) apart: how far to move for one desktop (pixels for the mouse, millimetres for the touchpad) and how firmly each desktop holds (*Notch*; Off follows the hand evenly). *Reverse direction* turns that hand around.

**The screen follows the hand.** With *Follows the hand* on (the default, set for each way), the desktops slide along with the mouse or the fingers as on a Mac, the next ones coming into view, and the desktop changes once, when you let go: to the nearest one, or on to where a quick flick carries it. Windows pinned to every shelf stay put, and windows pinned to a shelf stay put sideways. Turned off, each desktop is gone to at once as the hand reaches it. The same settings are under `[mouse]` and `[touchpad]` in config.toml; a config.toml from an earlier version is read as the mouse settings.

## Get started

1. Download with the button above and run it. It installs itself and starts with Windows.
2. Hold the trigger and flick the mouse sideways.
3. Tap the trigger and press **+** on the map to add a desktop or a new row.

## Good to know

- For Windows 11 23H2 (build 22631.3085 or later), 24H2 (build 26100.2605 or later) and 25H2. On any other version it tells you so and does not start.
- To remove it: Settings → Apps → Installed apps → Flick.

## Build

`cargo build --release` on Windows, or `scripts/build-wsl.sh build --release` from WSL.

[MIT](LICENSE). `vendor/winvd` and `vendor/winvd-24h2` come from [VirtualDesktopAccessor](https://github.com/Ciantic/VirtualDesktopAccessor) (MIT).
