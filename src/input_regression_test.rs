// Input regression tests use thread-local Win32 spies; hardware probes still use native APIs.
static INPUT_TESTS: Mutex<()> = Mutex::new(());

mod legacy {
    use super::super::*;
    include!("input_legacy_test.rs");
    pub(super) fn mouse(s: &Settings, message: u32, info: &MSLLHOOKSTRUCT) -> bool {
        on_mouse(s, message, info)
    }
    pub(super) fn raw(handle: HRAWINPUT) {
        on_raw_input(handle);
    }
}

pub(crate) mod api {
    use super::*;
    use std::ffi::c_void;
    use windows::Win32::UI::Input::RAW_INPUT_DATA_COMMAND_FLAGS;

    #[derive(Clone, Debug, PartialEq)]
    pub(super) struct Read {
        pub command: u32,
        pub capacity: u32,
        pub header: u32,
        pub query: bool,
    }
    #[derive(Default)]
    pub(super) struct Calls {
        pub enabled: bool,
        pub packet: Vec<u8>,
        pub reads: Vec<Read>,
        pub registrations: Vec<Vec<(u16, u16, u32, isize)>>,
        pub messages: Vec<(u32, usize)>,
    }
    thread_local! { pub(super) static CALLS: RefCell<Calls> = RefCell::new(Calls::default()); }
    pub(crate) fn mocking() -> bool {
        CALLS.with(|c| c.borrow().enabled)
    }
    pub(crate) fn post(message: u32, wparam: usize) -> bool {
        CALLS.with(|c| {
            let mut c = c.borrow_mut();
            if !c.enabled {
                return false;
            }
            c.messages.push((message, wparam));
            true
        })
    }
    #[allow(non_snake_case)]
    pub(crate) unsafe fn GetRawInputData(
        handle: HRAWINPUT,
        command: RAW_INPUT_DATA_COMMAND_FLAGS,
        data: Option<*mut c_void>,
        size: *mut u32,
        header: u32,
    ) -> u32 {
        if !mocking() {
            return windows::Win32::UI::Input::GetRawInputData(handle, command, data, size, header);
        }
        CALLS.with(|c| {
            let mut c = c.borrow_mut();
            c.reads.push(Read {
                command: command.0,
                capacity: *size,
                header,
                query: data.is_none(),
            });
            let capacity = *size as usize;
            *size = c.packet.len() as u32;
            let Some(data) = data else { return 0 };
            if c.packet.len() > capacity {
                return u32::MAX;
            }
            std::ptr::copy_nonoverlapping(c.packet.as_ptr(), data.cast::<u8>(), c.packet.len());
            c.packet.len() as u32
        })
    }
    #[allow(non_snake_case)]
    pub(crate) unsafe fn RegisterRawInputDevices(
        devices: &[RAWINPUTDEVICE],
        size: u32,
    ) -> windows::core::Result<()> {
        if !mocking() {
            return windows::Win32::UI::Input::RegisterRawInputDevices(devices, size);
        }
        assert_eq!(size as usize, std::mem::size_of::<RAWINPUTDEVICE>());
        CALLS.with(|c| {
            c.borrow_mut().registrations.push(
                devices
                    .iter()
                    .map(|d| (d.usUsagePage, d.usUsage, d.dwFlags.0, d.hwndTarget.0))
                    .collect(),
            )
        });
        Ok(())
    }
}

fn settings() -> Settings {
    Settings {
        target: 0,
        triggers: vec![Trigger::XButton2],
        step_x: 260,
        step_y: 400,
        mouse_reverse: false,
        touchpad_reverse: true,
        mouse_notch_y: 100,
        touchpad_notch_y: 100,
    }
}

fn reset() {
    api::CALLS.with(|c| {
        *c.borrow_mut() = api::Calls {
            enabled: true,
            ..Default::default()
        }
    });
    HELD.with(|held| {
        held.borrow_mut().clear();
    });
    TOUCHPAD_ON.set(false);
    TOUCHPAD_BUTTONS.set(0);
    SETTINGS.with(|s| *s.borrow_mut() = Some(settings()));
    STEPPED.set(false);
    DRAGGING.set(false);
    STRAYED.set(((0, 0), 0));
    PLANE.set(Plane::new());
    LAST_ABSOLUTE.set(None);
    LAST_CLICK.set(None);
    LAST_TAP.set(None);
}

