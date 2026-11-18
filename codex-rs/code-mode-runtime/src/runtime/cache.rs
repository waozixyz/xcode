//! Private, bounded cache of sandboxed custom-cell executables. Keys include
//! source, API, compiler and toolchain inputs. Cache hits still run sandboxed.
use super::*;
use sha2::{Digest, Sha256};
use std::io::{Read, Seek, SeekFrom};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::sync::{LazyLock, Mutex, Weak};

static LOCKS: LazyLock<Mutex<HashMap<String, Weak<tokio::sync::Mutex<()>>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
static COMPILER: Mutex<Option<(String, String)>> = Mutex::new(None);

fn digest_file(path: &Path) -> Result<String, String> {
    let file = std::fs::File::open(path).map_err(|error| error.to_string())?;
    digest_reader(file)
}

fn digest_reader(mut reader: impl Read) -> Result<String, String> {
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 65536];
    loop {
        let count = reader
            .read(&mut buffer)
            .map_err(|error| error.to_string())?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

fn compiler_key(toolchain: &Path) -> Result<String, String> {
    let mut files = vec![
        toolchain.join("build/bin/ziran"),
        toolchain.join("build/bin/zi2c"),
    ];
    let mut folders = vec![toolchain.join("std"), toolchain.join("include")];
    while let Some(folder) = folders.pop() {
        for entry in std::fs::read_dir(folder).map_err(|error| error.to_string())? {
            let path = entry.map_err(|error| error.to_string())?.path();
            if path.is_dir() {
                folders.push(path);
            } else if path.is_file() {
                files.push(path);
            }
        }
    }
    files.push(PathBuf::from("/usr/bin/cc"));
    files.sort();
    let mut stamp = Sha256::new();
    for path in &files {
        let metadata = path
            .metadata()
            .map_err(|error| format!("compiler input {}: {error}", path.display()))?;
        stamp.update(path.as_os_str().as_encoded_bytes());
        for n in [
            metadata.dev(),
            metadata.ino(),
            metadata.len(),
            metadata.mtime() as u64,
            metadata.mtime_nsec() as u64,
        ] {
            stamp.update(n.to_le_bytes());
        }
    }
    let signature = format!("{:x}", stamp.finalize());
    let mut previous = COMPILER
        .lock()
        .map_err(|_| "compiler fingerprint lock poisoned")?;
    if let Some((old_signature, key)) = previous.as_ref() {
        if old_signature == &signature {
            return Ok(key.clone());
        }
    }
    let mut hash = Sha256::new();
    hash.update(signature.as_bytes());
    for path in files {
        hash.update(digest_file(&path)?.as_bytes());
    }
    let key = format!("{:x}", hash.finalize());
    *previous = Some((signature, key.clone()));
    Ok(key)
}

fn private_cache(root: &Path) -> Result<PathBuf, String> {
    let cache = root.join("cache");
    std::fs::create_dir_all(&cache).map_err(|error| error.to_string())?;
    let info = std::fs::symlink_metadata(&cache).map_err(|error| error.to_string())?;
    // SAFETY: geteuid has no pointer arguments or side effects.
    if !info.is_dir() || info.uid() != unsafe { libc::geteuid() } {
        return Err("native cell cache must be a directory owned by this user".into());
    }
    std::fs::set_permissions(&cache, std::fs::Permissions::from_mode(0o700))
        .map_err(|error| error.to_string())?;
    Ok(cache)
}

fn hit(cache: &Path, key: &str, output: &Path) -> Result<bool, String> {
    let binary = cache.join(format!("{key}.bin"));
    let receipt = cache.join(format!("{key}.json"));
    let valid_file = |path: &Path, limit| {
        std::fs::symlink_metadata(path).is_ok_and(|info| {
            info.is_file()
                && info.uid() == unsafe { libc::geteuid() }
                && info.mode() & 0o022 == 0
                && info.len() <= limit
        })
    };
    if !valid_file(&receipt, 4096) || !valid_file(&binary, MAX_FRAME as u64) {
        return Ok(false);
    }
    let Ok(bytes) = std::fs::read(&receipt) else {
        return Ok(false);
    };
    let metadata: JsonValue = match serde_json::from_slice(&bytes) {
        Ok(value) => value,
        Err(_) => return Ok(false),
    };
    if metadata["key"] != key {
        return Ok(false);
    }
    std::fs::create_dir_all(output.parent().ok_or("missing native output parent")?)
        .map_err(|error| error.to_string())?;
    let mut cached = match std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&binary)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(_) => return Ok(false),
    };
    let info = cached.metadata().map_err(|error| error.to_string())?;
    if !info.is_file()
        || info.uid() != unsafe { libc::geteuid() }
        || info.mode() & 0o022 != 0
        || info.len() > MAX_FRAME as u64
        || !digest_reader(&mut cached).is_ok_and(|digest| metadata["sha256"] == digest)
    {
        return Ok(false);
    }
    cached
        .seek(SeekFrom::Start(0))
        .map_err(|error| error.to_string())?;
    // Copy from the verified, pinned descriptor into a private inode. A cache
    // file can then be replaced or evicted without coupling an active exec to
    // another process's writable descriptor (which Linux reports as ETXTBSY).
    let mut staged =
        tempfile::NamedTempFile::new_in(output.parent().ok_or("missing native output parent")?)
            .map_err(|error| error.to_string())?;
    std::io::copy(&mut cached, &mut staged).map_err(|error| error.to_string())?;
    staged
        .as_file()
        .set_permissions(std::fs::Permissions::from_mode(0o500))
        .map_err(|error| error.to_string())?;
    staged
        .as_file()
        .sync_all()
        .map_err(|error| error.to_string())?;
    let executable = staged.persist(output).map_err(|error| error.to_string())?;
    drop(executable);
    Ok(true)
}

