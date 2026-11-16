include!("../../native/zi2c_build.rs");

fn main() {
    compile_ziran("native/git.zi", "xcoder_git", &["git"]);
}
