include!("../../native/zi2c_build.rs");

fn main() {
    compile_ziran("native/apply_patch.zi", "xcoder_apply_patch", &[]);
}
