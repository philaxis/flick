//! Global hooks that turn "hold the trigger and move the mouse" into grid steps.
//!
//! The hooks live on their own thread. Windows delivers every mouse and
//! keyboard event through that thread, so it must never wait on anything: it
//! only does arithmetic and posts messages to the app window, and the app can
//! take as long as it likes without the cursor stuttering.
//!
//! While the trigger is held the cursor does not move: the hook swallows
//! every mouse move, and the movement is read from raw input instead, which
//! reports what the device did regardless of where the cursor is.

use crate::{
    config::Config,
    grid::Dir,
    hold::{Chord, Held, Source},
    slide::Plane,
};
use std::{
    cell::{Cell, RefCell},
    sync::{
        atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicU64, Ordering},
        Mutex,
    },
    time::Instant,
};
use windows::Win32::{
    Foundation::{CloseHandle, HANDLE, HWND, LPARAM, LRESULT, POINT, WPARAM},
    Security::{GetSidSubAuthority, GetSidSubAuthorityCount, GetTokenInformation, TokenIntegrityLevel, TOKEN_MANDATORY_LABEL, TOKEN_QUERY},
    System::{
        LibraryLoader::GetModuleHandleW,
        StationsAndDesktops::{CloseDesktop, OpenInputDesktop, DESKTOP_CONTROL_FLAGS, DESKTOP_READOBJECTS},
        Threading::{GetCurrentProcess, GetCurrentThreadId, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION},
    },
    UI::{
        Input::{
            GetRawInputData,
            KeyboardAndMouse::{
                MapVirtualKeyW, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS,
                KEYEVENTF_KEYUP, MAPVK_VK_TO_VSC, VIRTUAL_KEY,
            },
            RegisterRawInputDevices, HRAWINPUT, RAWINPUT, RAWINPUTDEVICE, RAWINPUTHEADER, RIDEV_INPUTSINK, RIDEV_REMOVE,
            RID_INPUT, RIM_TYPEKEYBOARD, RIM_TYPEMOUSE,
        },
        WindowsAndMessaging::{
            CallNextHookEx, CreateWindowExW, DefWindowProcW, DispatchMessageW, GetCursorPos, GetForegroundWindow,
            GetMessageW, GetSystemMetrics, GetWindowThreadProcessId, KillTimer, PostMessageW, PostThreadMessageW,
            RegisterClassW, SetTimer, SetWindowsHookExW, UnhookWindowsHookEx, WindowFromPoint, HC_ACTION, HHOOK,
            HWND_MESSAGE, KBDLLHOOKSTRUCT, LLKHF_INJECTED, MSG, MSLLHOOKSTRUCT, SM_CXSCREEN, SM_CXVIRTUALSCREEN, SM_CYSCREEN,
            SM_CYVIRTUALSCREEN, WH_KEYBOARD_LL, WH_MOUSE_LL, WINDOW_EX_STYLE, WINDOW_STYLE, WM_APP, WM_INPUT,
            WM_KEYDOWN, WM_KEYUP, WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MOUSEMOVE, WM_QUIT, WM_SYSKEYDOWN, WM_SYSKEYUP,
            WM_XBUTTONDOWN, WM_XBUTTONUP, WNDCLASSW,
        },
    },
};

/// One step in a direction, as a hotkey takes it; sent by the test driver
/// (examples/drive.rs). wparam: direction index (see `dir_from_index`).
pub const WM_STEP: u32 = WM_APP + 1;
/// The trigger was pressed and released without travelling a step.
pub const WM_CLICK: u32 = WM_APP + 2;
/// The trigger was released after at least one step.
pub const WM_RELEASE: u32 = WM_APP + 3;

/// The user finished choosing a new trigger; wparam is 1 when one was chosen
/// (fetch it with `take_captured`) and 0 when it was cancelled.
pub const WM_CAPTURED: u32 = WM_APP + 4;
/// The held trigger began to travel sideways: the minimap is wanted, though
/// no cell has been come to yet. wparam and lparam as with `WM_SLIDE`.
pub const WM_DRAG: u32 = WM_APP + 5;
/// The hand came to another cell of the row. wparam: which stretch of the
/// hold it is of and the cell, counted from the one the stretch began on
/// (`slide_of`); lparam: which hold. To be answered with `slide_shown`.
pub const WM_SLIDE: u32 = WM_APP + 6;
/// The hand came to another row, the same way: wparam the row, counted from
/// the one the hold began on (`row_of`); lparam: which hold. To be answered
/// with `rows_shown`.
pub const WM_ROWS: u32 = WM_APP + 7;

/// Thread message telling the hook thread to pick up new settings.
const WM_RELOAD: u32 = WM_APP + 20;
/// Thread message telling the hook thread that the PC woke up: whatever was
/// held when it went to sleep is not any more, and the hooks may be gone.
const WM_WOKE: u32 = WM_APP + 21;
/// Two clicks of the trigger closer together than this are one.
const CLICK_APART_MS: u128 = 300;
/// Clicks following each other faster than this are a held button sending
/// its key combination over and over, not a hand: none of them counts.
const REPEAT_APART_MS: u128 = 120;
/// While something is held, this often it is checked that letting go of it
/// could still be seen (`blind`).
const WATCH_MS: u32 = 100;
/// The hooks are put in afresh this often while nothing is going on: Windows
/// drops a hook that was slow to answer, without a word, and until then the
/// trigger does nothing at all.
const REHOOK_MS: u32 = 5_000;
/// Marks key events we replay ourselves, so the hook lets them through. (An
/// arbitrary number: "KANK" in ASCII.)
const OWN_INPUT: usize = 0x4B41_4E4B;
/// All keys of a chord must go down within this long to count as one press.
/// Two keys next to each other are also typed in quick succession ("ty" in
/// "type"), so a two-key chord has to be much more simultaneous than that.
const CHORD_WINDOW_MS: u32 = 100;
const PAIR_WINDOW_MS: u32 = 40;
/// Keys sent by software (mouse software sending the chord for a button)
/// may come further apart, and nobody types with them, so they wait longer.
const INJECTED_WINDOW_MS: u32 = 500;
/// Travel this far (pixels) either way into a hold brings the minimap up.
const DRAG_SHOWS: i32 = 24;