fn publish(cache: &Path, key: &str, executable: &Path) -> Result<(), String> {
    let binary = tempfile::NamedTempFile::new_in(cache).map_err(|error| error.to_string())?;
    std::fs::copy(executable, binary.path()).map_err(|error| error.to_string())?;
    binary
        .as_file()
        .set_permissions(std::fs::Permissions::from_mode(0o500))
        .map_err(|error| error.to_string())?;
    binary
        .as_file()
        .sync_all()
        .map_err(|error| error.to_string())?;
    let metadata = json!({"key": key, "sha256": digest_file(binary.path())?});
    binary
        .persist(cache.join(format!("{key}.bin")))
        .map_err(|error| error.to_string())?;
    let receipt = tempfile::NamedTempFile::new_in(cache).map_err(|error| error.to_string())?;
    std::fs::write(
        receipt.path(),
        serde_json::to_vec(&metadata).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    receipt
        .persist(cache.join(format!("{key}.json")))
        .map_err(|error| error.to_string())?;
    // Keep at most 64 executables and 256 MiB on disk; active cells pin theirs.
    let mut entries = Vec::new();
    for entry in std::fs::read_dir(cache).map_err(|error| error.to_string())? {
        let path = entry.map_err(|error| error.to_string())?.path();
        if path.extension().is_some_and(|extension| extension == "bin") {
            let Ok(info) = std::fs::symlink_metadata(&path) else {
                continue;
            };
            if info.is_file() {
                entries.push((info.mtime(), path, info.len()));
            }
        }
    }
    entries.sort();
    let mut size: u64 = entries.iter().map(|entry| entry.2).sum();
    let mut count = entries.len();
    for (_, path, bytes) in entries {
        if count <= 64 && size <= 256 * 1024 * 1024 {
            break;
        }
        if path.file_stem().is_some_and(|stem| stem == key) {
            continue;
        }
        if std::fs::remove_file(&path).is_ok() {
            let _ = std::fs::remove_file(path.with_extension("json"));
            count -= 1;
            size -= bytes;
        }
    }
    Ok(())
}

pub(super) async fn prepare(
    directory: &Path,
    toolchain: &Path,
    source: &str,
) -> Result<bool, String> {
    let mut hash = Sha256::new();
    hash.update(b"xcode-cell-cache-v1\0");
    hash.update(compiler_key(toolchain)?.as_bytes());
    hash.update(include_bytes!("../../../../native/runtime.zi"));
    hash.update(include_bytes!("../native_sandbox.rs"));
    hash.update(source.as_bytes());
    let key = format!("{:x}", hash.finalize());
    let cache = private_cache(directory.parent().ok_or("missing native runtime root")?)?;
    let lock = {
        let mut locks = LOCKS.lock().map_err(|_| "native compile lock poisoned")?;
        locks.retain(|_, value| value.strong_count() > 0);
        let lock = locks
            .get(&key)
            .and_then(Weak::upgrade)
            .unwrap_or_else(|| Arc::new(tokio::sync::Mutex::new(())));
        locks.insert(key.clone(), Arc::downgrade(&lock));
        lock
    };
    let _guard = lock.lock().await;
    let output = directory.join("out/main");
    if hit(&cache, &key, &output)? {
        return Ok(true);
    }
    let mut compiler = crate::native_sandbox::compiler(directory, toolchain)?
        .spawn()
        .map_err(|error| format!("start Zi2c sandbox: {error}"))?;
    let process_group = compiler.id().ok_or("Zi2c compiler PID unavailable")?;
    let stdout = compiler
        .stdout
        .take()
        .ok_or("compiler stdout unavailable")?;
    let stderr = compiler
        .stderr
        .take()
        .ok_or("compiler stderr unavailable")?;
    let compilation = async {
        let (status, out, err) = tokio::join!(
            compiler.wait(),
            bounded_read(stdout, 64 * 1024),
            bounded_read(stderr, 64 * 1024)
        );
        Ok::<_, String>((status.map_err(|error| error.to_string())?, out?, err?))
    };
    let result = tokio::time::timeout(Duration::from_secs(30), compilation).await;
    let (status, out, err) = match result {
        Ok(result) => result?,
        Err(_) => {
            crate::native_sandbox::kill_process_group(process_group);
            let _ = compiler.wait().await;
            return Err("Zi2c compilation exceeded 30 seconds".into());
        }
    };
    if !status.success() {
        return Err(format!(
            "Zi2c compilation failed: {}{}",
            String::from_utf8_lossy(&out),
            String::from_utf8_lossy(&err)
        ));
    }
    let info = std::fs::symlink_metadata(&output).map_err(|error| error.to_string())?;
    if !info.is_file() || info.len() > MAX_FRAME as u64 {
        return Err("invalid compiled cell executable".into());
    }
    if let Err(error) = publish(&cache, &key, &output) {
        // A cache write failure must not discard a successfully compiled cell.
        tracing::debug!("native cache publication skipped: {error}");
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NESTED_TEST: &str =
        "runtime::cache::tests::native_cells_run_inside_nested_mount_namespace_without_bwrap";

    fn cell(root: &Path, name: &str, source: &str) -> PathBuf {
        let directory = root.join(name);
        std::fs::create_dir_all(directory.join("api")).unwrap();
        std::fs::write(
            directory.join("api/runtime.zi"),
            include_bytes!("../../../../native/runtime.zi"),
        )
        .unwrap();
        std::fs::write(directory.join("main.zi"), source).unwrap();
        directory
    }

    #[tokio::test]
    async fn native_cells_run_inside_nested_mount_namespace_without_bwrap() {
        if std::env::var_os("XCODE_NESTED_SANDBOX_TEST").is_some() {
            let root = tempfile::tempdir_in(runtime_root().unwrap()).unwrap();
            let toolchain = crate::native_sandbox::toolchain().unwrap();
            let source = r#"c :: #system_library "libc";
Write :: (fd: s32, data: *u8, length: usize) -> s64 #foreign c "write";
main :: () -> s32 { bytes: [3]u8 = .[50,51,10]; unused Write(1,*bytes[0],3); return 0 }"#;
            let directory = cell(root.path(), "nested", source);
            assert!(!prepare(&directory, &toolchain, source).await.unwrap());
            let (child, _channels) = crate::native_sandbox::cell(&directory, None).unwrap();
            let output = child.wait_with_output().await.unwrap();
            assert!(output.status.success(), "{output:?}");
            assert_eq!(output.stdout, b"23\n");
            return;
        }

        let fixture = tempfile::tempdir().unwrap();
        let fake_bwrap = fixture.path().join("bwrap");
        std::fs::write(
            &fake_bwrap,
            "#!/bin/sh\nprintf '%s\\n' 'bwrap: Failed to make / slave: Operation not permitted' >&2\nexit 1\n",
        )
        .unwrap();
        std::fs::set_permissions(&fake_bwrap, std::fs::Permissions::from_mode(0o700)).unwrap();
        let output = std::process::Command::new("/usr/bin/unshare")
            .args([
                "--user",
                "--map-root-user",
                "--mount",
                "--propagation",
                "unchanged",
            ])
            .arg(std::env::current_exe().unwrap())
            .args(["--exact", NESTED_TEST, "--nocapture"])
            .env("XCODE_NESTED_SANDBOX_TEST", "1")
            .env("XCODE_BWRAP", fake_bwrap)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "nested test failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[tokio::test]
    async fn real_cache_hit_executes_same_binary_and_corruption_rebuilds() {
        let root = tempfile::tempdir_in(runtime_root().unwrap()).unwrap();
        let toolchain = crate::native_sandbox::toolchain().unwrap();
        let source = r#"c :: #system_library "libc";
Write :: (fd: s32, data: *u8, length: usize) -> s64 #foreign c "write";
main :: () -> s32 { bytes: [3]u8 = .[50,51,10]; unused Write(1,*bytes[0],3); return 0 }"#;
        let first = cell(root.path(), "first", source);
        assert!(!prepare(&first, &toolchain, source).await.unwrap());
        let second = cell(root.path(), "second", source);
        assert!(prepare(&second, &toolchain, source).await.unwrap());
        let (child, _channels) = crate::native_sandbox::cell(&second, None).unwrap();
        let output = child.wait_with_output().await.unwrap();
        assert!(output.status.success(), "{output:?}");
        assert_eq!(output.stdout, b"23\n");
        let binary = std::fs::read_dir(root.path().join("cache"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| path.extension().is_some_and(|extension| extension == "bin"))
            .unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::write(binary, b"corrupt executable").unwrap();
        let third = cell(root.path(), "third", source);
        assert!(!prepare(&third, &toolchain, source).await.unwrap());
        let (child, _channels) = crate::native_sandbox::cell(&third, None).unwrap();
        let output = child.wait_with_output().await.unwrap();
        assert!(output.status.success(), "{output:?}");
        assert_eq!(output.stdout, b"23\n");
        let changed = source.replace("[50,51,10]", "[50,52,10]");
        let fourth = cell(root.path(), "fourth", &changed);
        assert!(!prepare(&fourth, &toolchain, &changed).await.unwrap());
        let (child, _channels) = crate::native_sandbox::cell(&fourth, None).unwrap();
        let output = child.wait_with_output().await.unwrap();
        assert!(output.status.success(), "{output:?}");
        assert_eq!(output.stdout, b"24\n");
    }

    #[tokio::test]
    async fn concurrent_identical_cells_compile_once_and_symlink_cache_is_rejected() {
        let root = tempfile::tempdir_in(runtime_root().unwrap()).unwrap();
        let toolchain = crate::native_sandbox::toolchain().unwrap();
        let source = "main :: () -> s32 { return 11 }";
        let mut jobs = JoinSet::new();
        for n in 0..4 {
            let directory = cell(root.path(), &format!("cell-{n}"), source);
            let toolchain = toolchain.clone();
            jobs.spawn(async move { prepare(&directory, &toolchain, source).await.unwrap() });
        }
        let mut hits = 0;
        while let Some(result) = jobs.join_next().await {
            if result.unwrap() {
                hits += 1;
            }
        }
        assert_eq!(hits, 3);
        let other = tempfile::tempdir_in(runtime_root().unwrap()).unwrap();
        std::os::unix::fs::symlink(root.path().join("cache"), other.path().join("cache")).unwrap();
        let directory = cell(other.path(), "cell", source);
        assert!(
            prepare(&directory, &toolchain, source)
                .await
                .unwrap_err()
                .contains("owned by this user")
        );
    }
}
