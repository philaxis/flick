//! Device discovery and registry reads run on the app thread, never in a hook.
use super::{cm_per_unit, Contact, Event, Frame, Gesture};
use std::{
    cell::RefCell,
    sync::atomic::{AtomicBool, Ordering},
    time::Instant,
};
use windows::{
    core::{w, PCWSTR},
    Win32::{
        Devices::HumanInterfaceDevice::*,
        Foundation::{HANDLE, HWND},
        System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD},
        UI::Input::*,
    },
};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Status {
    #[default]
    Absent,
    Off,
    Conflict,
    Active,
    Unavailable,
}

pub const NOTICE: &str =
    "Windows' three-finger gestures (swipes and taps) must be set to Nothing to use this";
static REGISTRATION_FAILED: AtomicBool = AtomicBool::new(false);

pub fn registration_failed(failed: bool) {
    REGISTRATION_FAILED.store(failed, Ordering::Relaxed);
}

const KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\PrecisionTouchPad");

fn value(name: PCWSTR, absent: Option<u32>) -> Option<u32> {
    let mut value = 0u32;
    let mut size = 4u32;
    let result = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            KEY,
            name,
            RRF_RT_REG_DWORD,
            None,
            Some((&mut value as *mut u32).cast()),
            Some(&mut size),
        )
    };
    if result == windows::Win32::Foundation::ERROR_FILE_NOT_FOUND {
        return absent;
    }
    result.ok().ok()?;
    Some(value)
}

fn gestures_off() -> bool {
    // Missing global values use Windows defaults, which enable gestures.
    let modes = [
        value(w!("ThreeFingerSlideEnabled"), None),
        value(w!("ThreeFingerTapEnabled"), None),
    ];
    // shortcut: conflicting advanced mappings stay inactive even with global Nothing,
    // until Windows' undocumented precedence can be verified on hardware.
    let advanced = [
        w!("ThreeFingerUp"),
        w!("ThreeFingerDown"),
        w!("ThreeFingerLeft"),
        w!("ThreeFingerRight"),
        w!("ThreeFingerTap"),
    ]
    .map(|name| value(name, Some(0)));
    modes
        .iter()
        .chain(advanced.iter())
        .all(|value| *value == Some(0))
}

thread_local! {
    static KNOWN: RefCell<(Vec<isize>, Status)> = const { RefCell::new((Vec::new(), Status::Absent)) };
}

/// Called at startup and by the existing CONFIG timer, including when the config is unchanged.
pub fn poll(enabled: bool) -> Status {
    let handles = devices();
    KNOWN.with(|known| {
        let mut known = known.borrow_mut();
        let status = if handles.is_empty() {
            Status::Absent
        } else if !enabled {
            Status::Off
        } else if !gestures_off() {
            Status::Conflict
        } else if REGISTRATION_FAILED.load(Ordering::Relaxed) {
            Status::Unavailable
        } else {
            Status::Active
        };
        if status == Status::Unavailable
            && REGISTRATION_FAILED.load(Ordering::Relaxed)
            && known.0 == handles
        {
            if known.1 != status {
                crate::app::log("precision touchpad raw input registration failed");
            }
            known.1 = status;
            return status;
        }
        if known.0 != handles || known.1 != status {
            let readers = if status == Status::Active {
                handles
                    .iter()
                    .filter_map(|handle| Device::open(*handle))
                    .collect::<Vec<_>>()
            } else {
                Vec::new()
            };
            let status = if status == Status::Active && readers.is_empty() {
                Status::Unavailable
            } else {
                status
            };
            crate::input::update_touchpad(readers);
            *known = (handles, status);
        }
        known.1
    })
}

pub fn status() -> Status {
    KNOWN.with(|known| known.borrow().1)
}

#[cfg(test)]
pub fn probe_devices() -> Vec<Device> {
    devices().into_iter().filter_map(Device::open).inspect(|device| {
        eprintln!("touchpad device {}: {} contact slots", device.handle, device.slots.len());
        for slot in &device.slots {
            for (name, cap) in [("X", slot.x), ("Y", slot.y)] {
                eprintln!("  link {} report {} {name}: logical {}..{} physical {}..{} units {:#x} exponent {:#x}",
                    slot.link, slot.report, cap.LogicalMin, cap.LogicalMax, cap.PhysicalMin, cap.PhysicalMax, cap.Units, cap.UnitsExp);
            }
        }
    }).collect()
}

