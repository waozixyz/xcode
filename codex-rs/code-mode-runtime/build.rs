include!("../../native/zi2c_build.rs");

fn main() {
    let root = compile_ziran("native/dispatch.zi", "xcoder_dispatch", &[]);
    // Custom cells compile against the runtime module with this toolchain.
    println!("cargo:rerun-if-changed={}", root.join("native/runtime.zi").display());
    let toolchain = ziran(&root, ["pkg", "path", "ziran"])
        .output()
        .expect("resolve Ziran toolchain");
    assert!(toolchain.status.success(), "resolve locked Ziran toolchain");
    println!(
        "cargo:rustc-env=XCODER_ZIRAN_TOOLCHAIN_DEFAULT={}",
        String::from_utf8(toolchain.stdout)
            .expect("toolchain path UTF-8")
            .trim()
    );
}