// Where the hand is along the row, for the app to go there and for the
// minimap to follow it between cells (`hand`). Written by the hook
// thread, but for `SHOWN`, which the app writes as it gets there.
/// A hold is going on, and which one it is (they are counted).
static HOLDING: AtomicBool = AtomicBool::new(false);
static HOLD_ID: AtomicU32 = AtomicU32::new(0);
/// Which stretch of the hold: sideways travel begins anew, from the cell
/// come to, with every change of row.
static STRETCH: AtomicU32 = AtomicU32::new(0);
/// The cell the hand has come to, counted from the one the stretch began on,
/// and how far it is from that cell, in thousandths of a cell.
static ASKED: AtomicI32 = AtomicI32::new(0);
static FRACTION: AtomicI32 = AtomicI32::new(0);
/// What the app last said of a stretch (`slide_shown`): hold << 32 |
/// stretch << 24 | cell << 16 | cells to the left << 8 | cells to the right.
static SHOWN: AtomicU64 = AtomicU64::new(0);
/// In place of the cells to either side, when the app could not say.
const NO_ROOM: i32 = 0xFF;
/// The same up and down, for the whole hold: the row the hand has come to,
/// counted from the one the hold began on, how far the minimap leans towards
/// the next one (`Detent::lean`) in thousandths of a row, and
/// what the app last said (hold << 32 | row << 16 | rows above << 8 | rows
/// below).
static ROW_ASKED: AtomicI32 = AtomicI32::new(0);
static ROW_FRACTION: AtomicI32 = AtomicI32::new(0);
static ROWS_SHOWN: AtomicU64 = AtomicU64::new(0);
/// The board is open: a hold begun now goes from row to row as the hand
/// goes, not by notches (`Plane::linear`).
static BOARD_OPEN: AtomicBool = AtomicBool::new(false);

/// The app says when the board opens and closes.
pub fn set_board_open(open: bool) {
    BOARD_OPEN.store(open, Ordering::SeqCst);
}

/// The wparam of `WM_SLIDE` and `WM_DRAG`.
fn slide_wparam(stretch: u32, cell: i32) -> usize {
    (stretch as u8 as usize) << 16 | cell as i16 as u16 as usize
}

/// The stretch and the cell in the wparam of `WM_SLIDE` and `WM_DRAG`.
pub fn slide_of(wparam: usize) -> (u8, i32) {
    ((wparam >> 16) as u8, wparam as u16 as i16 as i32)
}

/// The app got to a stretch of `hold`: it is showing `cell` of it, and the
/// row has `room` cells to the left and to the right of the one the stretch
/// began on (`None` where the hand is not kept to a row).
pub fn slide_shown(hold: u32, stretch: u8, cell: i32, room: Option<(usize, usize)>) {
    let (left, right) = room.map_or((NO_ROOM, NO_ROOM), |(left, right)| (left.min(0xFE) as i32, right.min(0xFE) as i32));
    let cell = cell.clamp(i8::MIN as i32, i8::MAX as i32) as i8 as u8;
    SHOWN.store((hold as u64) << 32 | (stretch as u64) << 24 | (cell as u64) << 16 | (left as u64) << 8 | right as u64, Ordering::SeqCst);
}

/// What the app said of the stretch going on, if it has: the cell it is
/// showing, and the cells to the left and to the right.
fn shown() -> Option<(i32, Option<(i32, i32)>)> {
    let shown = SHOWN.load(Ordering::SeqCst);
    let now = (HOLD_ID.load(Ordering::SeqCst) as u64) << 8 | STRETCH.load(Ordering::SeqCst) as u8 as u64;
    let (left, right) = ((shown >> 8) as u8 as i32, shown as u8 as i32);
    (shown >> 24 == now).then(|| ((shown >> 16) as u8 as i8 as i32, (left != NO_ROOM).then_some((left, right))))
}

/// The row in the wparam of `WM_ROWS`.
pub fn row_of(wparam: usize) -> i32 {
    wparam as u16 as i16 as i32
}

/// The app got to `row` of `hold`, counted from the row the hold began on,
/// which has `room` rows above it and below it.
pub fn rows_shown(hold: u32, row: i32, room: (usize, usize)) {
    let (up, down) = (room.0.min(0xFE) as u64, room.1.min(0xFE) as u64);
    let row = row.clamp(i8::MIN as i32, i8::MAX as i32) as i8 as u8 as u64;
    ROWS_SHOWN.store((hold as u64) << 32 | row << 16 | up << 8 | down, Ordering::SeqCst);
}

/// What the app said of the rows in the hold going on, if it has: the row it
/// is showing, and the rows above and below the first.
fn rows_seen() -> Option<(i32, (i32, i32))> {
    let shown = ROWS_SHOWN.load(Ordering::SeqCst);
    ((shown >> 32) as u32 == HOLD_ID.load(Ordering::SeqCst))
        .then(|| ((shown >> 16) as u8 as i8 as i32, ((shown >> 8) as u8 as i32, shown as u8 as i32)))
}

/// Bumped by the hook thread before and after it writes where the hand is,
/// so that `hand` reads it whole: odd while a write is under way.
static WRITING: AtomicU32 = AtomicU32::new(0);

/// Writes where the hand is, as one piece for `hand`.
fn writing(write: impl FnOnce()) {
    WRITING.fetch_add(1, Ordering::SeqCst);
    write();
    WRITING.fetch_add(1, Ordering::SeqCst);
}

/// Where the hand is, for the minimap to follow it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Hand {
    /// How far the hand is from the cell the app is showing, in cells
    /// (positive rightwards): cells not gone to yet and the way towards the
    /// next one.
    pub along: f32,
    /// From row to row (positive downwards): rows not gone to yet, and the
    /// little the minimap leans towards the next one before the hand gets
    /// there (`Detent::lean`).
    pub across: f32,
    /// Changes whenever what `along` and `across` are counted from does (a
    /// new hold or stretch, the app getting to a cell or a row): they may
    /// jump then, with the hand itself still.
    pub counted_from: u64,
}

