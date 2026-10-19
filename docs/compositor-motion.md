# Composed-frame motion experiment

This branch starts at `0078a16ab337d8ae14eb24805d60aead5a07081b` (`release/0.6.0-pre.6`). It does not change the package version, publish a release, or change the default F coefficients or logical commit distance.

## What changed

- P1: input coordinates retain floating-point precision. Each input sample carries a receiver `Instant`/QPC timeline, the separate OS millisecond timestamp, and, for PTP, the device scan time. The 16-bit 100 µs PTP clock handles wrap, idle resets and implausible timestamps. Raw mouse input has no hardware QPC; receipt time is used, without pretending that `GetMessageTime` has sub-millisecond precision.
- P2: the normal overlay integrates successive targets at their input times. Rendering samples a copy at the frame time; a late event cannot rewind the canonical spring. The board consumes an ordered batch and uses one sampled strip for its painted geometry and thumbnails. Depth continues settling without further input. A bounded visual prediction uses the latest three reports, rejects stale/stationary/reversed/changed-axis input, and cannot commit a desktop. It is capped at half a report interval and 8 ms; intervals below 2 ms are not extrapolated.
- A DirectComposition scene hosts shared live-thumbnail visuals (and, opt-in, the board's opaque D2D surface). It is on by default for the normal view (`compositor = "auto"`); see the final trial below. Float transforms and one scene commit replace independently rounded thumbnail updates for that host. Shared thumbnail dimensions are retained at registration. The normal-view banner and the board popup remain separate windows; multiple monitor hosts do not share one atomic device transaction.
- P3: optional slower visual commits/settling, with unchanged logical latch, HOLD plateau, damping and default tracking coefficients. Tracking returns to its normal response when real held input resumes pulling.
- Optional blur is a separate prototype: two faint directional copies per public DWM thumbnail. It is not a captured-texture convolution or motion-compensated frame generation. Trails appear only above 4 px of travel per frame, so slow drags get none, and the frame after motion stops is drawn once more to clear them.
- Settings has an "Experimental · smoother-looking motion" group: the blur switch and the Default/Gentle/Slow commit spring. Both are off/default; a change applies from the next move without a restart.

## Run the experiments

Use the settings window's Experimental group, or append to `%APPDATA%\Flick\config.toml` (or edit the existing table):

```toml
[experimental]
commit_speed = "default" # default: 35/32; gentle: 28/25; slow: 24/24
motion_blur = false
```

Compare one option at a time. Both apply from the next move. The normal view registers its thumbnails per hold; the board registers them when it opens, so reopen it after changing blur. The default remains `default` / `false`. Blur needs public thumbnails, so while it is on the normal view uses the public path.

The compositor is chosen once at startup by a root config key:

```toml
compositor = "auto" # normal view composed; "off": public thumbnails everywhere; "all": the board too
```

`FLICK_COMPOSITION=0` forces `off` and any other value forces `all` for one process (used by the probes). Composition is gated to Windows build families 22621, 22631, 26100 and 26200 and falls back per thumbnail on shared-thumbnail creation errors; this is an ABI guard, not compatibility certification. Each window's shared-thumbnail size is reused while `GetWindowRect` reports the same size, so a hold does not pay a public register/query/unregister round trip per window.

## Measurement contract

`examples/support/display.rs` drives real `SendInput` relative mouse reports on a jittered 0.8–1.2 ms schedule, and three-contact PTP reports on a 7–8 ms schedule. Replay goes through `Gesture::update`, the real input entry point, and the scan-clock mapping in test builds. It does not simulate HID transport, firmware filtering, palm rejection or driver batching. Mouse motion is quantized to one raw count; synthetic contacts use 0.01 mm coordinates. Actual send intervals are recorded, not assumed to equal the requested schedule.

Each six-second scenario is slow (0.08 cells/s), normal (0.24), fast (0.65), or a minimum-jerk mixed trajectory with acceleration, deceleration, pauses, reversal and multiple cell crossings. Both the normal overlay (`follow`) and board are exercised. `motion-probe` is absent from the ordinary executable.

Desktop Duplication captures composed pixels using `LastPresentTime` and `AccumulatedFrames`. Colored test windows are tracked by their longest solid horizontal run, with balanced color channels to reject wallpaper/desktop icons; weighted centroids retain fractional motion. A pre-input image anchors onset. A green D2D marker on the board follows the same fixture geometry, outside the board clip. Raw screenshots are retained locally for visual validation.

- **Capture losses** (`AccumulatedFrames > 1`) are unknown image history, never evidence of smoothness or an app stall.
- **Pixel repeats** and refresh intervals with no changed image are reported separately. F's intentional tanh saturation and post-commit HOLD can produce genuine identical images even when hand input continues. These are not automatically pacing bugs.
- **Full-scenario CV** uses absolute increments and includes the intentional F curve. It is descriptive, not a pass/fail judder metric. The slow tracking window, 0.6–1.0 seconds after axis selection, avoids the commit/plateau and has approximately 3% ideal curve-induced CV; its sample count and unknown frames must be considered.
- **Peak step** uses adjacent refreshes with no accumulated capture and an unclipped common test patch. It excludes unknown frames and is therefore a measured lower bound, not a guaranteed maximum.
- **First response** is the first ≥1 px image displacement, including zoom. It is not steady-state input-to-photon latency. A separate onset subtracts the existing axis dead zone (mouse: 12/1000 cell; PTP: 3/12 cell). Board presentation thresholds and spring response remain included.
- DDA observes the composed desktop, not photons, panel response or physical scanout. This laptop is a single 1920×1080 ~60 Hz output. No physical 120/144/165 Hz or mixed-refresh validation is claimed.

The probe covers horizontal movement and reversals across several columns; vertical/diagonal input, multiple physical monitors and actual hand-driven HID were not exercised. Three colored fixtures cover the first three test desktops; clipped/missing patches are excluded rather than counted as stationary.

The scripts stage temporary desktops, run and restore configuration/state inside one bridge job; child processes use null standard streams and cleanup releases the mouse trigger. The verified baseline executable is SHA-256 `35cb76e3077c1a6de594eba9d80e4e7c322cc3a935065320fc0340c08585834d`, built from `3b8e8d0` (baseline plus instrumentation). For transport it is added to a temporary, unreferenced Git tree; the binary is not on the pushed branch.

```text
scripts/compositor-e2e.ps1       # baseline, timed, blur, composition, gentle, slow
scripts/compositor-gpu-e2e.ps1   # repeat slow cases; separate GPU-counter comparison
python3 scripts/display-stats.py <variant-folder> ...
```

GPU counters are a separate instrumented run. Per-process 3D-engine utilization/running-time counters include the DWM's other work and may include overlapping engines; they are not per-pass shader durations, ETW present history or isolated thumbnail GPU timings. Counter failure is reported as unavailable. The nine one-second samples span an approximately eight-second window, including startup/settling around the six-second gesture. Full-desktop staging readback and pixel scanning also perturb the workload; capture-work times exclude `AcquireNextFrame` waiting and are retained in the summaries. This probe is not a passive external camera.

## Evidence and results

**The <10% displayed-increment CV goal is reached for the normal view only on the composed path** (final trial below). The board stays on the public path by default; slower visual settling, blur and the composed board are experiments. Sections before the final recheck describe earlier trials and are kept for the record.

Early detector/marker trials are not used as proof: the first detector included wallpaper colors, the baseline board marker was clipped, and an early shared-visual prototype placed normal-view windows behind the wallpaper. [`AddVisual` with a null sibling](https://learn.microsoft.com/en-us/windows/win32/api/dcomp/nf-dcomp-idcompositionvisual-addvisual) inserts above all children only when `insertAbove` is `FALSE`; this was corrected for the final trial. A high update count with missing content is a failure.

### Baseline → timestamped default (main trial)

Each row is one six-second gesture; these are individual trials, not confidence intervals. `R/N` counts repeated positions among comparable captured pairs while input changed; `L` is missing capture history. Full CV includes the F curve, so use the steady tracking table to assess low-speed judder. Onset is milliseconds after the axis dead zone, not steady-state input-to-photon latency.

| View / input / trajectory | Repeats R/N before → after | Full CV % before → after | Peak px/refresh before → after | Onset ms before → after | Capture L before → after |
|---|---:|---:|---:|---:|---:|
| board-mouse-fast | 99/241 → 114/252 | 335.3 → 410.5 | 253.0 → 163.0 | 92.8 → 60.9 | 6 → 2 |
| board-mouse-mixed | 141/267 → 133/257 | 371.0 → 257.9 | 201.0 → 200.0 | 103.9 → 104.5 | 7 → 18 |
| board-mouse-normal | 173/305 → 182/324 | 622.7 → 523.0 | 235.0 → 267.0 | 141.2 → 132.0 | 7 → 4 |
| board-mouse-slow | 132/284 → 159/308 | 185.9 → 167.0 | 4.0 → 10.5 | 319.4 → 329.5 | 1 → 5 |
| board-ptp-fast | 105/250 → 108/263 | 382.0 → 355.7 | 434.0 → 429.0 | 74.9 → 67.6 | 1 → 0 |
| board-ptp-mixed | 116/263 → 116/274 | 285.5 → 278.4 | 396.0 → 247.0 | 77.6 → 69.6 | 1 → 0 |
| board-ptp-normal | 167/273 → 176/284 | 553.3 → 566.1 | 249.0 → 242.0 | 153.2 → 138.5 | 0 → 1 |
| board-ptp-slow | 93/229 → 95/232 | 146.4 → 178.8 | 10.5 → 7.0 | 336.1 → 344.1 | 2 → 0 |
| follow-mouse-fast | 97/244 → 52/201 | 312.7 → 280.5 | 202.0 → 196.0 | 72.2 → 36.6 | 3 → 1 |
| follow-mouse-mixed | 24/160 → 25/167 | 210.8 → 211.7 | 205.0 → 190.7 | 71.0 → 59.1 | 3 → 2 |
| follow-mouse-normal | invalid → 14/164 | invalid → 326.2 | invalid → 197.0 | invalid → 58.9 | 50 → 1 |
| follow-mouse-slow | invalid → 42/221 | invalid → 229.4 | invalid → 30.0 | invalid → 30.2 | 63 → 3 |
| follow-ptp-fast | 80/226 → 86/236 | 302.6 → 308.2 | 207.0 → 212.5 | 72.0 → 55.1 | 4 → 4 |
| follow-ptp-mixed | 20/165 → 24/176 | 209.3 → 212.6 | 210.5 → 208.2 | 68.6 → 49.2 | 5 → 2 |
| follow-ptp-normal | 18/126 → 13/122 | 325.4 → 324.0 | 200.0 → 194.0 | 70.9 → 61.9 | 1 → 1 |
| follow-ptp-slow | 100/243 → 99/241 | 282.4 → 279.2 | 29.0 → 29.0 | 87.5 → 69.2 | 1 → 2 |

The initial baseline slow mouse trial stalled its injector for **1824 ms** and lost 63 capture frames; the normal mouse trial had a 164 ms injector gap and 50 losses. They are retained in the JSON as invalid trials, not presented as improvements. Slow cases were repeated separately below; the normal baseline remains inconclusive. Other capture losses are shown rather than imputed.

### Commit-speed A/B (displayed peaks)

These runs keep the same logical commit and held tracking coefficients. Numbers are the largest observed adjacent-refresh step **during input / after release** in pixels; `0` after release means no measured movement in the eligible window, not zero latency. Clipping and lost frames make these lower bounds. Board peaks do not decrease monotonically, so slower springs alone do not solve its delivery problem.

| View / input / trajectory | Default 35/32 | Gentle 28/25 | Slow 24/24 |
|---|---:|---:|---:|
| board-mouse-fast | 163.0 / 0.0 | 325.0 / 0.0 | 327.0 / 0.0 |
| board-mouse-mixed | 200.0 / 65.0 | 166.0 / 51.0 | 331.0 / 35.0 |
| board-ptp-fast | 429.0 / 0.0 | 189.0 / 0.0 | 297.0 / 0.0 |
| board-ptp-mixed | 247.0 / 80.0 | 184.0 / 42.0 | 180.0 / 129.0 |
| follow-mouse-fast | 196.0 / 39.5 | 173.0 / 39.0 | 150.5 / 14.5 |
| follow-mouse-mixed | 190.7 / 51.0 | 162.5 / 40.0 | 146.0 / 35.0 |
| follow-ptp-fast | 212.5 / 40.0 | 169.5 / 12.0 | 151.0 / 38.5 |
| follow-ptp-mixed | 208.2 / 49.0 | 163.0 / 41.0 | 147.0 / 38.0 |

For normal-view fast mouse input, the measured peak is **196 → 173 → 150.5 px/refresh**. The board reaches **242–267 px/refresh** in normal-speed cases and up to 429 in the fast PTP run. The 429 px event retains the same 405 px fixture width across adjacent captures, so it is not a change in which color patch was selected; this supports a delivery discontinuity in addition to spring strobing. It does not establish a universal 250 px limit or identify a single cause for every jump.

### Observed mechanisms

- The default normal-view slow-mouse tracking window contains seventeen 1 px and six 2 px steps (CV 34.8%); PTP has seventeen 1 px and seven 2 px steps (35.2%). The corresponding **float model geometry** has CV 4.63% / 4.15%, reconstructed from traced position/scale and the still-selected fixture at x=200, width=600. The frame-sampled input CV is 35.36% / 31.14%. Thus smooth float geometry is lost at the integer-thumbnail/presentation boundary; input cadence alone does not explain the final steps. Model numbers are diagnostic submission data, never substituted for composed-pixel results.
- On the main default board/PTP/slow trial, thumbnail and D2D-marker increments disagree by more than 1 px in 41/138 comparable moving pairs (p95 discrepancy 2.00 px). The initial DComp board had 0/21 and p95 0.32 px, but fewer adjacent-refresh pairs because its display cadence was worse. The final corrected compositor trial is reported separately below.
- The 125 Hz/60 Hz beat is plausible, but its isolated contribution is not proven by these end-to-end trials: receipt jitter, coordinate quantization, spring dynamics and composition all contribute. A perfect 8 ms clock supplies two reports in 55 refreshes and three in five refreshes per second, not strict 2/3 alternation every frame. Timestamp-preserving integration removes the assumption that every input batch arrived at render time; it does not eliminate integer raster steps.
- App submission remains insufficient evidence: the main timestamped normal fast-mouse trace has median/p95 gaps 16.66/16.84 ms while displayed increments remain irregular. DComp board slow-mouse update/commit work has median/p95 11.29/21.82 ms, versus 0.08/0.75 ms for public-thumbnail updates; the board paint cost moves in the opposite direction. This does not isolate the driver wait inside that aggregate call block.

### Final recheck: composed normal view by default, composed board opt-in

Trials: `bridge-57b94919` (commit `a1f4283`, variants `off`/`auto`/`all`, all 16 cases each) and a normal-view recheck `bridge-7c07a28c` (`871f0ac`, `off`/`auto`, five follow cases). Summaries: `docs/measurements/compositor-motion/final-{timed,default,composition}.json` (`timed` = `off`). Same laptop, single ~60 Hz 1920×1080 output, Desktop Duplication as above; individual trials, not confidence intervals.

**Board diagnosis.** The earlier composed board's poor cadence was not Commit overhead as such. On the public path the board's D2D repaint already costs Flick ~44% of the 3D engine at 60 Hz (DWM ~23%) and `EndDraw` on the HWND target blocks ~14 ms; in the composed scene the same wait moves into `Commit` (draw 0.5 ms, commit 12–15 ms). The prototype surface was premultiplied, so DWM blended a full-screen layer every frame; making it opaque (`DXGI_ALPHA_MODE_IGNORE`, as the public window target already is) brought the app-side interval p95 from 24.5 to 17.5 ms and capture losses from 71 to 1 (board-mouse-slow). The full-surface redraw still finishes late on this GPU: in board-ptp-fast the composed board left 69 refreshes without a new image, mostly single-refresh gaps about every 33 ms in long stretches, against 2 on the public path. That is why the board stays public by default. Fixing it needs a retained scene (rows/plates as their own surfaces moved by transforms, not a full repaint per frame) or a cheaper board paint; neither was attempted here.

**Normal view.** It has no painted surface in the scene; only shared thumbnail visuals move. It meets the steady-tracking goal and keeps cadence, so it is the default. Its first visible movement is about 35 ms later than the public path in both trials: the hold's windows (and with them the composition device) were dropped after every hold and a new D3D/D2D/DComp device was created before the first frame; the reuse of the size query (`871f0ac`) did not change this. The latest commit keeps one device per thread and creates it when the config is read; it is compile-checked only (see Validation).

| Measure (scenario) | Public path (`off`) | Default (`auto`) | Composed board (`all`) |
|---|---:|---:|---:|
| Steady tracking CV %, normal view, slow mouse (trial / recheck) | 34.8 / 33.9 | **10.5 / 9.0** | 7.6 |
| Steady tracking CV %, normal view, slow PTP | 34.8 / 34.8 | **9.2 / 10.9** | 12.0 |
| Refreshes with no new image, normal view slow mouse | 129 / 139 | **0 / 1** | 2 |
| Normal-view frame work p50/p95 ms (slow mouse) | 0.45 / 2.83 | **0.13 / 0.33** | 0.14 / 0.33 |
| Normal-view frame gap p95 ms (slow mouse) | 16.79 | 16.76 | 16.75 |
| Onset after dead zone ms, normal view fast mouse (trial / recheck) | 35.6 / 35.4 | 72.8 / 71.1 (regression, see above) | 71.9 |
| Board thumbnail vs D2D steps >1 px, slow mouse / slow PTP | 32/93 / 4/142 | same path as `off` | **0/135 / 0/93** |
| Board pixel repeats while moving, slow mouse | 93/252 | 88/239 | 37/299 |
| Board app interval p95 ms, slow mouse (premultiplied prototype: 24.5) | 19.6 | 19.0 | 17.5 |
| Board refreshes with no new image, fast PTP | 2 | 10 | **69** |

Slow-mouse board tracking CV is 51.0 (`off`) vs 76.5 (`all`) because the composed board alternates 0 px and ~3 px steps where it misses a refresh; the composed slow PTP board has 11.9 vs 33.0. Blur was not re-measured: its new rule (no trail under 4 px of travel per frame, one clearing redraw after motion) is covered by a unit test, not by composed-pixel evidence.

### Validation

- Local: `cargo check --target x86_64-pc-windows-msvc --all-targets` (and with `--features motion-probe`) after each change.
- Windows (shared laptop): `cargo test --release` 156 passed, 0 failed at `a1f4283` and at `871f0ac`; `cargo build --release --locked` succeeded; the motion-probe E2E ran once per commit (`scripts/compositor-final-e2e.ps1`, modes `final` and `final-follow`), each in one job that stages, runs and restores the config.
- The settings window was rendered (`--render-settings`) at `871f0ac` and checked: the Experimental group shows the blur switch and the Default/Gentle/Slow spring choice.
- Not verified on Windows: the last code commit (one composition device per thread, created when the config is read, and the notice alignment in settings). Its expected effect is removing the ~35 ms normal-view onset regression; one `final-follow` run would confirm it.
- Not covered: multiple monitors, other refresh rates, other Windows builds, device loss, protected content, physical HID input; blur and slower springs were only unit-tested in this round.


## Why these changes, and what remains risky

Rust is not a 60 Hz rendering limit. The previous application's frame-clock trace measured submission cadence, not composed image motion. Integer destinations and discarded input timestamps/precision are concrete mechanisms for uneven motion; their contributions must be separated from the F curve's deliberately varying velocity.

The analytic 35/.78 unit spring advances by about 0.240406 cells in its fastest 60 Hz interval (the unit test checks this). At a fixed half-scale 1920 px desktop with a .07 gap, that example is about 247 px/frame. This is a calculation, not a measured universal maximum: live depth, focus, prior velocity and clipping differ. Slower experimental springs trade lower peak movement for longer completion; the default stays unchanged.

The compositor prototype uses undocumented `dwmapi` ordinal 147. Window resizing, device loss/recreation, minimization, protected surfaces, other Windows revisions and mixed-monitor atomicity still need broader validation. The normal view uses it by default with a per-thumbnail public fallback and `compositor = "off"`; the board stays public by default. Compositor-side `IDCompositionAnimation` playback of the coupled damped springs/focus/depth has not been implemented: an approximation would need interruption and curve-error validation. The existing normal overlay already has a dedicated render thread and a display-driven clock; simply adding another timer or changing language would not resolve these image-position issues.

Blur triples registered thumbnails (one original plus two copies), adds property submissions, can soften text, and can worsen frame delivery on this laptop. Its trail is half a frame's travel, capped at 32 px before the two fractional offsets; copy alpha is 12% and 6%, and stale samples suppress the trail. It remains off by default regardless of how visually appealing an individual screenshot appears.

Primary references: [DirectComposition transactions](https://learn.microsoft.com/en-us/windows/win32/directcomp/basic-concepts), [surface BeginDraw contract](https://learn.microsoft.com/en-us/windows/win32/api/dcomp/nf-dcomp-idcompositionsurface-begindraw), [shared thumbnail reverse engineering](https://blog.adeltax.com/dwm-thumbnails-but-with-idcompositionvisual/), [Desktop Duplication frame information](https://learn.microsoft.com/en-us/windows/win32/api/dxgi1_2/ns-dxgi1_2-dxgi_outdupl_frame_info), [PTP scan time](https://learn.microsoft.com/en-us/windows-hardware/design/component-guidelines/touchpad-windows-precision-touchpad-collection), [Microsoft's GPU engine accounting explanation](https://devblogs.microsoft.com/directx/gpus-in-the-task-manager/). Windows Task View/Alt+Tab shared-visual evidence motivates the prototype; public Core Animation/DirectComposition architecture is not proof of the private implementation of every Mission Control transition. The separately supplied `RESEARCH.md` was read and remains uncommitted.
