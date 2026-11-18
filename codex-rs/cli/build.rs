include!("../../native/zi2c_build.rs");

fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS");
    if target_os.as_deref() == Ok("macos") {
        println!("cargo:rustc-link-arg=-ObjC");
    }
    if target_os.as_deref() == Ok("linux")
        && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("gnu")
    {
        // GNU ld otherwise retains input symbol tables until the final link completes,
        // which can trigger systemd-oomd on constrained builders even when rustc fits.
        println!("cargo:rustc-link-arg=-Wl,--no-keep-memory");
        // The idle-memory watcher relies on the GNU allocator's trim call.
        compile_ziran("native/idle.zi", "xcode_idle", &[]);
    }
}
