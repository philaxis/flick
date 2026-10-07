//! Read-only check that the virtual desktop COM interfaces work on this Windows build.
#[cfg(windows)]
fn main() {
    match winvd::get_desktops() {
        Ok(desktops) => {
            let current = winvd::get_current_desktop().and_then(|d| d.get_id());
            println!("desktops: {}", desktops.len());
            for d in desktops {
                let id = d.get_id();
                let mark = if id.is_ok() && id == current { "*" } else { " " };
                println!("{mark} {:?} {:?} {:?}", d.get_index(), id, d.get_name());
            }
        }
        Err(e) => println!("error: {e:?}"),
    }
}

#[cfg(not(windows))]
fn main() {}
