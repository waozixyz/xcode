//! Build and execute cells with unprivileged kernel isolation that remains
//! available inside an existing user/mount namespace. Landlock restricts path,
//! network, signal and Unix-socket access; seccomp and rlimits bound native code.
use std::ffi::CString;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::process::{Child, Command};

const FS_EXECUTE: u64 = 1 << 0;
const FS_WRITE_FILE: u64 = 1 << 1;
const FS_READ_FILE: u64 = 1 << 2;
const FS_READ_DIR: u64 = 1 << 3;
const FS_REMOVE_DIR: u64 = 1 << 4;
const FS_REMOVE_FILE: u64 = 1 << 5;
const FS_MAKE_DIR: u64 = 1 << 7;
const FS_MAKE_REG: u64 = 1 << 8;
const FS_MAKE_SYM: u64 = 1 << 12;
const FS_REFER: u64 = 1 << 13;
const FS_TRUNCATE: u64 = 1 << 14;
const FS_IOCTL_DEV: u64 = 1 << 15;
const FS_RESOLVE_UNIX: u64 = 1 << 16;
const FS_READ: u64 = FS_READ_FILE | FS_READ_DIR;
const FS_WRITE: u64 = FS_WRITE_FILE
    | FS_REMOVE_DIR
    | FS_REMOVE_FILE
    | FS_MAKE_DIR
    | FS_MAKE_REG
    | FS_MAKE_SYM
    | FS_REFER
    | FS_TRUNCATE;
const NET_TCP: u64 = (1 << 0) | (1 << 1);
const NET_UDP: u64 = (1 << 2) | (1 << 3);
const SCOPE_ABSTRACT_UNIX_SOCKET: u64 = 1 << 0;
const SCOPE_SIGNAL: u64 = 1 << 1;

#[repr(C)]
struct LandlockRulesetAttr {
    handled_access_fs: u64,
    handled_access_net: u64,
    scoped: u64,
}

#[repr(C, packed)]
struct LandlockPathBeneathAttr {
    allowed_access: u64,
    parent_fd: i32,
}

#[derive(Clone, Copy)]
enum SandboxKind {
    Compiler,
    Cell,
}

#[derive(Default)]
struct SandboxPolicy {
    read_exec: Vec<PathBuf>,
    read_only: Vec<PathBuf>,
    read_write: Vec<PathBuf>,
}

pub(crate) fn toolchain() -> Result<PathBuf, String> {
    let path = std::env::var_os("XCODE_ZIRAN_TOOLCHAIN")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("XCODE_ZIRAN_TOOLCHAIN_DEFAULT")));
    let path = path
        .canonicalize()
        .map_err(|error| format!("Ziran toolchain: {error}"))?;
    if !path.join("build/bin/ziran").is_file() {
        return Err("XCODE_ZIRAN_TOOLCHAIN must name a built Ziran toolchain".into());
    }
    Ok(path)
}

fn supported_fs_access(abi: i32) -> u64 {
    let mut access = (1 << 13) - 1;
    if abi >= 2 {
        access |= FS_REFER;
    }
    if abi >= 3 {
        access |= FS_TRUNCATE;
    }
    if abi >= 5 {
        access |= FS_IOCTL_DEV;
    }
    if abi >= 9 {
        access |= FS_RESOLVE_UNIX;
    }
    access
}

fn landlock_abi() -> Result<i32, String> {
    // SAFETY: a null attribute with VERSION queries the supported Landlock ABI.
    let abi = unsafe {
        libc::syscall(
            libc::SYS_landlock_create_ruleset,
            std::ptr::null::<LandlockRulesetAttr>(),
            0usize,
            1u32,
        )
    } as i32;
    if abi < 0 {
        return Err(format!(
            "Landlock is required for native cell isolation: {}",
            std::io::Error::last_os_error()
        ));
    }
    if abi < 6 {
        return Err(format!(
            "Landlock ABI 6 or newer is required for native cell isolation (found {abi})"
        ));
    }
    Ok(abi)
}