fn devices() -> Vec<isize> {
    unsafe {
        let mut count = 0;
        let size = std::mem::size_of::<RAWINPUTDEVICELIST>() as u32;
        if GetRawInputDeviceList(None, &mut count, size) == u32::MAX || count > 1024 {
            return Vec::new();
        }
        let mut list = vec![RAWINPUTDEVICELIST::default(); count as usize];
        let read = GetRawInputDeviceList(Some(list.as_mut_ptr()), &mut count, size);
        if read == u32::MAX {
            return Vec::new();
        }
        let mut handles = Vec::new();
        for device in list
            .iter()
            .take(read as usize)
            .filter(|d| d.dwType == RIM_TYPEHID)
        {
            let mut info = RID_DEVICE_INFO {
                cbSize: std::mem::size_of::<RID_DEVICE_INFO>() as u32,
                ..Default::default()
            };
            let mut bytes = info.cbSize;
            if GetRawInputDeviceInfoW(
                device.hDevice,
                RIDI_DEVICEINFO,
                Some((&mut info as *mut RID_DEVICE_INFO).cast()),
                &mut bytes,
            ) == u32::MAX
            {
                continue;
            }
            let hid = info.Anonymous.hid;
            if hid.usUsagePage == 0x0D && hid.usUsage == 0x05 {
                handles.push(device.hDevice.0);
            }
        }
        handles.sort_unstable();
        handles
    }
}

pub fn register(window: HWND, on: bool) -> bool {
    let device = RAWINPUTDEVICE {
        usUsagePage: 0x0D,
        usUsage: 0x05,
        dwFlags: if on { RIDEV_INPUTSINK } else { RIDEV_REMOVE },
        hwndTarget: if on { window } else { HWND(0) },
    };
    unsafe {
        RegisterRawInputDevices(&[device], std::mem::size_of::<RAWINPUTDEVICE>() as u32).is_ok()
    }
}

struct Slot {
    link: u16,
    report: u8,
    x: HIDP_VALUE_CAPS,
    y: HIDP_VALUE_CAPS,
}

pub struct Device {
    pub handle: isize,
    data: Vec<u64>, // aligned owned preparsed data, not a borrowed Windows pointer
    slots: Vec<Slot>,
    frame: Frame,
}

impl Device {
    fn prep(&self) -> PHIDP_PREPARSED_DATA {
        PHIDP_PREPARSED_DATA(self.data.as_ptr() as isize)
    }

    fn cap(&self, page: u16, link: u16, usage: u16) -> Option<HIDP_VALUE_CAPS> {
        let mut caps = [HIDP_VALUE_CAPS::default(); 16];
        let mut count = caps.len() as u16;
        let result = unsafe {
            HidP_GetSpecificValueCaps(
                HidP_Input,
                page,
                link,
                usage,
                caps.as_mut_ptr(),
                &mut count,
                self.prep(),
            )
        };
        (result == HIDP_STATUS_SUCCESS && count > 0).then_some(caps[0])
    }

    fn open(handle: isize) -> Option<Self> {
        unsafe {
            let mut bytes = 0;
            if GetRawInputDeviceInfoW(HANDLE(handle), RIDI_PREPARSEDDATA, None, &mut bytes)
                == u32::MAX
                || bytes == 0
                || bytes > 65536
            {
                return None;
            }
            let mut data = vec![0u64; (bytes as usize + 7) / 8];
            if GetRawInputDeviceInfoW(
                HANDLE(handle),
                RIDI_PREPARSEDDATA,
                Some(data.as_mut_ptr().cast()),
                &mut bytes,
            ) == u32::MAX
            {
                return None;
            }
            let mut device = Self {
                handle,
                data,
                slots: Vec::new(),
                frame: Frame::default(),
            };
            let mut caps = HIDP_CAPS::default();
            if HidP_GetCaps(device.prep(), &mut caps) != HIDP_STATUS_SUCCESS
                || caps.NumberInputValueCaps > 1024
            {
                return None;
            }
            let mut values = vec![HIDP_VALUE_CAPS::default(); caps.NumberInputValueCaps as usize];
            let mut count = caps.NumberInputValueCaps;
            if HidP_GetValueCaps(HidP_Input, values.as_mut_ptr(), &mut count, device.prep())
                != HIDP_STATUS_SUCCESS
            {
                return None;
            }
            for cap in values.iter().take(count as usize) {
                let id = if cap.IsRange.0 != 0 {
                    cap.Anonymous.Range.UsageMin <= 0x51 && cap.Anonymous.Range.UsageMax >= 0x51
                } else {
                    cap.Anonymous.NotRange.Usage == 0x51
                };
                if cap.UsagePage != 0x0D || !id {
                    continue;
                }
                let link = cap.LinkCollection;
                let (x, y) = (device.cap(1, link, 0x30)?, device.cap(1, link, 0x31)?);
                device.slots.push(Slot {
                    link,
                    report: cap.ReportID,
                    x,
                    y,
                });
            }
            if device.slots.is_empty()
                || device.slots.len() > 10
                || device.cap(0x0D, 0, 0x54).is_none()
            {
                return None;
            }
            Some(device)
        }
    }

