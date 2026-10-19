//! Device discovery and registry reads run on the app thread, never in a hook.
use super::{cm_per_unit, logical_range, windows_gesture_conflict, Contact, Event, Frame, Gesture};
use std::{
    cell::RefCell,
    sync::atomic::{AtomicBool, Ordering},
};
use windows::{
    core::{w, PCWSTR},
    Win32::{
        Devices::HumanInterfaceDevice::*,
        Foundation::{CloseHandle, HANDLE, HWND},
        Storage::FileSystem::{CreateFileW, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING},
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
    if result == Err(windows::Win32::Foundation::ERROR_FILE_NOT_FOUND.into()) {
        return absent;
    }
    result.ok()?;
    Some(value)
}

/// Diagnostics go to log.txt, each at most once per device set or status change.
fn note(message: &str) {
    #[cfg(not(test))]
    crate::app::log(&format!("touchpad: {message}"));
    #[cfg(test)]
    eprintln!("touchpad: {message}");
}

/// Windows' choices as read, and why they block the trigger (None when they do not).
fn gesture_settings() -> (String, Option<&'static str>) {
    let slide = value(w!("ThreeFingerSlideEnabled"), None);
    let tap = value(w!("ThreeFingerTapEnabled"), None);
    let names = ["ThreeFingerUp", "ThreeFingerDown", "ThreeFingerLeft", "ThreeFingerRight", "ThreeFingerTap"];
    let advanced = [
        w!("ThreeFingerUp"),
        w!("ThreeFingerDown"),
        w!("ThreeFingerLeft"),
        w!("ThreeFingerRight"),
        w!("ThreeFingerTap"),
    ]
    .map(|name| value(name, None));
    let show = |value: Option<u32>| value.map_or("missing".to_owned(), |value| value.to_string());
    let mut read = format!("ThreeFingerSlideEnabled={} ThreeFingerTapEnabled={}", show(slide), show(tap));
    for (name, value) in names.iter().zip(advanced) {
        read += &format!(" {name}={}", show(value));
    }
    (read, windows_gesture_conflict(slide, tap, advanced))
}

thread_local! {
    static KNOWN: RefCell<(Vec<isize>, Status)> = const { RefCell::new((Vec::new(), Status::Absent)) };
}

/// Called at startup and by the existing CONFIG timer, including when the config is unchanged.
pub fn poll(enabled: bool) -> Status {
    if !enabled { registration_failed(false); }
    let handles = devices();
    let (gestures, conflict) = gesture_settings();
    KNOWN.with(|known| {
        let mut known = known.borrow_mut();
        let status = if handles.is_empty() {
            Status::Absent
        } else if !enabled {
            Status::Off
        } else if conflict.is_some() {
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
                note("raw input registration failed, so the trigger is unavailable");
            }
            known.1 = status;
            return status;
        }
        if known.0 != handles || known.1 != status {
            let readers = if status == Status::Active {
                handles
                    .iter()
                    .filter_map(|handle| match Device::open(*handle) {
                        Ok(device) => Some(device),
                        Err(why) => {
                            note(&format!("device {handle:x} ({}) cannot be read: {why}", name(*handle)));
                            None
                        }
                    })
                    .collect::<Vec<_>>()
            } else {
                Vec::new()
            };
            let status = if status == Status::Active && readers.is_empty() {
                Status::Unavailable
            } else {
                status
            };
            let why = match status {
                Status::Absent => format!("no precision touchpad collection (HID 0D/05); {}", inventory()),
                Status::Off => "three_finger is false under [touchpad] in config.toml".to_owned(),
                Status::Conflict => format!("Windows {}", conflict.unwrap_or_default()),
                Status::Unavailable => "no touchpad collection could be parsed".to_owned(),
                Status::Active => format!("reading {} of {} touchpad collection(s)", readers.len(), handles.len()),
            };
            note(&format!("{status:?}, enabled {enabled}: {why}; registry {gestures}"));
            if status == Status::Active || known.1 == Status::Active {
                crate::input::update_touchpad(readers);
            }
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
    eprintln!("touchpad inventory: {}", inventory());
    devices()
        .into_iter()
        .filter_map(|handle| match Device::open(handle) {
            Ok(device) => Some(device),
            Err(why) => {
                note(&format!("device {handle:x} ({}) cannot be read: {why}", name(handle)));
                None
            }
        })
        .collect()
}

#[cfg(test)]
pub fn probe_settings() -> String {
    let (read, conflict) = gesture_settings();
    format!("{read} -> {}", conflict.unwrap_or("no conflict"))
}

/// The model part of the device path (`HID#VID_06CB&PID_CE26&Col02`), and its product string.
fn name(handle: isize) -> String {
    let mut path = [0u16; 512];
    let mut len = path.len() as u32;
    let got = unsafe { GetRawInputDeviceInfoW(HANDLE(handle), RIDI_DEVICENAME, Some(path.as_mut_ptr().cast()), &mut len) };
    if got == u32::MAX || got == 0 {
        return "unnamed".to_owned();
    }
    let path = &path[..(got as usize).min(path.len())];
    let text = String::from_utf16_lossy(path);
    let model: String = text.trim_end_matches('\0').splitn(3, '#').skip(1).take(1).collect();
    let mut zero_ended = path.to_vec();
    if zero_ended.last() != Some(&0) {
        zero_ended.push(0);
    }
    let product = unsafe {
        CreateFileW(
            PCWSTR(zero_ended.as_ptr()),
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_FLAGS_AND_ATTRIBUTES(0),
            HANDLE(0),
        )
    }
    .ok()
    .and_then(|file| {
        let mut buffer = [0u16; 127];
        let ok = unsafe { HidD_GetProductString(file, buffer.as_mut_ptr().cast(), std::mem::size_of_val(&buffer) as u32) };
        unsafe { let _ = CloseHandle(file); }
        let end = buffer.iter().position(|c| *c == 0).unwrap_or(buffer.len());
        (ok.0 != 0 && end > 0).then(|| String::from_utf16_lossy(&buffer[..end]))
    });
    match product {
        Some(product) => format!("{model} \"{product}\""),
        None => model,
    }
}

/// Every HID collection's usage, so a log shows what a touchpad that is not precision reports as.
fn inventory() -> String {
    let (mut mice, mut hid) = (0, Vec::new());
    unsafe {
        let mut count = 0;
        let size = std::mem::size_of::<RAWINPUTDEVICELIST>() as u32;
        if GetRawInputDeviceList(None, &mut count, size) == u32::MAX || count > 1024 {
            return "device list unavailable".to_owned();
        }
        let mut list = vec![RAWINPUTDEVICELIST::default(); count as usize];
        let read = GetRawInputDeviceList(Some(list.as_mut_ptr()), &mut count, size);
        for device in list.iter().take(if read == u32::MAX { 0 } else { read as usize }) {
            if device.dwType == RIM_TYPEMOUSE {
                mice += 1;
            } else if device.dwType == RIM_TYPEHID {
                if let Some((page, usage)) = usage(device.hDevice) {
                    if page == 0x0D || (page == 1 && usage == 2) {
                        hid.push(format!("{page:02X}/{usage:02X} {}", name(device.hDevice.0)));
                    }
                }
            }
        }
    }
    format!("{mice} mouse device(s); digitizer HID collections [{}]", hid.join(", "))
}

fn usage(device: HANDLE) -> Option<(u16, u16)> {
    let mut info = RID_DEVICE_INFO {
        cbSize: std::mem::size_of::<RID_DEVICE_INFO>() as u32,
        ..Default::default()
    };
    let mut bytes = info.cbSize;
    if unsafe {
        GetRawInputDeviceInfoW(device, RIDI_DEVICEINFO, Some((&mut info as *mut RID_DEVICE_INFO).cast()), &mut bytes)
    } == u32::MAX
    {
        return None;
    }
    let hid = unsafe { info.Anonymous.hid };
    Some((hid.usUsagePage, hid.usUsage))
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
            if usage(device.hDevice) == Some((0x0D, 0x05)) {
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
    let result = unsafe {
        crate::input::RegisterRawInputDevices(&[device], std::mem::size_of::<RAWINPUTDEVICE>() as u32)
    };
    match (&result, on) {
        (Ok(()), true) => note(&format!("raw input registered for 0D/05 with INPUTSINK to window {:x}", window.0)),
        (Err(e), _) => note(&format!("raw input {} failed: {e}", if on { "registration" } else { "removal" })),
        _ => {}
    }
    result.is_ok()
}

#[derive(Clone, Copy, Debug)]
struct Axis {
    link: u16,
    min: i32,
    max: i32,
    signed: bool,
    bits: u16,
    scale: f64,
}

/// One finger collection: its id, and where its tip, confidence and position are.
struct Slot {
    link: u16,
    report: u8,
    buttons: u16,
    confidence: bool,
    x: Axis,
    y: Axis,
}

#[derive(Default)]
struct Diagnosis {
    reports: u32,
    frames: u32,
    failure_logged: bool,
    stall_logged: bool,
}

pub struct Device {
    pub handle: isize,
    data: Vec<u64>, // aligned owned preparsed data, not a borrowed Windows pointer
    slots: Vec<Slot>,
    frame: Frame,
    seen: Diagnosis,
    #[cfg(test)]
    samples: usize,
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

    fn has_button(&self, link: u16, usage: u16) -> bool {
        let mut caps = [HIDP_BUTTON_CAPS::default(); 16];
        let mut count = caps.len() as u16;
        let result = unsafe {
            HidP_GetSpecificButtonCaps(HidP_Input, 0x0D, link, usage, caps.as_mut_ptr(), &mut count, self.prep())
        };
        result == HIDP_STATUS_SUCCESS && count > 0 && caps[..count as usize].iter().any(|c| c.LinkCollection == link)
    }

    /// Link collections nested in `link`; some descriptors put X and Y, or the buttons, a level down.
    fn children(nodes: &[HIDP_LINK_COLLECTION_NODE], link: u16) -> Vec<u16> {
        nodes
            .iter()
            .enumerate()
            .filter(|(index, node)| *index != 0 && node.Parent == link && *index as u16 != link)
            .map(|(index, _)| index as u16)
            .collect()
    }

    fn axis(&self, link: u16, usage: u16, fallback_cm: f64) -> Option<Axis> {
        let cap = self.cap(1, link, usage).filter(|cap| cap.LinkCollection == link)?;
        let (min, max) = logical_range(cap.LogicalMin, cap.LogicalMax, cap.BitSize);
        Some(Axis {
            link,
            min,
            max,
            signed: min < 0,
            bits: cap.BitSize,
            // shortcut: unknown physical units assume a 10 x 6 cm pad, upgrade with a device calibration if needed.
            scale: cm_per_unit((min, max), (cap.PhysicalMin, cap.PhysicalMax), cap.Units, cap.UnitsExp, fallback_cm),
        })
    }

    fn open(handle: isize) -> Result<Self, String> {
        unsafe {
            let mut bytes = 0;
            if GetRawInputDeviceInfoW(HANDLE(handle), RIDI_PREPARSEDDATA, None, &mut bytes)
                == u32::MAX
                || bytes == 0
                || bytes > 65536
            {
                return Err(format!("no preparsed data ({bytes} bytes)"));
            }
            let mut data = vec![0u64; (bytes as usize + 7) / 8];
            if GetRawInputDeviceInfoW(
                HANDLE(handle),
                RIDI_PREPARSEDDATA,
                Some(data.as_mut_ptr().cast()),
                &mut bytes,
            ) == u32::MAX
            {
                return Err("preparsed data could not be read".to_owned());
            }
            let mut device = Self {
                handle,
                data,
                slots: Vec::new(),
                frame: Frame::default(),
                seen: Diagnosis::default(),
                #[cfg(test)]
                samples: 0,
            };
            let mut caps = HIDP_CAPS::default();
            if HidP_GetCaps(device.prep(), &mut caps) != HIDP_STATUS_SUCCESS
                || caps.NumberInputValueCaps > 1024
            {
                return Err("HID capabilities could not be read".to_owned());
            }
            let mut values = vec![HIDP_VALUE_CAPS::default(); caps.NumberInputValueCaps as usize];
            let mut count = caps.NumberInputValueCaps;
            if HidP_GetValueCaps(HidP_Input, values.as_mut_ptr(), &mut count, device.prep())
                != HIDP_STATUS_SUCCESS
            {
                return Err("HID value capabilities could not be read".to_owned());
            }
            let mut nodes = vec![HIDP_LINK_COLLECTION_NODE::default(); caps.NumberLinkCollectionNodes.max(1) as usize];
            let mut node_count = nodes.len() as u32;
            if HidP_GetLinkCollectionNodes(nodes.as_mut_ptr(), &mut node_count, device.prep()) != HIDP_STATUS_SUCCESS {
                node_count = 0;
            }
            nodes.truncate(node_count as usize);
            let mut skipped = Vec::new();
            for cap in values.iter().take(count as usize) {
                let id = if cap.IsRange.0 != 0 {
                    cap.Anonymous.Range.UsageMin <= 0x51 && cap.Anonymous.Range.UsageMax >= 0x51
                } else {
                    cap.Anonymous.NotRange.Usage == 0x51
                };
                if cap.UsagePage != 0x0D || !id || device.slots.iter().any(|s| s.link == cap.LinkCollection) {
                    continue;
                }
                let link = cap.LinkCollection;
                let near = std::iter::once(link).chain(Self::children(&nodes, link)).collect::<Vec<_>>();
                let x = near.iter().find_map(|l| device.axis(*l, 0x30, 10.0));
                let y = near.iter().find_map(|l| device.axis(*l, 0x31, 6.0));
                let buttons = near.iter().copied().find(|l| device.has_button(*l, 0x42));
                let (Some(x), Some(y), Some(buttons)) = (x, y, buttons) else {
                    skipped.push(format!(
                        "link {link} lacks{}{}{}",
                        if x.is_none() { " X" } else { "" },
                        if y.is_none() { " Y" } else { "" },
                        if buttons.is_none() { " tip switch" } else { "" }
                    ));
                    continue;
                };
                device.slots.push(Slot {
                    link,
                    report: cap.ReportID,
                    buttons,
                    confidence: device.has_button(buttons, 0x47),
                    x,
                    y,
                });
            }
            let contact_count = device.cap(0x0D, 0, 0x54);
            let scan = device.cap(0x0D, 0, 0x56);
            let layout = format!(
                "{} input report bytes, {} link collections, contact count {}, scan time {}, {} contact slot(s): [{}]{}",
                caps.InputReportByteLength,
                node_count,
                contact_count.map_or("missing".to_owned(), |c| format!("link {} report {}", c.LinkCollection, c.ReportID)),
                scan.map_or("missing".to_owned(), |c| format!("link {} report {}", c.LinkCollection, c.ReportID)),
                device.slots.len(),
                device
                    .slots
                    .iter()
                    .map(|s| format!(
                        "link {} report {} buttons@{} confidence {} X@{} {}..{} Y@{} {}..{}",
                        s.link, s.report, s.buttons, s.confidence, s.x.link, s.x.min, s.x.max, s.y.link, s.y.min, s.y.max
                    ))
                    .collect::<Vec<_>>()
                    .join("; "),
                if skipped.is_empty() { String::new() } else { format!(", skipped: {}", skipped.join("; ")) },
            );
            if device.slots.is_empty() || device.slots.len() > 10 || contact_count.is_none() || scan.is_none() {
                return Err(layout);
            }
            note(&format!("device {handle:x} ({}): {layout}", name(handle)));
            Ok(device)
        }
    }

    fn value(&self, page: u16, link: u16, usage: u16, report: &[u8]) -> Result<u32, String> {
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
        if status != HIDP_STATUS_SUCCESS {
            return Err(format!("page {page:x} link {link} usage {usage:x}: status {:#x}", status.0));
        }
        Ok(value)
    }

    fn position(&self, axis: &Axis, usage: u16, report: &[u8]) -> Result<f64, String> {
        let value = self.value(1, axis.link, usage, report)?;
        let logical = if axis.signed && (1..32).contains(&axis.bits) {
            ((value << (32 - axis.bits)) as i32) >> (32 - axis.bits)
        } else {
            value as i32
        };
        // Some descriptors understate their range; clamp rather than lose the whole frame.
        Ok(logical.clamp(axis.min, axis.max) as f64 * axis.scale)
    }

    fn report(&mut self, report: &[u8]) -> Option<Vec<Contact>> {
        #[cfg(test)]
        {
            self.samples += 1;
            if self.samples <= 12 {
                eprintln!("touchpad raw report: {report:02x?}");
            }
        }
        self.seen.reports = self.seen.reports.saturating_add(1);
        let parsed = self.parse(report);
        if self.seen.reports == 1 {
            note(&format!(
                "first report from device {:x}: {} bytes, id {:#04x}, {}",
                self.handle,
                report.len(),
                report.first().copied().unwrap_or(0),
                match &parsed {
                    Ok((scan, count, records)) => format!("scan {scan} count {count} records {records:?}"),
                    Err(why) => format!("unreadable: {why}"),
                }
            ));
        }
        let (scan, count, records) = match parsed {
            Ok(parsed) => parsed,
            Err(why) => {
                if !self.seen.failure_logged && self.seen.reports > 1 {
                    self.seen.failure_logged = true;
                    note(&format!("report {} from device {:x} unreadable: {why}; bytes {report:02x?}", self.seen.reports, self.handle));
                }
                return None;
            }
        };
        debug_log!("touchpad report: device {} scan {scan} count {count} records {records:?} bytes {report:02x?}", self.handle);
        #[cfg(test)]
        if self.samples <= 12 {
            eprintln!("touchpad report: scan {scan} count {count} records {records:?}");
        }
        let frame = self.frame.push(scan, count, &records);
        if frame.is_some() {
            self.seen.frames = self.seen.frames.saturating_add(1);
        } else if self.seen.frames == 0 && self.seen.reports >= 300 && !self.seen.stall_logged {
            self.seen.stall_logged = true;
            note(&format!(
                "{} reports from device {:x} but no complete frame; last scan {scan} count {count} records {records:?}",
                self.seen.reports, self.handle
            ));
        }
        frame
    }

    fn parse(&self, report: &[u8]) -> Result<(u32, usize, Vec<(u32, Option<Contact>)>), String> {
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
                    slot.buttons,
                    usages.as_mut_ptr(),
                    &mut length,
                    self.prep(),
                    &mut copy,
                )
            };
            if status != HIDP_STATUS_SUCCESS || length as usize > usages.len() {
                return Err(format!("buttons of link {}: status {:#x} length {length}", slot.buttons, status.0));
            }
            let usages = &usages[..length as usize];
            let contact = if usages.contains(&0x42) {
                Some(Contact {
                    id,
                    // Without a confidence usage the device has already rejected palms itself.
                    confident: !slot.confidence || usages.contains(&0x47),
                    x: self.position(&slot.x, 0x30, report)?,
                    y: self.position(&slot.y, 0x31, report)?,
                })
            } else {
                None
            };
            records.push((id, contact));
        }
        Ok((scan, count, records))
    }
}

pub struct Reader {
    pub devices: Vec<Device>,
    owner: Option<isize>,
    gesture: Gesture,
    /// Contact counts already written to the log, one bit each, and whether a hold began.
    logged: u32,
    #[cfg(test)]
    last_count: usize,
}

impl Default for Reader {
    fn default() -> Self {
        Self {
            devices: Vec::new(),
            owner: None,
            gesture: Gesture::default(),
            logged: 0,
            #[cfg(test)]
            last_count: 0,
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
    pub fn report(&mut self, device: isize, report: &[u8], ms: u32) -> Option<Event> {
        let Some(index) = self.devices.iter().position(|d| d.handle == device) else {
            if self.logged & 1 << 31 == 0 {
                self.logged |= 1 << 31;
                note(&format!("report from unknown device {device:x}; known [{}]",
                    self.devices.iter().map(|d| format!("{:x}", d.handle)).collect::<Vec<_>>().join(", ")));
            }
            return None;
        };
        let contacts = self.devices[index].report(report)?;
        if self.owner.is_some_and(|owner| owner != device) {
            return None;
        }
        if !contacts.is_empty() {
            self.owner = Some(device);
        }
        let event = self.gesture.update(&contacts, ms);
        // Each finger count once, so a log shows how many fingers this pad reports at most.
        let bit = 1 << contacts.len().min(10);
        if self.logged & bit == 0 && !contacts.is_empty() {
            self.logged |= bit;
            let unsure = contacts.iter().filter(|c| !c.confident).count();
            note(&format!("first frame with {} contact(s), {unsure} without confidence", contacts.len()));
        }
        if event.began && self.logged & 1 << 30 == 0 {
            self.logged |= 1 << 30;
            note("three-finger hold began");
        }
        #[cfg(test)]
        {
            if self.last_count != contacts.len() || event.began || event.ended.is_some() {
                eprintln!(
                    "touchpad frame at {ms}: {} contacts {event:?}",
                    contacts.len()
                );
            }
            self.last_count = contacts.len();
        }
        if contacts.is_empty() {
            self.owner = None;
        }
        Some(event)
    }
}

#[cfg(test)]
mod tests {
    /// Needs no hand on the pad: lists collections and Windows' gesture settings on this PC.
    #[test]
    #[ignore = "reads this PC's touchpad collections and registry"]
    fn touchpad_inventory() {
        eprintln!("touchpad registry: {}", super::probe_settings());
        eprintln!("touchpad readable collections: {}", super::probe_devices().len());
    }
}
