include!("../../native/zi2c_build.rs");

fn main() {
    compile_ziran("native/git.zi", "xcode_git", &["git"]);
}