/// Where the hand is now; nothing while no hold is going on.
pub fn hand() -> Hand {
    loop {
        let before = WRITING.load(Ordering::SeqCst);
        if before % 2 == 1 {
            std::hint::spin_loop();
            continue;
        }
        let holding = HOLDING.load(Ordering::SeqCst);
        let (hold, stretch) = (HOLD_ID.load(Ordering::SeqCst), STRETCH.load(Ordering::SeqCst));
        let (shown_raw, rows_raw) = (SHOWN.load(Ordering::SeqCst), ROWS_SHOWN.load(Ordering::SeqCst));
        let along = (ASKED.load(Ordering::SeqCst) - shown().map_or(0, |(cell, _)| cell)) as f32 + FRACTION.load(Ordering::SeqCst) as f32 / 1000.0;
        let across = (ROW_ASKED.load(Ordering::SeqCst) - rows_seen().map_or(0, |(row, _)| row)) as f32
            + ROW_FRACTION.load(Ordering::SeqCst) as f32 / 1000.0;
        if WRITING.load(Ordering::SeqCst) != before {
            continue;
        }
        let mut counted_from = holding as u64 ^ (hold as u64) << 1 ^ (stretch as u64) << 33;
        for part in [shown_raw, rows_raw] {
            counted_from = counted_from.rotate_left(17) ^ part.wrapping_mul(0x9E37_79B9_7F4A_7C15);
        }
        return if holding { Hand { along, across, counted_from } } else { Hand { counted_from, ..Hand::default() } };
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Trigger {
    XButton1,
    XButton2,
    Middle,
    Key(u32),
    /// Several keys pressed together, e.g. "e+r+t".
    Chord(Vec<u32>),
}

/// The config-file name of a key (the inverse of `key_code`).
fn key_name(key: u32) -> String {
    match key {
        0x13 => "pause".into(),
        0x14 => "capslock".into(),
        0x20 => "space".into(),
        0x5D => "apps".into(),
        0x91 => "scrolllock".into(),
        0xA3 => "rctrl".into(),
        0xA5 => "ralt".into(),
        0x30..=0x39 | 0x41..=0x5A => (key as u8 as char).to_ascii_lowercase().to_string(),
        0x70..=0x87 => format!("f{}", key - 0x70 + 1),
        _ => format!("vk{key:02x}"),
    }
}

fn key_code(name: &str) -> Option<u32> {
    if let Some(hex) = name.strip_prefix("vk") {
        return u32::from_str_radix(hex, 16).ok().filter(|code| (1..=0xFE).contains(code));
    }
    Some(match name {
        "pause" => 0x13,
        "capslock" => 0x14,
        "space" => 0x20,
        "apps" => 0x5D,
        "scrolllock" => 0x91,
        "rctrl" => 0xA3,
        "ralt" => 0xA5,
        _ => {
            let mut chars = name.chars();
            match (chars.next(), chars.next()) {
                // Letters and digits share their virtual-key code with ASCII.
                (Some(c), None) if c.is_ascii_alphanumeric() => c.to_ascii_uppercase() as u32,
                _ => {
                    let n: u32 = name.strip_prefix('f')?.parse().ok()?;
                    (1..=24).contains(&n).then(|| 0x70 + n - 1)?
                }
            }
        }
    })
}

impl Trigger {
    /// How this trigger is written in the config file.
    pub fn to_config(&self) -> String {
        match self {
            Trigger::XButton1 => "xbutton1".into(),
            Trigger::XButton2 => "xbutton2".into(),
            Trigger::Middle => "middle".into(),
            Trigger::Key(key) => key_name(*key),
            Trigger::Chord(keys) => keys.iter().map(|key| key_name(*key)).collect::<Vec<_>>().join("+"),
        }
    }

    /// How this trigger is called when talking to the user.
    pub fn describe(&self) -> String {
        match self {
            Trigger::XButton1 => "Mouse back button".into(),
            Trigger::XButton2 => "Mouse forward button".into(),
            Trigger::Middle => "Mouse wheel button".into(),
            _ => self.to_config().to_uppercase(),
        }
    }

    fn parse_one(name: &str) -> Option<Trigger> {
        Some(match name {
            "xbutton1" => Trigger::XButton1,
            "xbutton2" => Trigger::XButton2,
            "middle" => Trigger::Middle,
            _ => {
                let keys: Option<Vec<u32>> = name.split('+').map(|key| key_code(key.trim())).collect();
                match keys?.as_slice() {
                    [key] => Trigger::Key(*key),
                    keys => Trigger::Chord(keys.to_vec()),
                }
            }
        })
    }

    /// Parses a comma-separated list such as "xbutton2, e+r+t". Returns the
    /// triggers understood and the entries that were not.
    pub fn parse_list(text: &str) -> (Vec<Trigger>, Vec<String>) {
        let mut triggers = Vec::new();
        let mut unknown = Vec::new();
        for name in text.split(',').map(|name| name.trim().to_ascii_lowercase()).filter(|name| !name.is_empty()) {
            match Trigger::parse_one(&name) {
                Some(trigger) => triggers.push(trigger),
                None => unknown.push(name),
            }
        }
        (triggers, unknown)
    }
}

pub fn dir_from_index(index: usize) -> Option<Dir> {
    [Dir::Left, Dir::Right, Dir::Up, Dir::Down].get(index).copied()
}

#[derive(Clone)]
struct Settings {
    /// The app window, as a plain number so it can cross threads.
    target: isize,
    triggers: Vec<Trigger>,
    /// How far the hand goes from one cell to the next, and from one row to
    /// the next: farther, so that rows are not changed by the way.
    step_x: i32,
    step_y: i32,
}

impl Settings {
    /// The keys of the chord among the triggers (only one is used).
    fn chord(&self) -> Vec<u32> {
        self.triggers
            .iter()
            .find_map(|t| match t {
                Trigger::Chord(keys) => Some(keys.clone()),
                _ => None,
            })
            .unwrap_or_default()
    }
}

/// While set, the next button or key combination pressed is reported as the
/// new trigger instead of being acted on.
static CAPTURING: AtomicBool = AtomicBool::new(false);
static CAPTURED: Mutex<Option<String>> = Mutex::new(None);

pub fn begin_capture() {
    CAPTURING.store(true, Ordering::SeqCst);
}

pub fn cancel_capture() {
    CAPTURING.store(false, Ordering::SeqCst);
}

pub fn take_captured() -> Option<String> {
    CAPTURED.lock().unwrap().take()
}

fn finish_capture(s: &Settings, result: Option<String>) {
    CAPTURING.store(false, Ordering::SeqCst);
    CAPTURE_KEYS.with(|keys| keys.borrow_mut().clear());
    let chosen = result.is_some();
    *CAPTURED.lock().unwrap() = result;
    post(s, WM_CAPTURED, chosen as usize);
}

/// Keys count as one combination from the first press to the first release.
fn capture_key(s: &Settings, key: u32, up: bool) -> bool {
    const ESCAPE: u32 = 0x1B;
    if !up {
        if key == ESCAPE {
            finish_capture(s, None);
        } else {
            CAPTURE_KEYS.with(|keys| {
                let mut keys = keys.borrow_mut();
                if !keys.contains(&key) {
                    keys.push(key);
                }
            });
        }
        return true;
    }
    let keys = CAPTURE_KEYS.with(|keys| std::mem::take(&mut *keys.borrow_mut()));
    // A single letter or digit as a hold-trigger would make it untypable.
    let typable = |key: u32| (0x30..=0x5A).contains(&key) || key == 0x20;
    match keys.as_slice() {
        [] => {}
        [only] if typable(*only) => {}
        keys => finish_capture(s, Some(keys.iter().map(|key| key_name(*key)).collect::<Vec<_>>().join("+"))),
    }
    true
}

/// Settings handed from the app to the hook thread.
static SHARED: Mutex<Option<Settings>> = Mutex::new(None);
static THREAD: AtomicU32 = AtomicU32::new(0);

// State of the hook thread.
thread_local! {
    static SETTINGS: RefCell<Option<Settings>> = const { RefCell::new(None) };
    /// Which triggers are down right now.
    static HELD: RefCell<Held> = RefCell::new(Held::default());
    static STEPPED: Cell<bool> = const { Cell::new(false) };
    /// The app was told of this hold travelling sideways (`WM_DRAG`).
    static DRAGGING: Cell<bool> = const { Cell::new(false) };
    /// Where the hand is up and down in this hold, and along the row in this
    /// stretch of it.
    static PLANE: Cell<Plane> = const { Cell::new(Plane::new()) };
    /// The hook thread's hidden window, which receives raw mouse input.
    static RAW_WINDOW: Cell<isize> = const { Cell::new(0) };
    /// Last position reported by an absolute pointer (remote desktop, pen).
    static LAST_ABSOLUTE: Cell<Option<(i32, i32)>> = const { Cell::new(None) };
    /// The chord among the triggers (one without keys when there is none).
    static CHORD: RefCell<Chord> = RefCell::new(Chord::default());
    /// The timer that gives up waiting for the rest of the chord, 0 for none.
    static CHORD_TIMER: Cell<usize> = const { Cell::new(0) };
    /// Keys pressed so far while a new trigger is being chosen.
    static CAPTURE_KEYS: RefCell<Vec<u32>> = const { RefCell::new(Vec::new()) };
    /// The mouse and the keyboard hook.
    static HOOKS: RefCell<Vec<HHOOK>> = const { RefCell::new(Vec::new()) };
    /// The timer that watches over a hold (`watch`), 0 for none.
    static WATCH_TIMER: Cell<usize> = const { Cell::new(0) };
    /// How far the mouse got from where it was when the hold began: where it
    /// is now, and the farthest it was.
    static STRAYED: Cell<((i32, i32), i32)> = const { Cell::new(((0, 0), 0)) };
    /// When a click of the trigger was last taken, and when the trigger was
    /// last clicked at all.
    static LAST_CLICK: Cell<Option<Instant>> = const { Cell::new(None) };
    static LAST_TAP: Cell<Option<Instant>> = const { Cell::new(None) };
    /// When the first key of the chord was held back.
    static CHORD_BEGAN: Cell<Option<Instant>> = const { Cell::new(None) };
    /// The key event being handled was sent by software, not a keyboard.
    static INJECTED: Cell<bool> = const { Cell::new(false) };
    /// When a chord given up as typing was last written to the log.
    static LAST_PARTIAL_LOG: Cell<Option<Instant>> = const { Cell::new(None) };
    /// This process' integrity level, and those found of other processes.
    static LEVELS: RefCell<(Option<u32>, Vec<(u32, Option<u32>)>)> = const { RefCell::new((None, Vec::new())) };
    /// The window that had the keyboard and the one under the cursor when
    /// the hold began.
    static PRESSED_OVER: Cell<(isize, isize)> = const { Cell::new((0, 0)) };
}

/// Starts the hook thread (once) and gives it these settings.
pub fn install(target: HWND, triggers: Vec<Trigger>, config: &Config) {
    *SHARED.lock().unwrap() = Some(Settings {
        target: target.0,
        triggers,
        step_x: config.step_x.max(20),
        step_y: config.step_y.max(20),
    });
    match THREAD.load(Ordering::SeqCst) {
        0 => {
            std::thread::spawn(hook_thread);
        }
        thread => unsafe {
            let _ = PostThreadMessageW(thread, WM_RELOAD, WPARAM(0), LPARAM(0));
        },
    }
}

/// The PC woke from sleep.
pub fn woke() {
    let thread = THREAD.load(Ordering::SeqCst);
    if thread != 0 {
        unsafe {
            let _ = PostThreadMessageW(thread, WM_WOKE, WPARAM(0), LPARAM(0));
        }
    }
}

pub fn uninstall() {
    let thread = THREAD.swap(0, Ordering::SeqCst);
    if thread != 0 {
        unsafe {
            let _ = PostThreadMessageW(thread, WM_QUIT, WPARAM(0), LPARAM(0));
        }
    }
}

fn reload() {
    let settings = SHARED.lock().unwrap().clone();
    // Keys held back for the old chord are typing now.
    let keys = settings.as_ref().map(Settings::chord).unwrap_or_default();
    let held_back = CHORD.with(|chord| {
        let mut old = chord.replace(Chord::new(keys));
        old.timeout()
    });
    replay(&held_back);
    watch_chord();
    #[cfg(feature = "debug-log")]
    if let Some(s) = &settings {
        debug_log!(
            "settings: triggers {:?}, step {}",
            s.triggers, s.step_x
        );
    }
    // What was held under the old settings may not be a trigger any more.
    end_hold("the settings changed");
    SETTINGS.with(|slot| *slot.borrow_mut() = settings);
}

/// Puts the hooks in, taking out the ones there were.
fn hook() {
    unsafe {
        let module = GetModuleHandleW(None).unwrap_or_default();
        let mouse = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), module, 0);
        let keyboard = SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_proc), module, 0);
        if mouse.is_err() || keyboard.is_err() {
            crate::app::log("installing the input hooks failed");
        }
        let old = HOOKS.replace([mouse, keyboard].into_iter().flatten().collect());
        for hook in old {
            let _ = UnhookWindowsHookEx(hook);
        }
    }
}