const EDGES: [(u32, u32, u32, u32); 5] = [
    (WM_LBUTTONDOWN, WM_LBUTTONUP, 0, 1),
    (WM_RBUTTONDOWN, WM_RBUTTONUP, 0, 2),
    (WM_MBUTTONDOWN, WM_MBUTTONUP, 0, 4),
    (WM_XBUTTONDOWN, WM_XBUTTONUP, 1 << 16, 8),
    (WM_XBUTTONDOWN, WM_XBUTTONUP, 2 << 16, 16),
];

#[test]
fn an_ordinary_down_recovers_every_button_after_a_lost_up() {
    for (down, up, mouse_data, bit) in EDGES {
        reset();
        TOUCHPAD_ON.set(true);
        HELD.with(|held| {
            held.borrow_mut().press(Source::Touchpad);
        });
        let info = MSLLHOOKSTRUCT {
            mouseData: mouse_data,
            ..Default::default()
        };
        let s = Settings {
            triggers: vec![],
            ..settings()
        };
        assert!(on_mouse(&s, down, &info));
        assert_eq!(TOUCHPAD_BUTTONS.get(), bit);
        HELD.with(|held| {
            held.borrow_mut().clear();
        }); // the swallowed up was lost
        assert!(!on_mouse(&s, down, &info));
        assert_eq!(TOUCHPAD_BUTTONS.get(), 0);
        assert!(!on_mouse(&s, up, &info));
    }
}

#[test]
fn lifecycle_resets_clear_stale_buttons_even_after_the_hold_has_ended() {
    let _guard = INPUT_TESTS.lock().unwrap();
    for enabled in [false, true] {
        for holding in [false, true] {
            for action in [0, 1, 2, 3, 4] {
                reset();
                TOUCHPAD_ON.set(enabled);
                if holding {
                    HELD.with(|held| {
                        held.borrow_mut().press(Source::Touchpad);
                    });
                }
                TOUCHPAD_BUTTONS.set(31);
                match action {
                    0 => end_hold("the hook lost a release"),
                    1 => assert!(thread_message(WM_WOKE)),
                    2 => assert!(thread_message(WM_RELOAD)),
                    3 => hook(), // hook replacement after timeout
                    _ => {
                        *TOUCHPAD_DEVICES.lock().unwrap() = Some(Vec::new());
                        assert!(thread_message(WM_TOUCHPAD));
                    }
                }
                assert_eq!(
                    TOUCHPAD_BUTTONS.get(),
                    0,
                    "holding={holding}, reset={action}"
                );
                HELD.with(|held| {
                    held.borrow_mut().clear();
                });
                let s = Settings {
                    triggers: vec![],
                    ..settings()
                };
                for (down, up, mouse_data, _) in EDGES {
                    let info = MSLLHOOKSTRUCT {
                        mouseData: mouse_data,
                        ..Default::default()
                    };
                    assert!(!on_mouse(&s, down, &info));
                    assert!(!on_mouse(&s, up, &info));
                }
            }
        }
    }
}

#[test]
fn a_new_touchpad_source_clears_old_bits_even_when_another_trigger_holds() {
    reset();
    HELD.with(|held| {
        held.borrow_mut().press(Source::Key(0x7C));
    });
    TOUCHPAD_BUTTONS.set(31);
    press(Source::Touchpad);
    assert_eq!(TOUCHPAD_BUTTONS.get(), 0);
    TOUCHPAD_BUTTONS.set(1);
    press(Source::Touchpad); // repeated down is still the same hold
    assert_eq!(TOUCHPAD_BUTTONS.get(), 1);
}

fn packet(kind: u32, len: usize) -> Vec<u8> {
    let mut packet = vec![0; len];
    packet[..4].copy_from_slice(&kind.to_le_bytes());
    packet[4..8].copy_from_slice(&(len as u32).to_le_bytes());
    packet
}

#[derive(Debug, PartialEq)]
struct Outcome {
    reads: Vec<api::Read>,
    registrations: Vec<Vec<(u16, u16, u32, isize)>>,
    messages: Vec<(u32, usize)>,
    strayed: ((i32, i32), i32),
    held: bool,
    sources: Vec<Source>,
    last_absolute: Option<(i32, i32)>,
    stepped: bool,
}

