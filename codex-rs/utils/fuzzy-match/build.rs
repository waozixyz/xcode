include!("../../../native/zi2c_build.rs");

fn main() {
    compile_ziran("native/fuzzy.zi", "xcode_fuzzy", &[]);
}
