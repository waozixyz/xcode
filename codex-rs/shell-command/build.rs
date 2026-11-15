include!("../../native/zi2c_build.rs");

fn main() {
    compile_ziran("native/shell.zi", "xcoder_shell", &[]);
}
