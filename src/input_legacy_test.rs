// Frozen from v0.5.15-pre.3 for feature-off regression comparisons.
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