/// Now and then, while nothing is held or held back.
unsafe extern "system" fn rehook(_: HWND, _: u32, _: usize, _: u32) {
    if !held() && !CHORD.with(|chord| chord.borrow().waiting()) {
        hook();
    }
    // In the record once a minute, to show the hook thread alive.
    thread_local!(static ROUNDS: Cell<u32> = const { Cell::new(0) });
    if ROUNDS.replace((ROUNDS.get() + 1) % (60_000 / REHOOK_MS)) == 0 {
        debug_log!("hooks put in afresh every {REHOOK_MS} ms; held: {}", held());
    }
}

fn hook_thread() {
    unsafe {
        THREAD.store(GetCurrentThreadId(), Ordering::SeqCst);
        reload();
        let module = GetModuleHandleW(None).unwrap_or_default();
        let class = windows::core::w!("flick.input");
        RegisterClassW(&WNDCLASSW {
            lpfnWndProc: Some(raw_window_proc),
            hInstance: module.into(),
            lpszClassName: class,
            ..Default::default()
        });
        let window =
            CreateWindowExW(WINDOW_EX_STYLE(0), class, None, WINDOW_STYLE(0), 0, 0, 0, 0, HWND_MESSAGE, None, module, None);
        RAW_WINDOW.set(window.0);
        hook();
        SetTimer(None, 0, REHOOK_MS, Some(rehook));
        let mut message = MSG::default();
        while GetMessageW(&mut message, None, 0, 0).as_bool() {
            if message.hwnd.0 == 0 && message.message == WM_RELOAD {
                reload();
            } else if message.hwnd.0 == 0 && message.message == WM_WOKE {
                end_hold("the PC had gone to sleep");
                hook();
            } else {
                // Timer callbacks arrive here.
                DispatchMessageW(&message);
            }
        }
        // Whoever holds a trigger down as the app goes must not find the
        // cursor still kept in place by it.
        end_hold("the app is closing");
        for hook in HOOKS.take() {
            let _ = UnhookWindowsHookEx(hook);
        }
    }
}

