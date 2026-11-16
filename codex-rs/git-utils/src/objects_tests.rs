use super::*;
use pretty_assertions::assert_eq;
use std::process::Command;
use tempfile::TempDir;

fn git(root: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[test]
fn hashes_match_git() {
    assert_eq!(
        ObjectId::hash(Kind::Blob, b"hello\n").to_string(),
        "ce013625030ba8dba906f756967f9e9ca394464a"
    );
    assert_eq!(
        ObjectId::hash(Kind::Tree, b"").to_string(),
        "4b825dc642cb6eb9a060e54bf8d69288fbee4904"
    );
}

#[test]
fn git_reads_what_the_store_writes() {
    let dir = TempDir::new().expect("tempdir");
    let repo = Repository::init(dir.path()).expect("init");
    let big: Vec<u8> = (0..200_000u32).map(|i| (i * 7 % 251) as u8).collect();
    let hello = repo.write_object(Kind::Blob, b"hello\n").expect("blob");
    let large = repo.write_object(Kind::Blob, &big).expect("large blob");
    let inner = repo
        .write_object(
            Kind::Tree,
            &encode_tree(&[TreeEntry {
                name: b"x".to_vec(),
                mode: EntryMode::BlobExecutable,
                id: hello,
            }]),
        )
        .expect("inner tree");
    let entries = vec![
        TreeEntry { name: b"a.b".to_vec(), mode: EntryMode::Blob, id: hello },
        TreeEntry { name: b"a".to_vec(), mode: EntryMode::Tree, id: inner },
        TreeEntry { name: b"link".to_vec(), mode: EntryMode::Link, id: hello },
        TreeEntry { name: b"big".to_vec(), mode: EntryMode::Blob, id: large },
    ];
    let tree = repo.write_object(Kind::Tree, &encode_tree(&entries)).expect("tree");
    let signature = Signature { name: "Codex", email: "noreply@openai.com", seconds: 1_700_000_000 };
    let commit = repo.commit(&signature, "baseline\n\nbody", tree, &[]).expect("commit");

    assert_eq!(git(dir.path(), &["rev-parse", "HEAD"]).trim(), commit.to_string());
    assert_eq!(git(dir.path(), &["rev-parse", "HEAD^{tree}"]).trim(), tree.to_string());
    assert_eq!(
        git(dir.path(), &["ls-tree", "-r", "--name-only", "HEAD"]),
        "a.b\na/x\nbig\nlink\n"
    );
    assert_eq!(git(dir.path(), &["cat-file", "-p", &hello.to_string()]), "hello\n");
    assert_eq!(git(dir.path(), &["cat-file", "-s", &large.to_string()]).trim(), "200000");
    git(dir.path(), &["fsck", "--strict", "--no-dangling"]);

    // And the store reads its own objects back.
    assert_eq!(repo.head_tree_id().expect("head tree"), tree);
    let read = repo.read_tree(&tree).expect("read tree");
    let names: Vec<&[u8]> = read.iter().map(|entry| entry.name.as_slice()).collect();
    assert_eq!(names, vec![&b"a.b"[..], b"a", b"big", b"link"]);
    assert_eq!(repo.read_kind(&large, Kind::Blob, 200_000).expect("blob"), big);
    assert!(repo.read_kind(&large, Kind::Blob, 199_999).is_err());
}

#[test]
fn the_store_reads_what_git_writes() {
    let dir = TempDir::new().expect("tempdir");
    git(dir.path(), &["init", "-q", "-b", "main"]);
    fs::write(dir.path().join("file.txt"), "content\n".repeat(1000)).expect("write");
    fs::create_dir(dir.path().join("sub")).expect("mkdir");
    fs::write(dir.path().join("sub/run.sh"), "#!/bin/sh\n").expect("write");
    git(dir.path(), &["add", "."]);
    git(
        dir.path(),
        &["-c", "user.name=t", "-c", "user.email=t@example.com", "commit", "-q", "-m", "one"],
    );
    git(dir.path(), &["pack-refs", "--all"]);

    let repo = Repository::open(&dir.path().join(".git")).expect("open");
    let tree = repo.head_tree_id().expect("head tree");
    assert_eq!(tree.to_string(), git(dir.path(), &["rev-parse", "HEAD^{tree}"]).trim());
    let entries = repo.read_tree(&tree).expect("tree");
    assert_eq!(entries.len(), 2);
    let file = entries.iter().find(|entry| entry.name == b"file.txt").expect("file");
    assert_eq!(
        repo.read_kind(&file.id, Kind::Blob, 1 << 20).expect("blob"),
        "content\n".repeat(1000).into_bytes()
    );
}