fn raw_outcome(bytes: &[u8], holding: bool, baseline: bool) -> Outcome {
    reset();
    if holding {
        HELD.with(|held| {
            held.borrow_mut().press(Source::Key(0x7C));
        });
    }
    api::CALLS.with(|c| c.borrow_mut().packet = bytes.to_vec());
    if baseline {
        legacy::raw(HRAWINPUT(0));
    } else {
        on_raw_input(HRAWINPUT(0));
    }
    api::CALLS.with(|c| {
        let c = c.borrow();
        Outcome {
            reads: c.reads.clone(),
            registrations: c.registrations.clone(),
            messages: c.messages.clone(),
            strayed: STRAYED.get(),
            held: held(),
            sources: [
                Source::XButton1,
                Source::XButton2,
                Source::Middle,
                Source::Key(0x7C),
                Source::Touchpad,
            ]
            .into_iter()
            .filter(|source| HELD.with(|held| held.borrow().has(*source)))
            .collect(),
            last_absolute: LAST_ABSOLUTE.get(),
            stepped: STEPPED.get(),
        }
    })
}

#[test]
fn feature_off_matches_pre_feature_hook_and_raw_packets_byte_for_byte() {
    let _guard = INPUT_TESTS.lock().unwrap();
    assert!(!Config::default().touchpad.three_finger);
    let offset = std::mem::offset_of!(RAWINPUT, data);
    let mut mouse = packet(RIM_TYPEMOUSE.0, std::mem::size_of::<RAWINPUT>());
    mouse[offset + std::mem::offset_of!(windows::Win32::UI::Input::RAWMOUSE, lLastX)..][..4]
        .copy_from_slice(&12i32.to_le_bytes());
    mouse[offset + std::mem::offset_of!(windows::Win32::UI::Input::RAWMOUSE, lLastY)..][..4]
        .copy_from_slice(&(-7i32).to_le_bytes());
    let mut absolute = mouse.clone();
    absolute[offset..offset + 2].copy_from_slice(&1u16.to_le_bytes());
    let mut key = packet(RIM_TYPEKEYBOARD.0, std::mem::size_of::<RAWINPUT>());
    key[offset + std::mem::offset_of!(windows::Win32::UI::Input::RAWKEYBOARD, Flags)..][..2]
        .copy_from_slice(&1u16.to_le_bytes());
    key[offset + std::mem::offset_of!(windows::Win32::UI::Input::RAWKEYBOARD, VKey)..][..2]
        .copy_from_slice(&0x7Cu16.to_le_bytes());
    let mut button = mouse.clone();
    button[offset + 4..offset + 6].copy_from_slice(&0x200u16.to_le_bytes());
    for bytes in [
        mouse,
        absolute,
        key,
        button,
        packet(RIM_TYPEHID.0, std::mem::size_of::<RAWINPUT>()),
    ] {
        for holding in [false, true] {
            let expected = raw_outcome(&bytes, holding, true);
            let actual = raw_outcome(&bytes, holding, false);
            assert_eq!(actual, expected, "kind {}, holding {holding}", bytes[0]);
            assert_eq!(actual.reads.len(), usize::from(holding));
            for read in &actual.reads {
                assert!(!read.query);
                assert_eq!(read.capacity as usize, std::mem::size_of::<RAWINPUT>());
            }
            assert!(actual
                .registrations
                .iter()
                .flatten()
                .all(|(page, _, _, _)| *page != 0x0D));
        }
    }
    for holding in [false, true] {
        for (down, up, mouse_data, _) in EDGES {
            for message in [down, up, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_MOUSEHWHEEL] {
                let info = MSLLHOOKSTRUCT {
                    mouseData: mouse_data,
                    ..Default::default()
                };
                let s = if holding {
                    settings()
                } else {
                    Settings {
                        triggers: vec![],
                        ..settings()
                    }
                };
                let run = |baseline| {
                    reset();
                    if holding {
                        HELD.with(|held| {
                            held.borrow_mut().press(Source::Key(0x7C));
                        });
                    }
                    let swallowed = if baseline {
                        legacy::mouse(&s, message, &info)
                    } else {
                        on_mouse(&s, message, &info)
                    };
                    let sources: Vec<_> = [
                        Source::XButton1,
                        Source::XButton2,
                        Source::Middle,
                        Source::Key(0x7C),
                    ]
                    .into_iter()
                    .filter(|source| HELD.with(|held| held.borrow().has(*source)))
                    .collect();
                    assert_eq!(TOUCHPAD_BUTTONS.get(), 0);
                    api::CALLS.with(|c| {
                        let c = c.borrow();
                        (
                            swallowed,
                            sources,
                            c.registrations.clone(),
                            c.messages.clone(),
                        )
                    })
                };
                assert_eq!(run(false), run(true));
            }
        }
    }
    reset();
    *TOUCHPAD_DEVICES.lock().unwrap() = Some(Vec::new());
    assert!(thread_message(WM_TOUCHPAD));
    api::CALLS.with(|c| {
        assert!(
            c.borrow().registrations.is_empty(),
            "OFF registered/unregistered HID"
        )
    });
}