fn post(s: &Settings, message: u32, wparam: usize) {
    unsafe {
        let hold = HOLD_ID.load(Ordering::SeqCst);
        let _ = PostMessageW(HWND(s.target), message, WPARAM(wparam), LPARAM(hold as isize));
    }
}

/// Starts or stops receiving raw input (only wanted while held): the mouse
/// for its movement, and both it and the keyboard to see the trigger let go
/// of even if the hooks do not.
fn listen_raw(on: bool) {
    let window = HWND(RAW_WINDOW.get());
    // Generic desktop devices: 2 is the mouse, 6 the keyboard.
    let devices = [2, 6].map(|usage| RAWINPUTDEVICE {
        usUsagePage: 1,
        usUsage: usage,
        dwFlags: if on { RIDEV_INPUTSINK } else { RIDEV_REMOVE },
        hwndTarget: if on { window } else { HWND(0) },
    });
    let registered = unsafe { RegisterRawInputDevices(&devices, std::mem::size_of::<RAWINPUTDEVICE>() as u32) };
    if let (true, Err(e)) = (on, registered) {
        // Without it the held trigger sees no movement at all.
        crate::app::log(&format!("raw mouse input could not be registered: {e}"));
    }
}

fn held() -> bool {
    HELD.with(|held| held.borrow().any())
}

fn press(source: Source) {
    let began = HELD.with(|held| held.borrow_mut().press(source));
    debug_log!("down {source:?}{}", if began { ": hold begins" } else { "" });
    if began {
        writing(|| {
            HOLD_ID.fetch_add(1, Ordering::SeqCst);
            STRETCH.store(1, Ordering::SeqCst);
            ASKED.store(0, Ordering::SeqCst);
            FRACTION.store(0, Ordering::SeqCst);
            ROW_ASKED.store(0, Ordering::SeqCst);
            ROW_FRACTION.store(0, Ordering::SeqCst);
        });
        PLANE.set(if BOARD_OPEN.load(Ordering::SeqCst) { Plane::linear() } else { Plane::new() });
        DRAGGING.set(false);
        HOLDING.store(true, Ordering::SeqCst);
        STEPPED.set(false);
        LAST_ABSOLUTE.set(None);
        STRAYED.set(((0, 0), 0));
        // Process ids come round again: a process found run as administrator
        // in an earlier hold may be gone, and its id another's by now.
        LEVELS.with(|levels| levels.borrow_mut().1.clear());
        PRESSED_OVER.set(unsafe { (GetForegroundWindow().0, under_cursor().map_or(0, |window| window.0)) });
        listen_raw(true);
        WATCH_TIMER.set(unsafe { SetTimer(None, 0, WATCH_MS, Some(watch)) });
    }
}

/// Stops what a hold keeps going: the raw input and the watching over it.
fn stop_listening() {
    HOLDING.store(false, Ordering::SeqCst);
    listen_raw(false);
    let timer = WATCH_TIMER.replace(0);
    if timer != 0 {
        unsafe {
            let _ = KillTimer(None, timer);
        }
    }
}

/// Ends the hold, whatever holds it, without waiting for that to be let go
/// of: for when letting go could not be seen, or does not matter any more.
fn end_hold(why: &str) {
    if HELD.with(|held| held.borrow_mut().clear()) {
        debug_log!("press ignored from here on, hold ended by the app: {why}");
        let _ = why;
        CHORD.with(|chord| chord.borrow_mut().let_go());
        stop_listening();
        // Never as a click: the user did not ask for the board.
        with_settings(|s| {
            post(s, WM_RELEASE, 0);
            false
        });
    }
}

/// The integrity level of a process (whether it runs as administrator).
unsafe fn level_of(process: HANDLE) -> Option<u32> {
    let mut token = HANDLE::default();
    OpenProcessToken(process, TOKEN_QUERY, &mut token).ok()?;
    let mut label = [0u8; 128];
    let mut len = 0u32;
    let got = GetTokenInformation(token, TokenIntegrityLevel, Some(label.as_mut_ptr().cast()), label.len() as u32, &mut len);
    let _ = CloseHandle(token);
    got.ok()?;
    let sid = (*(label.as_ptr() as *const TOKEN_MANDATORY_LABEL)).Label.Sid;
    Some(*GetSidSubAuthority(sid, (*GetSidSubAuthorityCount(sid)).saturating_sub(1) as u32))
}

