// Shared by every build script that links an Xcoder Ziran module; each one
// includes this file with `include!`. Zi2c writes C under OUT_DIR/native.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The Xcoder project root: the nearest ancestor that holds `ziran.toml`.
fn ziran_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .find(|directory| directory.join("ziran.toml").is_file())
        .expect("Xcoder ziran.toml above this crate")
        .to_path_buf()
}

/// A Ziran command that uses the committed lock unless a local override exists.
fn ziran<I, S>(root: &Path, arguments: I) -> Command
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut command = Command::new(std::env::var_os("ZIRAN").unwrap_or_else(|| "ziran".into()));
    command.current_dir(root).args(arguments);
    if !root.join("ziran.local.toml").exists() {
        command.arg("--locked");
    }
    command
}

/// Generate MODULE with Zi2c and compile every generated C file into LIBRARY.
/// The sources of each named PACKAGE also rerun the build. Returns the root.
fn compile_ziran(module: &str, library: &str, packages: &[&str]) -> PathBuf {
    let root = ziran_root();
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR")).join("native");
    println!("cargo:rerun-if-env-changed=ZIRAN");
    for file in [module, "native/zi2c_build.rs", "ziran.toml", "ziran.lock", "ziran.local.toml"] {
        println!("cargo:rerun-if-changed={}", root.join(file).display());
    }
    for package in packages {
        let resolved = ziran(&root, ["pkg", "path", package])
            .output()
            .expect("resolve Ziran package");
        assert!(resolved.status.success(), "Ziran package {package} resolution failed");
        let path = String::from_utf8(resolved.stdout).expect("UTF-8 package path");
        println!("cargo:rerun-if-changed={}", Path::new(path.trim()).join("src").display());
    }
    // Modules from an earlier import graph must not be linked.
    if out.exists() {
        std::fs::remove_dir_all(&out).expect("clear generated modules");
    }
    let generate = [OsStr::new("build"), OsStr::new("--project"), OsStr::new("--target=c"),
                    OsStr::new("-o"), out.as_os_str(), OsStr::new(module)];
    assert!(
        ziran(&root, generate).status().expect("run Zi2c").success(),
        "Zi2c generation of {module} failed"
    );
    let mut sources = std::fs::read_dir(&out)
        .expect("generated modules")
        .map(|entry| entry.expect("generated module").path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "c"))
        .collect::<Vec<_>>();
    sources.sort();
    cc::Build::new()
        .files(sources)
        .include(&out)
        .flag_if_supported("-std=c99")
        .opt_level(3)
        .compile(library);
    root
}
