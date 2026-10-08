//! A record of what the input hooks saw and what they made of it, for
//! finding out why a gesture was not taken. Only builds with the `debug-log`
//! feature write it (to `debug-log.txt` next to the settings); in any other
//! build `debug_log!` compiles to nothing.
//!
//! It holds times, mouse travel, verdicts and the trigger going down and up.
//! Never what is typed, nor window titles, nor paths.

/// Adds a line to the record, written like `format!`.
macro_rules! debug_log {
    ($($arg:tt)*) => {
        #[cfg(feature = "debug-log")]
        $crate::debuglog::line(format!($($arg)*));
    };
}

/// Whether this build keeps the record.
pub const ON: bool = cfg!(feature = "debug-log");

/// Where the record is kept.
pub fn path() -> std::path::PathBuf {
    crate::config::config_path().with_file_name("debug-log.txt")
}

#[cfg(feature = "debug-log")]
pub use on::*;

#[cfg(feature = "debug-log")]
mod on {
    use std::{
        fs::File,
        io::Write,
        sync::{
            atomic::{AtomicU32, Ordering},
            mpsc, Mutex, OnceLock,
        },
        time::Instant,
    };
    use windows::Win32::{
        Foundation::HANDLE,
        UI::Input::{GetRawInputDeviceInfoW, RIDI_DEVICENAME},
    };

    /// The record stops growing here.
    const LIMIT: usize = 16 * 1024 * 1024;

    static STARTED: OnceLock<Instant> = OnceLock::new();
    static LINES: OnceLock<Mutex<mpsc::Sender<String>>> = OnceLock::new();
    static DEVICES: Mutex<Vec<isize>> = Mutex::new(Vec::new());
    static HELD_BACK: AtomicU32 = AtomicU32::new(0);

    /// Counts a cursor movement the hook kept from moving the cursor.
    pub fn held_back() {
        HELD_BACK.fetch_add(1, Ordering::Relaxed);
    }

    /// How many were counted since this was last asked.
    pub fn take_held_back() -> u32 {
        HELD_BACK.swap(0, Ordering::Relaxed)
    }

    /// The hooks must never wait for the disk, so lines are handed to a
    /// thread that does the writing.
    fn writer(lines: mpsc::Receiver<String>) {
        let Ok(mut file) = File::create(super::path()) else { return };
        let mut written = 0;
        for line in lines {
            if written > LIMIT {
                continue;
            }
            written += line.len() + 1;
            let _ = writeln!(file, "{line}");
            if written > LIMIT {
                let _ = writeln!(file, "(the record is full)");
            }
        }
    }

    /// Adds a line, headed by the seconds since the app started.
    pub fn line(text: String) {
        let at = STARTED.get_or_init(Instant::now).elapsed().as_secs_f64();
        let lines = LINES.get_or_init(|| {
            let (sender, receiver) = mpsc::channel();
            std::thread::spawn(move || writer(receiver));
            Mutex::new(sender)
        });
        if let Ok(lines) = lines.lock() {
            let _ = lines.send(format!("{at:10.3} {text}"));
        }
    }

    /// Notes, the first time a device reports movement, what kind it is: the
    /// hardware's own id (maker and model), which tells a touchpad from a mouse.
    pub fn device(handle: HANDLE) {
        let Ok(mut seen) = DEVICES.lock() else { return };
        if seen.contains(&handle.0) {
            return;
        }
        seen.push(handle.0);
        let mut name = [0u16; 256];
        let mut len = name.len() as u32;
        let got = unsafe { GetRawInputDeviceInfoW(handle, RIDI_DEVICENAME, Some(name.as_mut_ptr().cast()), &mut len) };
        let name = if got == u32::MAX || got == 0 { String::new() } else { String::from_utf16_lossy(&name[..(got as usize).min(name.len())]) };
        // `\\?\HID#VID_046D&PID_C52B&MI_01#…`: the part before the second
        // `#` is the model; what follows tells one unit from another.
        let model: String = name.trim_end_matches('\0').splitn(3, '#').take(2).collect::<Vec<_>>().join("#");
        line(format!("device {:x} is {model}", handle.0));
    }
}