/// Whether a window belongs to a process above this one (run as
/// administrator while this one is not). Windows keeps what is typed and
/// clicked there from the hooks and the raw input of a process below.
pub fn above_us(window: HWND) -> bool {
    if window.0 == 0 {
        return false;
    }
    let mut pid = 0u32;
    unsafe {
        GetWindowThreadProcessId(window, Some(&mut pid));
    }
    // The window is gone.
    if pid == 0 {
        return false;
    }
    LEVELS.with(|levels| {
        let mut levels = levels.borrow_mut();
        let ours = *levels.0.get_or_insert_with(|| unsafe { level_of(GetCurrentProcess()) }.unwrap_or(u32::MAX));
        let theirs = match levels.1.iter().find(|(known, _)| *known == pid) {
            Some((_, level)) => *level,
            None => {
                let level = unsafe {
                    OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok().and_then(|process| {
                        let level = level_of(process);
                        let _ = CloseHandle(process);
                        level
                    })
                };
                // Process ids come round again; a short memory is enough.
                if levels.1.len() >= 32 {
                    levels.1.clear();
                }
                levels.1.push((pid, level));
                level
            }
        };
        // A process that cannot even be asked is taken to be above.
        theirs.map_or(true, |theirs| theirs > ours)
    })
}

fn under_cursor() -> Option<HWND> {
    let mut cursor = POINT::default();
    unsafe { GetCursorPos(&mut cursor).is_ok().then(|| WindowFromPoint(cursor)) }
}

/// Whether a window can keep the trigger's letting go from us: one run as
/// administrator (`above`) that came there after the trigger went down.
/// The window the trigger went down over, or with the keyboard, cannot: the
/// hook saw it go down, so it sees it come up too. Taken for blind as well,
/// a window run as administrator anywhere about, or one whose process cannot
/// be asked (as a window just being opened), ended every hold at once: the
/// trigger did nothing until a click elsewhere gave the keyboard away.
fn hidden_by(now: isize, at_press: isize, above: impl FnOnce(isize) -> bool) -> bool {
    now != at_press && above(now)
}

/// Why letting go of the trigger could not be seen right now, if so.
fn blind() -> Option<&'static str> {
    unsafe {
        // Locked, or asked for permission on the secure desktop.
        match OpenInputDesktop(DESKTOP_CONTROL_FLAGS(0), false, DESKTOP_READOBJECTS) {
            Ok(desktop) => {
                let _ = CloseDesktop(desktop);
            }
            Err(_) => return Some("the screen is locked or asking for permission"),
        }
    }
    let (keyboard, cursor) = PRESSED_OVER.get();
    let above = |window: isize| above_us(HWND(window));
    if hidden_by(unsafe { GetForegroundWindow() }.0, keyboard, above) {
        return Some("a window run as administrator has the keyboard");
    }
    if under_cursor().is_some_and(|window| hidden_by(window.0, cursor, above)) {
        return Some("the cursor is on a window run as administrator");
    }
    None
}

/// Watches over a hold: where its end could not be seen it is ended at
/// once, or it would go on after the trigger was let go of.
unsafe extern "system" fn watch(_: HWND, _: u32, _: usize, _: u32) {
    if !held() {
        return stop_listening();
    }
    if let Some(why) = blind() {
        end_hold(why);
    }
}

fn release(s: &Settings, source: Source) {
    let ended = HELD.with(|held| held.borrow_mut().release(source));
    debug_log!("up {source:?}{}", if ended { ": hold ends" } else { "" });
    if ended {
        debug_log!(
            "hold ended as a {}; cursor movements held back meanwhile: {}",
            if STEPPED.get() { "move" } else { "click" },
            crate::debuglog::take_held_back()
        );
        stop_listening();
        // A click is the trigger pressed and let go of where it was. One
        // that travelled, only not far enough for a step, is a move given
        // up: it must not open the board.
        let slack = s.step_x / 3;
        let stayed = !STEPPED.get() && STRAYED.get().1 < slack;
        if !stayed && !STEPPED.get() {
            debug_log!("no click: the mouse had strayed {} (a click stays within {slack})", STRAYED.get().1);
        }
        // Nor is a click right upon a click one: nobody opens the board and
        // answers it that fast, but a button set up to send a key combination
        // may send it over and over while it is held. Only a click that was
        // taken counts here: a hand clicking away quickly must not find
        // every click after the first ignored for as long as it goes on.
        let now = Instant::now();
        let within = |last: Option<Instant>, ms: u128| last.is_some_and(|last| now.duration_since(last).as_millis() < ms);
        // Even those are not ignored for longer than a second on end.
        let again = stayed
            && (within(LAST_CLICK.get(), CLICK_APART_MS) || (within(LAST_TAP.get(), REPEAT_APART_MS) && within(LAST_CLICK.get(), 1000)));
        if again {
            debug_log!("click ignored: one was taken less than {CLICK_APART_MS} ms ago, or they come faster than a hand clicks");
        }
        let click = stayed && !again;
        if stayed {
            LAST_TAP.set(Some(now));
        }
        if click {
            LAST_CLICK.set(Some(now));
        }
        post(s, if click { WM_CLICK } else { WM_RELEASE }, 0);
    }
}

/// A trigger seen let go of by raw input, which tells of it as well as the
/// hook does and often first. Whichever comes second finds nothing held;
/// this one is what ends the hold when the hook never hears of it.
fn raw_release(source: Source) {
    if HELD.with(|held| held.borrow().has(source)) {
        debug_log!("up {source:?} (told by raw input)");
        with_settings(|s| {
            release(s, source);
            false
        });
    }
}

/// A key seen coming up by raw input.
fn raw_key_up(key: u32) {
    raw_release(Source::Key(key));
    if CHORD.with(|chord| chord.borrow().holds(key)) {
        CHORD.with(|chord| chord.borrow_mut().let_go());
        raw_release(Source::Chord);
    }
}

