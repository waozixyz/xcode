include!("../../native/zi2c_build.rs");

fn main() {
    compile_ziran("native/execpolicy.zi", "xcode_execpolicy", &[]);
}