#[test]
fn shared_legacy_processing_is_unchanged_from_the_frozen_tag() {
    let baseline = include_str!("input_legacy_test.rs");
    let current = include_str!("input.rs");
    let hook = current
        .split_once("fn on_mouse_trigger(")
        .unwrap()
        .1
        .split_once("{\n")
        .unwrap()
        .1
        .split_once("\nfn key_event(")
        .unwrap()
        .0
        .trim();
    let old_hook = baseline
        .split_once("fn on_mouse(")
        .unwrap()
        .1
        .split_once("{\n")
        .unwrap()
        .1
        .trim();
    assert_eq!(hook, old_hook);
    let raw = current
        .split_once("fn raw_mouse_input(")
        .unwrap()
        .1
        .split_once("    if raw.header.dwType == RIM_TYPEKEYBOARD.0 {")
        .unwrap()
        .1
        .split_once("\nfn touchpad_event(")
        .unwrap()
        .0
        .trim();
    let old_raw = baseline
        .split_once("    if raw.header.dwType == RIM_TYPEKEYBOARD.0 {")
        .unwrap()
        .1
        .split_once("\nfn on_mouse(")
        .unwrap()
        .0
        .trim();
    assert_eq!(raw, old_raw);
}

#[test]
fn touchpad_raw_mouse_releases_other_triggers_without_counting_mouse_movement() {
    reset();
    TOUCHPAD_ON.set(true);
    HELD.with(|held| {
        let mut held = held.borrow_mut();
        held.press(Source::Touchpad);
        held.press(Source::Middle);
    });
    let offset = std::mem::offset_of!(RAWINPUT, data);
    let mut mouse = packet(RIM_TYPEMOUSE.0, std::mem::size_of::<RAWINPUT>());
    mouse[offset..offset + 2].copy_from_slice(&1u16.to_le_bytes());
    mouse[offset + 4..offset + 6].copy_from_slice(&0x20u16.to_le_bytes());
    mouse[offset + std::mem::offset_of!(windows::Win32::UI::Input::RAWMOUSE, lLastX)..][..4]
        .copy_from_slice(&900i32.to_le_bytes());
    mouse[offset + std::mem::offset_of!(windows::Win32::UI::Input::RAWMOUSE, lLastY)..][..4]
        .copy_from_slice(&800i32.to_le_bytes());
    api::CALLS.with(|c| c.borrow_mut().packet = mouse);
    on_raw_input(HRAWINPUT(0));
    HELD.with(|held| {
        let held = held.borrow();
        assert!(held.has(Source::Touchpad));
        assert!(!held.has(Source::Middle));
    });
    assert_eq!(STRAYED.get(), ((0, 0), 0));
    assert_eq!(LAST_ABSOLUTE.get(), None);
    api::CALLS.with(|c| {
        let c = c.borrow();
        assert_eq!(c.reads.len(), 1);
        assert!(c.messages.is_empty());
    });
}

#[test]
fn enabled_raw_reads_once_into_a_bounded_stack_buffer_and_skips_idle_mouse_keys() {
    for kind in [RIM_TYPEMOUSE.0, RIM_TYPEKEYBOARD.0, RIM_TYPEHID.0] {
        for size in [std::mem::size_of::<RAWINPUT>(), 1024, 1025] {
            reset();
            TOUCHPAD_ON.set(true);
            STRAYED.set(((123, 456), 789));
            api::CALLS.with(|c| c.borrow_mut().packet = packet(kind, size));
            on_raw_input(HRAWINPUT(0));
            assert_eq!(STRAYED.get(), ((123, 456), 789));
            assert!(!held());
            api::CALLS.with(|c| {
                let c = c.borrow();
                assert_eq!(c.reads.len(), 1);
                assert_eq!(c.reads[0].capacity, 1024);
                assert!(!c.reads[0].query);
                assert!(c.messages.is_empty());
                assert!(c.registrations.is_empty());
            });
        }
    }
}