/// Raw mouse input while the trigger is held: the device's own movement,
/// unaffected by the cursor being frozen.
fn on_raw_input(handle: HRAWINPUT) {
    const MOUSE_MOVE_ABSOLUTE: u16 = 0x01;
    const MOUSE_VIRTUAL_DESKTOP: u16 = 0x02;
    // Buttons coming up, as raw input flags them.
    const BUTTONS_UP: [(u16, Source); 3] = [(0x0020, Source::Middle), (0x0080, Source::XButton1), (0x0200, Source::XButton2)];
    const KEY_UP: u16 = 0x01;
    const KEY_E0: u16 = 0x02;
    if !held() {
        return;
    }
    let mut raw = RAWINPUT::default();
    let mut size = std::mem::size_of::<RAWINPUT>() as u32;
    let header = std::mem::size_of::<RAWINPUTHEADER>() as u32;
    let read = unsafe { GetRawInputData(handle, RID_INPUT, Some(&mut raw as *mut _ as *mut _), &mut size, header) };
    if read == u32::MAX {
        return;
    }
    if raw.header.dwType == RIM_TYPEKEYBOARD.0 {
        let keyboard = unsafe { raw.data.keyboard };
        if keyboard.Flags & KEY_UP != 0 {
            // Raw input does not tell right from left Ctrl and Alt by the key.
            let right = keyboard.Flags & KEY_E0 != 0;
            raw_key_up(match keyboard.VKey {
                0x11 => if right { 0xA3 } else { 0xA2 },
                0x12 => if right { 0xA5 } else { 0xA4 },
                key => key as u32,
            });
        }
        return;
    }
    if raw.header.dwType != RIM_TYPEMOUSE.0 {
        return;
    }
    let mouse = unsafe { raw.data.mouse };
    let buttons = unsafe { mouse.Anonymous.Anonymous.usButtonFlags };
    for (flag, source) in BUTTONS_UP {
        if buttons & flag != 0 {
            raw_release(source);
        }
    }
    if !held() {
        return;
    }
    #[cfg(feature = "debug-log")]
    crate::debuglog::device(raw.header.hDevice);
    let (dx, dy) = if mouse.usFlags & MOUSE_MOVE_ABSOLUTE != 0 {
        // Absolute pointers report a position in 0..65535 across the screen;
        // the movement is the difference from the previous report.
        let (w, h) = unsafe {
            if mouse.usFlags & MOUSE_VIRTUAL_DESKTOP != 0 {
                (GetSystemMetrics(SM_CXVIRTUALSCREEN), GetSystemMetrics(SM_CYVIRTUALSCREEN))
            } else {
                (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN))
            }
        };
        let now = ((mouse.lLastX as i64 * w as i64 / 65535) as i32, (mouse.lLastY as i64 * h as i64 / 65535) as i32);
        match LAST_ABSOLUTE.replace(Some(now)) {
            Some(before) => (now.0 - before.0, now.1 - before.1),
            None => (0, 0),
        }
    } else {
        (mouse.lLastX, mouse.lLastY)
    };
    if dx != 0 || dy != 0 {
        with_settings(|s| {
            travel(s, dx, dy);
            false
        });
    }
}

unsafe extern "system" fn raw_window_proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if message == WM_INPUT {
        on_raw_input(HRAWINPUT(lparam.0));
    }
    DefWindowProcW(hwnd, message, wparam, lparam)
}

/// Adds mouse travel and emits steps.
///
/// Each way on its own (`Plane`): one cell per `step_x` of travel as far as
/// the hand goes, and one row each time up and down travel gathers `step_y`
/// (by notches, `Detent`; on the board one per `step_y` as the hand goes),
/// sideways travel moving the
/// hand along the row only and up and down travel from row to row only, so a
/// hand going slantwise does both at once. The app is told of the row or the
/// cell the hand has come to whenever that is another one (`WM_ROWS`,
/// `WM_SLIDE`): where the hand is, not that it took a step, so an app that
/// was busy meanwhile goes straight to where the hand is by then rather than
/// through every cell it passed. Coming to another row begins a new stretch
/// along the row, from the cell the app lands on there, with what the hand
/// went towards the next cell kept.
fn travel(s: &Settings, dx: i32, dy: i32) {
    let ((x, y), farthest) = STRAYED.get();
    let (x, y) = (x + dx, y + dy);
    STRAYED.set(((x, y), farthest.max(x.abs()).max(y.abs())));
    let (step, step_y) = (s.step_x, s.step_y);
    let mut plane = PLANE.get();
    if let Some((_, (up, down))) = rows_seen() {
        plane.rows.set_room(up, down, step_y);
    }
    if let Some((_, Some((left, right)))) = shown() {
        plane.cols.set_room(left, right, step);
    }
    let moved = plane.travel(dx, dy, (step, step_y));
    PLANE.set(plane);
    let (rows, cols) = (plane.rows, plane.cols);
    // In one piece for `hand`: a new stretch and where the hand is in it.
    writing(|| {
        if moved.row {
            STRETCH.fetch_add(1, Ordering::SeqCst);
        }
        ROW_ASKED.store(rows.cell(), Ordering::SeqCst);
        ROW_FRACTION.store((rows.lean(step_y) * 1000.0) as i32, Ordering::SeqCst);
        ASKED.store(cols.cell(), Ordering::SeqCst);
        FRACTION.store(cols.rest(step) * 1000 / step, Ordering::SeqCst);
    });
    let stretch = STRETCH.load(Ordering::SeqCst);
    if moved.row {
        // The app is told of the new stretch below, once it is on the row.
        DRAGGING.set(false);
        STEPPED.set(true);
        debug_log!("rows: row {}", rows.cell());
        post(s, WM_ROWS, rows.cell() as i16 as u16 as usize);
    }
    if moved.cell {
        STEPPED.set(true);
        debug_log!("slide: cell {} of stretch {stretch}", cols.cell());
        post(s, WM_SLIDE, slide_wparam(stretch, cols.cell()));
    }
    if !DRAGGING.get() && (STEPPED.get() || cols.rest(step).abs() >= DRAG_SHOWS || rows.gathered(step_y).abs() >= DRAG_SHOWS) {
        DRAGGING.set(true);
        post(s, WM_DRAG, slide_wparam(stretch, 0));
    }
    debug_log!("move {dx} {dy}: sideways {} of {step} and up and down {} of {step_y}", cols.rest(step), rows.gathered(step_y));
}

/// Returns true when the event belongs to the gesture and must not reach apps.
fn on_mouse(s: &Settings, message: u32, info: &MSLLHOOKSTRUCT) -> bool {
    let xbutton = (info.mouseData >> 16) as u16;
    let button = match message {
        WM_XBUTTONDOWN | WM_XBUTTONUP if xbutton == 1 => Some((Trigger::XButton1, Source::XButton1)),
        WM_XBUTTONDOWN | WM_XBUTTONUP if xbutton == 2 => Some((Trigger::XButton2, Source::XButton2)),
        WM_MBUTTONDOWN | WM_MBUTTONUP => Some((Trigger::Middle, Source::Middle)),
        _ => None,
    };
    let down = matches!(message, WM_XBUTTONDOWN | WM_MBUTTONDOWN);
    if CAPTURING.load(Ordering::SeqCst) {
        // Choosing a new trigger: the first button pressed is it.
        return match button {
            Some((trigger, _)) if down => {
                finish_capture(s, Some(trigger.to_config()));
                true
            }
            _ => false,
        };
    }
    if let Some((trigger, source)) = button {
        if s.triggers.contains(&trigger) {
            if down {
                press(source);
            } else {
                release(s, source);
            }
            return true;
        }
    }
    // The cursor stays put while the trigger is held; the movement itself is
    // read from raw input (`on_raw_input`).
    let held_back = message == WM_MOUSEMOVE && held();
    #[cfg(feature = "debug-log")]
    if held_back {
        crate::debuglog::held_back();
    }
    held_back
}

