include!("../../../native/zi2c_build.rs");

fn main() {
    compile_ziran("native/diff.zi", "xcode_diff", &["diff"]);
}