    fn value(&self, page: u16, link: u16, usage: u16, report: &[u8]) -> Option<u32> {
        let mut value = 0;
        let status = unsafe {
            HidP_GetUsageValue(
                HidP_Input,
                page,
                link,
                usage,
                &mut value,
                self.prep(),
                report,
            )
        };
        (status == HIDP_STATUS_SUCCESS).then_some(value)
    }

    fn position(
        &self,
        slot: &Slot,
        cap: &HIDP_VALUE_CAPS,
        usage: u16,
        report: &[u8],
    ) -> Option<f64> {
        let value = self.value(1, slot.link, usage, report)?;
        let logical = if cap.LogicalMin < 0 && (1..32).contains(&cap.BitSize) {
            ((value << (32 - cap.BitSize)) as i32) >> (32 - cap.BitSize)
        } else {
            value as i32
        };
        if logical < cap.LogicalMin || logical > cap.LogicalMax {
            return None;
        }
        // shortcut: unknown physical units assume a 10 x 6 cm pad, upgrade with a device calibration if needed.
        let scale = cm_per_unit(
            (cap.LogicalMin, cap.LogicalMax),
            (cap.PhysicalMin, cap.PhysicalMax),
            cap.Units,
            cap.UnitsExp,
            if usage == 0x30 { 10.0 } else { 6.0 },
        );
        Some(logical as f64 * scale)
    }

    fn report(&mut self, report: &[u8]) -> Option<Vec<Contact>> {
        let count = self.value(0x0D, 0, 0x54, report)? as usize;
        let scan = self.value(0x0D, 0, 0x56, report)?;
        let mut records = Vec::new();
        for slot in &self.slots {
            if slot.report != 0 && report.first().copied() != Some(slot.report) {
                continue;
            }
            let id = self.value(0x0D, slot.link, 0x51, report)?;
            let mut usages = [0u16; 32];
            let mut length = usages.len() as u32;
            let mut copy = report.to_vec();
            let status = unsafe {
                HidP_GetUsages(
                    HidP_Input,
                    0x0D,
                    slot.link,
                    usages.as_mut_ptr(),
                    &mut length,
                    self.prep(),
                    &mut copy,
                )
            };
            if status != HIDP_STATUS_SUCCESS || length as usize > usages.len() {
                return None;
            }
            let usages = &usages[..length as usize];
            let contact = if usages.contains(&0x42) {
                Some(Contact {
                    id,
                    confident: usages.contains(&0x47),
                    x: self.position(slot, &slot.x, 0x30, report)?,
                    y: self.position(slot, &slot.y, 0x31, report)?,
                })
            } else {
                None
            };
            records.push((id, contact));
        }
        debug_log!("touchpad report: device {} scan {scan} count {count} records {records:?} bytes {report:02x?}", self.handle);
        #[cfg(test)]
        eprintln!(
            "touchpad report: scan {scan} count {count} records {records:?} bytes {report:02x?}"
        );
        self.frame.push(scan, count, &records)
    }
}

pub struct Reader {
    pub devices: Vec<Device>,
    owner: Option<isize>,
    gesture: Gesture,
    clock: Instant,
}

impl Default for Reader {
    fn default() -> Self {
        Self {
            devices: Vec::new(),
            owner: None,
            gesture: Gesture::default(),
            clock: Instant::now(),
        }
    }
}

impl Reader {
    pub fn new(devices: Vec<Device>) -> Self {
        Self {
            devices,
            ..Self::default()
        }
    }
    pub fn cancel(&mut self) {
        self.gesture.cancel();
    }
    pub fn report(&mut self, device: isize, report: &[u8]) -> Option<Event> {
        let contacts = self
            .devices
            .iter_mut()
            .find(|d| d.handle == device)?
            .report(report)?;
        if self.owner.is_some_and(|owner| owner != device) {
            return None;
        }
        if !contacts.is_empty() {
            self.owner = Some(device);
        }
        let event = self
            .gesture
            .update(&contacts, self.clock.elapsed().as_millis() as u64);
        #[cfg(test)]
        eprintln!("touchpad frame: {} contacts {event:?}", contacts.len());
        if contacts.is_empty() {
            self.owner = None;
        }
        Some(event)
    }
}
