include!("../../../native/zi2c_build.rs");

fn main() {
    compile_ziran("native/diff.zi", "xcoder_diff", &["diff"]);
}
