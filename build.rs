// Links the prebuilt icon/version resource (assets/app.res, produced by
// scripts/make-icon.py) into the Windows exe.
fn main() {
    println!("cargo:rerun-if-changed=assets/app.res");
    if std::env::var_os("CARGO_CFG_WINDOWS").is_some() {
        let dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
        println!("cargo:rustc-link-arg-bins={dir}/assets/app.res");
    }
}