fn key_event(key: u32, up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(key as u16),
                wScan: unsafe { MapVirtualKeyW(key, MAPVK_VK_TO_VSC) } as u16,
                dwFlags: if up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) },
                time: 0,
                dwExtraInfo: OWN_INPUT,
            },
        },
    }
}

/// Gives key events to the apps: (key, is a release).
fn replay(events: &[(u32, bool)]) {
    if events.is_empty() {
        return;
    }
    let events: Vec<INPUT> = events.iter().map(|&(key, up)| key_event(key, up)).collect();
    unsafe {
        SendInput(&events, std::mem::size_of::<INPUT>() as i32);
    }
}

/// Keeps the timer that ends the wait for the rest of the chord in step with
/// the chord: running, from the first key held back, while any is.
fn watch_chord() {
    let (waiting, keys) = CHORD.with(|chord| (chord.borrow().waiting(), chord.borrow().len()));
    let timer = CHORD_TIMER.get();
    if waiting && timer == 0 {
        let window = if INJECTED.get() {
            INJECTED_WINDOW_MS
        } else if keys <= 2 {
            PAIR_WINDOW_MS
        } else {
            CHORD_WINDOW_MS
        };
        CHORD_BEGAN.set(Some(Instant::now()));
        CHORD_TIMER.set(unsafe { SetTimer(None, 0, window, Some(chord_timeout)) });
    } else if !waiting && timer != 0 {
        CHORD_TIMER.set(0);
        unsafe {
            let _ = KillTimer(None, timer);
        }
    }
}

/// The rest of the chord did not follow in time.
unsafe extern "system" fn chord_timeout(_: HWND, _: u32, _: usize, _: u32) {
    let held_back = CHORD.with(|chord| chord.borrow_mut().timeout());
    debug_log!("the chord's keys did not all go down in time: typing");
    // In the regular log too, now and then, so a lost press can be told.
    let now = Instant::now();
    let due = LAST_PARTIAL_LOG.get().map_or(true, |last| now.duration_since(last).as_secs() >= 20);
    if due && !held_back.is_empty() {
        LAST_PARTIAL_LOG.set(Some(now));
        let keys = held_back.iter().map(|(key, _)| key_name(*key)).collect::<Vec<_>>().join("+");
        let waited = CHORD_BEGAN.get().map_or(0, |began| began.elapsed().as_millis());
        crate::app::log(&format!("only {keys} of the trigger came within {waited} ms: typed instead"));
    }
    replay(&held_back);
    watch_chord();
}

fn on_key(s: &Settings, key: u32, up: bool) -> bool {
    if CAPTURING.load(Ordering::SeqCst) {
        return capture_key(s, key, up);
    }
    // Keys of a choice that was given up must not count towards the next.
    CAPTURE_KEYS.with(|keys| keys.borrow_mut().clear());
    if s.triggers.contains(&Trigger::Key(key)) {
        if up {
            release(s, Source::Key(key));
        } else {
            press(Source::Key(key));
        }
        return true;
    }
    let verdict = CHORD.with(|chord| {
        let mut chord = chord.borrow_mut();
        if chord.has(key) {
            chord.key(key, up)
        } else {
            chord.other_key(key, up)
        }
    });
    #[cfg(feature = "debug-log")]
    if !up && verdict == crate::hold::Verdict::default() && CHORD.with(|chord| chord.borrow().has(key)) {
        // How late the rest of the chord came says whether the wait is too short.
        if let Some(late) = CHORD_BEGAN.get().map(|began| began.elapsed().as_millis()).filter(|late| *late < 1000) {
            debug_log!("another key of the chord went down {late} ms after the first: too late, typing");
        }
    }
    replay(&verdict.replay);
    watch_chord();
    match verdict.trigger {
        Some(true) => press(Source::Chord),
        Some(false) => release(s, Source::Chord),
        None => {}
    }
    verdict.swallow
}

fn with_settings(f: impl FnOnce(&Settings) -> bool) -> bool {
    SETTINGS.with(|slot| slot.try_borrow().ok().and_then(|s| s.as_ref().map(f)).unwrap_or(false))
}

unsafe extern "system" fn mouse_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        let info = &*(lparam.0 as *const MSLLHOOKSTRUCT);
        if with_settings(|s| on_mouse(s, wparam.0 as u32, info)) {
            return LRESULT(1);
        }
    }
    CallNextHookEx(None, code, wparam, lparam)
}

unsafe extern "system" fn keyboard_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        let info = &*(lparam.0 as *const KBDLLHOOKSTRUCT);
        let up = matches!(wparam.0 as u32, WM_KEYUP | WM_SYSKEYUP);
        let down = matches!(wparam.0 as u32, WM_KEYDOWN | WM_SYSKEYDOWN);
        // Our own replayed keys pass; input injected by others (remote
        // desktop tools) is treated like a real keyboard.
        INJECTED.set(info.flags.0 & LLKHF_INJECTED.0 != 0);
        if (up || down) && info.dwExtraInfo != OWN_INPUT && with_settings(|s| on_key(s, info.vkCode, up)) {
            return LRESULT(1);
        }
    }
    CallNextHookEx(None, code, wparam, lparam)
}

#[cfg(test)]
mod tests {
    use super::hidden_by;

    /// A window run as administrator that already had the keyboard, or was
    /// under the cursor, as the trigger went down must not end the hold: the
    /// hook saw it go down there. One that came there since must.
    #[test]
    fn only_a_window_come_since_the_press_hides_the_letting_go() {
        let admin = |_| true;
        assert!(!hidden_by(7, 7, admin));
        assert!(hidden_by(8, 7, admin));
        assert!(!hidden_by(8, 7, |_| false));
        // No window has the keyboard at the press, then one run as
        // administrator gets it.
        assert!(hidden_by(8, 0, admin));
    }
}
