//! The only way to Windows' virtual desktops. The shell's interfaces for them
//! are undocumented and their layout differs between Windows versions, so
//! two `winvd` releases are linked in, one per layout; which one is used is
//! decided once from the Windows build, and until then every call fails
//! without touching COM.

/// The `winvd` release whose interface layout matches a range of Windows builds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backend {
    /// winvd 0.0.46: Windows 11 22H2 and 23H2 from the early 2024 updates on.
    Win23H2,
    /// winvd 0.0.49: Windows 11 24H2 and later.
    Win24H2,
}

impl Backend {
    /// The backend written against this Windows build, if there is one.
    pub fn for_build(build: u32, revision: u32) -> Option<Backend> {
        match build {
            22621 if revision >= 3155 => Some(Backend::Win23H2),
            22631 if revision >= 3085 => Some(Backend::Win23H2),
            26100 if revision >= 2605 => Some(Backend::Win24H2),
            26101.. => Some(Backend::Win24H2),
            _ => None,
        }
    }
}

#[cfg(windows)]
pub use shell::*;

#[cfg(windows)]
mod shell {
    use super::Backend;
    use std::{any::Any, sync::mpsc, sync::OnceLock};
    use windows::{
        core::{w, GUID},
        Win32::{
            Foundation::HWND,
            System::Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RRF_RT_REG_SZ},
        },
    };
    use winvd_23h2 as v23;
    use winvd_24h2 as v24;

    static BACKEND: OnceLock<Backend> = OnceLock::new();

    /// Windows' build and update revision, e.g. (22631, 6199).
    pub fn windows_build() -> Option<(u32, u32)> {
        let key = w!("SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion");
        unsafe {
            let mut text = [0u16; 16];
            let mut size = std::mem::size_of_val(&text) as u32;
            RegGetValueW(HKEY_LOCAL_MACHINE, key, w!("CurrentBuild"), RRF_RT_REG_SZ, None, Some(text.as_mut_ptr().cast()), Some(&mut size)).ok()?;
            let len = text.iter().position(|c| *c == 0).unwrap_or(text.len());
            let build: u32 = String::from_utf16_lossy(&text[..len]).trim().parse().ok()?;
            let mut revision = 0u32;
            let mut size = 4u32;
            let _ = RegGetValueW(HKEY_LOCAL_MACHINE, key, w!("UBR"), RRF_RT_REG_DWORD, None, Some(&mut revision as *mut u32 as *mut _), Some(&mut size));
            Some((build, revision))
        }
    }

    /// Settles which backend this process uses, for good. `None` means this
    /// Windows has none: calling the wrong layout can crash the shell, so the
    /// app must not start at all. `force` is for trying a build outside the
    /// known ranges and takes the layout of the nearest one.
    pub fn select(force: bool) -> Option<Backend> {
        let build = windows_build();
        let backend = build.and_then(|(build, revision)| Backend::for_build(build, revision)).or_else(|| {
            let newer = build.is_some_and(|(build, _)| build >= 26100);
            force.then_some(if newer { Backend::Win24H2 } else { Backend::Win23H2 })
        })?;
        Some(*BACKEND.get_or_init(|| backend))
    }

    fn backend() -> Result<Backend> {
        BACKEND.get().copied().ok_or(Error::NoBackend)
    }

    pub enum Error {
        /// `select` has not found a backend for this Windows.
        NoBackend,
        Win23H2(v23::Error),
        Win24H2(v24::Error),
    }

    pub type Result<T> = std::result::Result<T, Error>;

    // Shown as the underlying error, which is what the log has always held.
    impl std::fmt::Debug for Error {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                Error::NoBackend => f.write_str("NoBackend"),
                Error::Win23H2(e) => e.fmt(f),
                Error::Win24H2(e) => e.fmt(f),
            }
        }
    }

    impl From<v23::Error> for Error {
        fn from(e: v23::Error) -> Self {
            Error::Win23H2(e)
        }
    }

    impl From<v24::Error> for Error {
        fn from(e: v24::Error) -> Self {
            Error::Win24H2(e)
        }
    }

    /// A desktop, known by its GUID alone: a handle that also carries the
    /// index goes stale as soon as a desktop is moved or removed.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct Desktop(u128);

    impl Desktop {
        /// The GUID as text, which is how the grid stores desktops.
        pub fn id(self) -> String {
            format!("{:?}", GUID::from_u128(self.0))
        }

        fn v23(self) -> v23::Desktop {
            GUID::from_u128(self.0).into()
        }

        fn v24(self) -> v24::Desktop {
            v24::GUID::from_u128(self.0).into()
        }
    }

    fn from_v23(desktop: v23::Desktop) -> Result<Desktop> {
        Ok(Desktop(desktop.get_id()?.to_u128()))
    }

    fn from_v24(desktop: v24::Desktop) -> Result<Desktop> {
        Ok(Desktop(desktop.get_id()?.to_u128()))
    }

    /// The newer `windows` crate holds a window handle as a pointer.
    fn hwnd_v24(hwnd: HWND) -> v24::HWND {
        v24::HWND(hwnd.0 as *mut std::ffi::c_void)
    }

    /// Every desktop, in Windows' own order.
    pub fn get_desktops() -> Result<Vec<Desktop>> {
        match backend()? {
            Backend::Win23H2 => v23::get_desktops()?.into_iter().map(from_v23).collect(),
            Backend::Win24H2 => v24::get_desktops()?.into_iter().map(from_v24).collect(),
        }
    }

    pub fn get_current_desktop() -> Result<Desktop> {
        match backend()? {
            Backend::Win23H2 => from_v23(v23::get_current_desktop()?),
            Backend::Win24H2 => from_v24(v24::get_current_desktop()?),
        }
    }

    pub fn get_desktop_by_window(hwnd: HWND) -> Result<Desktop> {
        match backend()? {
            Backend::Win23H2 => from_v23(v23::get_desktop_by_window(hwnd)?),
            Backend::Win24H2 => from_v24(v24::get_desktop_by_window(hwnd_v24(hwnd))?),
        }
    }

    pub fn create_desktop() -> Result<Desktop> {
        match backend()? {
            Backend::Win23H2 => from_v23(v23::create_desktop()?),
            Backend::Win24H2 => from_v24(v24::create_desktop()?),
        }
    }

    pub fn switch_desktop(desktop: Desktop) -> Result<()> {
        match backend()? {
            Backend::Win23H2 => Ok(v23::switch_desktop(desktop.v23())?),
            Backend::Win24H2 => Ok(v24::switch_desktop(desktop.v24())?),
        }
    }

    /// Removes a desktop; its windows go to `fallback`.
    pub fn remove_desktop(desktop: Desktop, fallback: Desktop) -> Result<()> {
        match backend()? {
            Backend::Win23H2 => Ok(v23::remove_desktop(desktop.v23(), fallback.v23())?),
            Backend::Win24H2 => Ok(v24::remove_desktop(desktop.v24(), fallback.v24())?),
        }
    }

    /// Moves a desktop to a new position in Windows' desktop order.
    pub fn move_desktop(desktop: Desktop, index: u32) -> Result<()> {
        match backend()? {
            Backend::Win23H2 => Ok(v23::move_desktop(desktop.v23(), index)?),
            Backend::Win24H2 => Ok(v24::move_desktop(desktop.v24(), index)?),
        }
    }

    pub fn move_window_to_desktop(desktop: Desktop, hwnd: HWND) -> Result<()> {
        match backend()? {
            Backend::Win23H2 => Ok(v23::move_window_to_desktop(desktop.v23(), &hwnd)?),
            Backend::Win24H2 => Ok(v24::move_window_to_desktop(desktop.v24(), &hwnd_v24(hwnd))?),
        }
    }


    /// The calls that take a window and nothing else.
    macro_rules! window_calls {
        ($($name:ident -> $value:ty;)*) => {$(
            pub fn $name(hwnd: HWND) -> Result<$value> {
                match backend()? {
                    Backend::Win23H2 => Ok(v23::$name(hwnd)?),
                    Backend::Win24H2 => Ok(v24::$name(hwnd_v24(hwnd))?),
                }
            }
        )*};
    }

    window_calls! {
        is_window_on_current_desktop -> bool;
        is_pinned_window -> bool;
        pin_window -> ();
        unpin_window -> ();
        is_pinned_app -> bool;
        pin_app -> ();
        unpin_app -> ();
    }

    /// What the listener reports. Only a change of the current desktop is of
    /// any use to the app.
    #[derive(Clone)]
    pub enum DesktopEvent {
        DesktopChanged { new: Desktop, old: Desktop },
        Other,
    }

    impl From<v23::DesktopEvent> for DesktopEvent {
        fn from(event: v23::DesktopEvent) -> Self {
            match event {
                v23::DesktopEvent::DesktopChanged { new, old } => match (from_v23(new), from_v23(old)) {
                    (Ok(new), Ok(old)) => DesktopEvent::DesktopChanged { new, old },
                    _ => DesktopEvent::Other,
                },
                _ => DesktopEvent::Other,
            }
        }
    }

    impl From<v24::DesktopEvent> for DesktopEvent {
        fn from(event: v24::DesktopEvent) -> Self {
            match event {
                v24::DesktopEvent::DesktopChanged { new, old } => match (from_v24(new), from_v24(old)) {
                    (Ok(new), Ok(old)) => DesktopEvent::DesktopChanged { new, old },
                    _ => DesktopEvent::Other,
                },
                _ => DesktopEvent::Other,
            }
        }
    }

    /// Keeps the listener alive; dropping it stops the listener.
    pub struct DesktopEventThread {
        _thread: Box<dyn Any>,
    }

    pub fn listen_desktop_events(sender: mpsc::Sender<DesktopEvent>) -> Result<DesktopEventThread> {
        let thread: Box<dyn Any> = match backend()? {
            Backend::Win23H2 => Box::new(v23::listen_desktop_events(sender)?),
            Backend::Win24H2 => Box::new(v24::listen_desktop_events(sender)?),
        };
        Ok(DesktopEventThread { _thread: thread })
    }
}

#[cfg(test)]
mod tests {
    use super::Backend::{self, Win23H2, Win24H2};

    /// The two interface layouts are told apart by build number alone, and
    /// the one for the wrong Windows can crash the shell.
    #[test]
    fn each_windows_build_gets_the_backend_written_for_it() {
        let cases = [
            ((22000, 9999), None),
            ((22621, 3154), None),
            ((22621, 3155), Some(Win23H2)),
            ((22631, 3084), None),
            ((22631, 6199), Some(Win23H2)),
            ((25398, 9999), None),
            ((26100, 2604), None),
            ((26100, 2605), Some(Win24H2)),
            ((26200, 0), Some(Win24H2)),
        ];
        for ((build, revision), backend) in cases {
            assert_eq!(Backend::for_build(build, revision), backend, "{build}.{revision}");
        }
    }
}