fn open_path(path: &Path) -> Result<OwnedFd, String> {
    let name = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| format!("sandbox path contains NUL: {}", path.display()))?;
    // SAFETY: name is NUL terminated and open returns a new owned descriptor.
    let fd = unsafe { libc::open(name.as_ptr(), libc::O_PATH | libc::O_CLOEXEC) };
    if fd < 0 {
        return Err(format!(
            "open sandbox path {}: {}",
            path.display(),
            std::io::Error::last_os_error()
        ));
    }
    // SAFETY: fd was just returned by open and is uniquely owned here.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

fn add_path_rule(
    ruleset: &OwnedFd,
    path: &Path,
    mut access: u64,
    supported: u64,
) -> Result<(), String> {
    let metadata = std::fs::metadata(path)
        .map_err(|error| format!("sandbox path {}: {error}", path.display()))?;
    access &= supported;
    if !metadata.is_dir() {
        access &= FS_EXECUTE
            | FS_WRITE_FILE
            | FS_READ_FILE
            | FS_TRUNCATE
            | FS_IOCTL_DEV
            | FS_RESOLVE_UNIX;
        if !metadata.file_type().is_file() {
            access &= !FS_TRUNCATE;
        }
    }
    let path_fd = open_path(path)?;
    let attr = LandlockPathBeneathAttr {
        allowed_access: access,
        parent_fd: path_fd.as_raw_fd(),
    };
    // SAFETY: ruleset and path descriptors remain open for the syscall and the
    // packed attribute exactly matches the stable Linux UAPI layout.
    let result = unsafe {
        libc::syscall(
            libc::SYS_landlock_add_rule,
            ruleset.as_raw_fd(),
            1u32,
            &attr as *const LandlockPathBeneathAttr,
            0u32,
        )
    };
    if result != 0 {
        return Err(format!(
            "add Landlock rule for {}: {}",
            path.display(),
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}

fn landlock_ruleset(policy: &SandboxPolicy) -> Result<OwnedFd, String> {
    let abi = landlock_abi()?;
    let fs_access = supported_fs_access(abi);
    let attr = LandlockRulesetAttr {
        handled_access_fs: fs_access,
        handled_access_net: NET_TCP | if abi >= 10 { NET_UDP } else { 0 },
        scoped: SCOPE_ABSTRACT_UNIX_SOCKET | SCOPE_SIGNAL,
    };
    // SAFETY: attr is the first three fields of the extensible Landlock UAPI
    // structure, and the supplied size deliberately excludes newer quiet fields.
    let fd = unsafe {
        libc::syscall(
            libc::SYS_landlock_create_ruleset,
            &attr as *const LandlockRulesetAttr,
            std::mem::size_of::<LandlockRulesetAttr>(),
            0u32,
        )
    } as i32;
    if fd < 0 {
        return Err(format!(
            "create Landlock ruleset: {}",
            std::io::Error::last_os_error()
        ));
    }
    // SAFETY: fd was just returned by landlock_create_ruleset.
    let ruleset = unsafe { OwnedFd::from_raw_fd(fd) };
    for path in &policy.read_exec {
        add_path_rule(&ruleset, path, FS_READ | FS_EXECUTE, fs_access)?;
    }
    for path in &policy.read_only {
        add_path_rule(&ruleset, path, FS_READ, fs_access)?;
    }
    for path in &policy.read_write {
        add_path_rule(&ruleset, path, FS_READ | FS_WRITE, fs_access)?;
    }
    Ok(ruleset)
}

fn instruction(code: u16, jt: u8, jf: u8, k: u32) -> libc::sock_filter {
    libc::sock_filter { code, jt, jf, k }
}

fn deny_syscall(filter: &mut Vec<libc::sock_filter>, syscall: libc::c_long) {
    const JEQ: u16 = 0x15;
    const RET: u16 = 0x06;
    const DENY: u32 = 0x00050000 | libc::EPERM as u32;
    filter.push(instruction(JEQ, 0, 1, syscall as u32));
    filter.push(instruction(RET, 0, 0, DENY));
}

fn seccomp_filter(kind: SandboxKind) -> Vec<libc::sock_filter> {
    #[cfg(target_arch = "x86_64")]
    let arch = 0xc000003e;
    #[cfg(target_arch = "aarch64")]
    let arch = 0xc00000b7;
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    compile_error!("Xcode native cell isolation currently supports Linux x86-64 and AArch64");
    const LD: u16 = 0x20;
    const JEQ: u16 = 0x15;
    const JGE: u16 = 0x35;
    const JSET: u16 = 0x45;
    const RET: u16 = 0x06;
    const ALLOW: u32 = 0x7fff0000;
    const DENY: u32 = 0x00050000 | libc::EPERM as u32;
    const ENOSYS: u32 = 0x00050000 | libc::ENOSYS as u32;
    let mut filter = vec![
        instruction(LD, 0, 0, 4),
        instruction(JEQ, 1, 0, arch),
        instruction(RET, 0, 0, 0x80000000),
        instruction(LD, 0, 0, 0),
        // Reject x32 syscall numbers before the normal syscall list.
        instruction(JGE, 0, 1, 0x40000000),
        instruction(RET, 0, 0, DENY),
    ];
    if matches!(kind, SandboxKind::Cell) {
        filter.extend([
            instruction(JEQ, 0, 1, libc::SYS_clone3 as u32),
            instruction(RET, 0, 0, ENOSYS),
            instruction(JEQ, 0, 4, libc::SYS_clone as u32),
            instruction(LD, 0, 0, 16),
            instruction(JSET, 1, 0, libc::CLONE_THREAD as u32),
            instruction(RET, 0, 0, DENY),
            instruction(RET, 0, 0, ALLOW),
        ]);
    }
    let mut denied = vec![
        libc::SYS_ptrace,
        libc::SYS_process_vm_readv,
        libc::SYS_process_vm_writev,
        libc::SYS_bpf,
        libc::SYS_perf_event_open,
        libc::SYS_mount,
        libc::SYS_umount2,
        libc::SYS_unshare,
        libc::SYS_setns,
        libc::SYS_chroot,
        libc::SYS_pivot_root,
        libc::SYS_keyctl,
        libc::SYS_add_key,
        libc::SYS_request_key,
        libc::SYS_open_by_handle_at,
        libc::SYS_name_to_handle_at,
        libc::SYS_kcmp,
        libc::SYS_userfaultfd,
        libc::SYS_io_uring_setup,
        libc::SYS_io_uring_enter,
        libc::SYS_io_uring_register,
        libc::SYS_pidfd_open,
        libc::SYS_pidfd_getfd,
        libc::SYS_pidfd_send_signal,
        libc::SYS_shmget,
        libc::SYS_shmat,
        libc::SYS_shmctl,
        libc::SYS_semget,
        libc::SYS_semop,
        libc::SYS_semtimedop,
        libc::SYS_semctl,
        libc::SYS_msgget,
        libc::SYS_msgsnd,
        libc::SYS_msgrcv,
        libc::SYS_msgctl,
        libc::SYS_socket,
        libc::SYS_connect,
        libc::SYS_bind,
        libc::SYS_listen,
        libc::SYS_accept,
        libc::SYS_accept4,
        libc::SYS_socketpair,
    ];
    #[cfg(target_arch = "x86_64")]
    if matches!(kind, SandboxKind::Cell) {
        denied.extend([libc::SYS_fork, libc::SYS_vfork]);
    }
    for syscall in denied {
        deny_syscall(&mut filter, syscall);
    }
    filter.push(instruction(RET, 0, 0, ALLOW));
    filter
}

fn apply_sandbox(
    command: &mut Command,
    policy: SandboxPolicy,
    memory: u64,
    kind: SandboxKind,
) -> Result<(), String> {
    let ruleset = landlock_ruleset(&policy)?;
    let mut filter = seccomp_filter(kind);
    // Used to close the PR_SET_PDEATHSIG race if the launcher exits during fork.
    let parent = unsafe { libc::getpid() };
    // SAFETY: the child-side closure uses only async-signal-safe syscalls. All
    // paths, rules and filter storage are prepared in the parent before fork.
    unsafe {
        command.pre_exec(move || {
            if libc::setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL, 0, 0, 0) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::getppid() != parent {
                return Err(std::io::Error::from_raw_os_error(libc::ECHILD));
            }
            for (resource, limit) in [
                (libc::RLIMIT_AS, memory),
                (libc::RLIMIT_CPU, 30),
                (libc::RLIMIT_FSIZE, 64 * 1024 * 1024),
                (libc::RLIMIT_CORE, 0),
                (libc::RLIMIT_NOFILE, 256),
            ] {
                let value = libc::rlimit {
                    rlim_cur: limit,
                    rlim_max: limit,
                };
                if libc::setrlimit(resource, &value) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
            }
            if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::syscall(libc::SYS_landlock_restrict_self, ruleset.as_raw_fd(), 0u32) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            let program = libc::sock_fprog {
                len: filter.len() as u16,
                filter: filter.as_mut_ptr(),
            };
            if libc::prctl(
                libc::PR_SET_SECCOMP,
                libc::SECCOMP_MODE_FILTER,
                &program as *const libc::sock_fprog,
            ) != 0
            {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    Ok(())
}

fn add_existing(paths: &mut Vec<PathBuf>, path: impl Into<PathBuf>) {
    let path = path.into();
    if path.exists() {
        paths.push(path);
    }
}

fn base(
    program: &Path,
    directory: &Path,
    memory: u64,
    kind: SandboxKind,
    mut policy: SandboxPolicy,
) -> Result<Command, String> {
    let private_tmp = directory.join("private-tmp");
    std::fs::create_dir_all(&private_tmp).map_err(|error| error.to_string())?;
    std::fs::set_permissions(&private_tmp, std::fs::Permissions::from_mode(0o700))
        .map_err(|error| error.to_string())?;
    policy.read_write.push(private_tmp.clone());
    add_existing(&mut policy.read_write, "/dev/null");
    let mut command = Command::new(program);
    command
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", &private_tmp)
        .env("TMPDIR", &private_tmp)
        .current_dir(directory)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    apply_sandbox(&mut command, policy, memory, kind)?;
    Ok(command)
}

pub(crate) fn compiler(directory: &Path, toolchain: &Path) -> Result<Command, String> {
    let program = toolchain.join("build/bin/ziran");
    let mut policy = SandboxPolicy::default();
    for path in ["/usr", "/bin", "/lib", "/lib64", "/etc/alternatives"] {
        add_existing(&mut policy.read_exec, path);
    }
    add_existing(&mut policy.read_only, "/etc/ld.so.cache");
    policy.read_exec.push(toolchain.join("build/bin"));
    policy.read_only.push(toolchain.join("std"));
    policy.read_only.push(toolchain.join("include"));
    policy.read_write.push(directory.to_path_buf());
    let mut command = base(
        &program,
        directory,
        1024 * 1024 * 1024,
        SandboxKind::Compiler,
        policy,
    )?;
    command.env("ZIRAN_JOBS", "1").args([
        "build",
        "--target=c",
        "--exe",
        "--entry",
        "main:main",
        "--root",
    ]);
    command
        .arg(directory)
        .arg("--module-path")
        .arg(format!("xcode={}", directory.join("api").display()));
    command.arg("-o").arg(directory.join("out"));
    command.arg(directory.join("main.zi"));
    Ok(command)
}

pub(crate) fn cell(
    directory: &Path,
    heap_limit: Option<usize>,
) -> Result<(Child, Vec<tokio::net::UnixStream>), String> {
    let limit = heap_limit
        .map(|value| value as u64)
        .unwrap_or(256 * 1024 * 1024);
    if limit < 16 * 1024 * 1024 {
        return Err("native cell memory limit must be at least 16 MiB".into());
    }
    let executable = directory.join("out/main");
    if !std::fs::symlink_metadata(&executable)
        .map_err(|error| error.to_string())?
        .file_type()
        .is_file()
    {
        return Err("Zi2c did not produce a regular executable".into());
    }
    let mut policy = SandboxPolicy::default();
    policy.read_exec.push(executable.clone());
    for path in ["/lib", "/lib64", "/usr/lib"] {
        add_existing(&mut policy.read_exec, path);
    }
    add_existing(&mut policy.read_only, "/etc/ld.so.cache");
    let mut command = base(&executable, directory, limit, SandboxKind::Cell, policy)?;
    // Pathname Unix-socket mediation requires Landlock ABI 9. Alpha currently
    // exposes ABI 8, so pass four already-connected callback channels instead.
    // The cell cannot create or connect sockets after seccomp is installed.
    let mut channels = Vec::with_capacity(4);
    let mut inherited = Vec::with_capacity(4);
    for _ in 0..4 {
        let (parent, child) = std::os::unix::net::UnixStream::pair()
            .map_err(|error| format!("create native callback channel: {error}"))?;
        parent
            .set_nonblocking(true)
            .map_err(|error| error.to_string())?;
        let descriptor = unsafe { libc::fcntl(child.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 16) };
        if descriptor < 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        // SAFETY: F_DUPFD_CLOEXEC returned a new descriptor owned here.
        inherited.push(unsafe { OwnedFd::from_raw_fd(descriptor) });
        channels.push(tokio::net::UnixStream::from_std(parent).map_err(|error| error.to_string())?);
    }
    // SAFETY: dup2/fcntl are async-signal-safe. The high numbered source
    // descriptors cannot be clobbered while mapping channels to descriptors 3-6.
    unsafe {
        command.pre_exec(move || {
            for (index, descriptor) in inherited.iter().enumerate() {
                let target = 3 + index as i32;
                if libc::dup2(descriptor.as_raw_fd(), target) < 0
                    || libc::fcntl(target, libc::F_SETFD, 0) < 0
                {
                    return Err(std::io::Error::last_os_error());
                }
            }
            Ok(())
        });
    }
    let child = spawn_written_executable(&mut command)
        .map_err(|error| format!("start native cell sandbox: {error}"))?;
    Ok((child, channels))
}

/// Spawns an executable that was just written, retrying `ETXTBSY`.
///
/// Every file is opened close-on-exec, but a thread that forks at the same
/// moment holds a copy of the writer's descriptor until its own `exec`, so a
/// freshly written cell can briefly be "busy" while parallel cells start.
fn spawn_written_executable(command: &mut Command) -> std::io::Result<Child> {
    const ATTEMPTS: u64 = 20;
    let mut attempt = 1;
    loop {
        match command.spawn() {
            Err(error) if error.raw_os_error() == Some(libc::ETXTBSY) && attempt < ATTEMPTS => {
                std::thread::sleep(std::time::Duration::from_millis(2 * attempt));
                attempt += 1;
            }
            result => return result,
        }
    }
}

pub(crate) fn kill_process_group(process_group: u32) {
    if let Ok(process_group) = i32::try_from(process_group) {
        // SAFETY: a negative PID targets only the private session created for
        // this compiler. ESRCH simply means it already exited.
        unsafe {
            libc::kill(-process_group, libc::SIGKILL);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn native_cell_cannot_read_host_files_or_open_inet_sockets() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(directory.path().join("out")).unwrap();
        let source = directory.path().join("probe.c");
        std::fs::write(
            &source,
            r#"#include <errno.h>
#include <fcntl.h>
#include <sys/socket.h>
#include <unistd.h>
int main(void) {
    int file = open("/etc/passwd", O_RDONLY);
    if (file >= 0 || errno != EACCES) return 10;
    int socket_fd = socket(AF_INET, SOCK_STREAM, 0);
    if (socket_fd >= 0 || errno != EPERM) return 11;
    return write(1, "isolated\n", 9) == 9 ? 0 : 12;
}
"#,
        )
        .unwrap();
        let status = std::process::Command::new("/usr/bin/cc")
            .arg(&source)
            .arg("-o")
            .arg(directory.path().join("out/main"))
            .status()
            .unwrap();
        assert!(status.success());
        let (child, _channels) = cell(directory.path(), None).unwrap();
        let output = child.wait_with_output().await.unwrap();
        assert!(output.status.success(), "{output:?}");
        assert_eq!(output.stdout, b"isolated\n");
    }
}
