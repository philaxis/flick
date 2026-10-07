//! Read-only check that the virtual desktop COM interfaces work on this Windows build.
// The app is a binary only; its door to the virtual desktops is compiled in
// here as it is, and this check uses a small part of it.
#[allow(dead_code)]
#[path = "../src/vdapi.rs"]
mod vdapi;

#[cfg(windows)]
fn main() {
    println!("windows build: {:?}", vdapi::windows_build());
    let Some(backend) = vdapi::select(false) else { return println!("no backend for this Windows") };
    println!("backend: {backend:?}");
    match vdapi::get_desktops() {
        Ok(desktops) => {
            let current = vdapi::get_current_desktop();
            println!("desktops: {}", desktops.len());
            for d in desktops {
                let mark = if current.as_ref().is_ok_and(|c| *c == d) { "*" } else { " " };
                println!("{mark} {}", d.id());
            }
            if let Err(e) = current {
                println!("current desktop: error: {e:?}");
            }
        }
        Err(e) => println!("error: {e:?}"),
    }
}

#[cfg(not(windows))]
fn main() {}
