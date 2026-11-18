//! A minimal Git object store for Codex's private baseline repositories.
//! Object formats come from the Ziran `git` package (`native/git.zi`); this
//! module does the file I/O. It supports SHA-1 repositories with loose
//! objects, which is all a baseline repository ever holds.

use anyhow::Context;
use std::fmt;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;

const ID_BYTES: usize = 20;
const DEFAULT_BRANCH: &str = "refs/heads/main";

unsafe extern "C" {
    fn xcode_git_hash_object(kind: i64, data: *const u8, length: i64, id: *mut u8) -> i32;
    fn xcode_git_loose_encoded_size(kind: i64, size: i64) -> i64;
    fn xcode_git_loose_encode(
        kind: i64,
        data: *const u8,
        length: i64,
        out: *mut u8,
        capacity: i64,
    ) -> i64;
    fn xcode_git_loose_decode(
        compressed: *const u8,
        length: i64,
        out: *mut u8,
        capacity: i64,
    ) -> i64;
    fn xcode_git_parse_header(data: *const u8, length: i64, kind: *mut i64, size: *mut i64)
    -> i64;
    fn xcode_git_tree_sort(items: *mut NativeTreeItem, scratch: *mut NativeTreeItem, count: i64)
    -> i32;
    fn xcode_git_tree_encoded_size(items: *const NativeTreeItem, count: i64) -> i64;
    fn xcode_git_tree_encode(
        items: *const NativeTreeItem,
        count: i64,
        out: *mut u8,
        capacity: i64,
    ) -> i64;
    fn xcode_git_tree_next(tree: *const u8, length: i64, at: i64, entry: *mut NativeTreeEntry)
    -> i64;
    fn xcode_git_commit_encoded_size(
        parent_count: i64,
        author: *const NativeSignature,
        committer: *const NativeSignature,
        message_length: i64,
    ) -> i64;
    fn xcode_git_commit_encode(
        tree: *const u8,
        parents: *const u8,
        parents_length: i64,
        author: *const NativeSignature,
        committer: *const NativeSignature,
        message: *const u8,
        message_length: i64,
        out: *mut u8,
        capacity: i64,
    ) -> i64;
    fn xcode_git_commit_tree(commit: *const u8, length: i64, tree: *mut u8) -> i32;
    fn xcode_git_parse_ref(
        text: *const u8,
        length: i64,
        target_start: *mut i64,
        target_end: *mut i64,
        id: *mut u8,
    ) -> i64;
    fn xcode_git_packed_ref(
        text: *const u8,
        length: i64,
        name: *const u8,
        name_length: i64,
        id: *mut u8,
    ) -> i32;
}

/// A borrowed Ziran `[]u8`.
#[repr(C)]
#[derive(Clone, Copy)]
struct NativeBytes {
    data: *const u8,
    length: i64,
}

impl NativeBytes {
    fn of(bytes: &[u8]) -> Self {
        Self {
            data: bytes.as_ptr(),
            length: bytes.len() as i64,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct NativeTreeItem {
    name: NativeBytes,
    mode: i64,
    id: NativeBytes,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct NativeTreeEntry {
    mode: i64,
    name_start: i64,
    name_end: i64,
    id_start: i64,
}

#[repr(C)]
struct NativeSignature {
    name: NativeBytes,
    email: NativeBytes,
    seconds: i64,
    offset_minutes: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Commit = 1,
    Tree = 2,
    Blob = 3,
}

/// A SHA-1 object id.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct ObjectId([u8; ID_BYTES]);

impl ObjectId {
    pub(crate) fn hash(kind: Kind, data: &[u8]) -> Self {
        let mut id = [0u8; ID_BYTES];
        // SAFETY: `data` covers its length and `id` holds 20 bytes.
        let ok =
            unsafe { xcode_git_hash_object(kind as i64, data.as_ptr(), data.len() as i64, id.as_mut_ptr()) };
        assert_eq!(ok, 1, "hash of a known object kind");
        Self(id)
    }

    fn hex(&self) -> String {
        self.0.iter().map(|byte| format!("{byte:02x}")).collect()
    }
}

impl fmt::Display for ObjectId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.hex())
    }
}

impl fmt::Debug for ObjectId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ObjectId({})", self.hex())
    }
}

/// Tree entry modes Git writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum EntryMode {
    Tree,
    Blob,
    BlobExecutable,
    Link,
    Commit,
}

impl EntryMode {
    fn bits(self) -> i64 {
        match self {
            Self::Tree => 0o40000,
            Self::Blob => 0o100644,
            Self::BlobExecutable => 0o100755,
            Self::Link => 0o120000,
            Self::Commit => 0o160000,
        }
    }

    fn from_bits(bits: i64) -> Option<Self> {
        // Git also accepts the historical group-writable blob mode.
        match bits {
            0o40000 => Some(Self::Tree),
            0o100644 | 0o100664 => Some(Self::Blob),
            0o100755 => Some(Self::BlobExecutable),
            0o120000 => Some(Self::Link),
            0o160000 => Some(Self::Commit),
            _ => None,
        }
    }

    pub(crate) fn is_tree(self) -> bool {
        self == Self::Tree
    }

    /// The octal mode as Git prints it in diffs.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Blob => "100644",
            Self::BlobExecutable => "100755",
            Self::Link => "120000",
            Self::Tree => "040000",
            Self::Commit => "160000",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TreeEntry {
    pub(crate) name: Vec<u8>,
    pub(crate) mode: EntryMode,
    pub(crate) id: ObjectId,
}

/// Encodes entries (in any order) as tree content in Git's order.
pub(crate) fn encode_tree(entries: &[TreeEntry]) -> Vec<u8> {
    let mut items: Vec<NativeTreeItem> = entries
        .iter()
        .map(|entry| NativeTreeItem {
            name: NativeBytes::of(&entry.name),
            mode: entry.mode.bits(),
            id: NativeBytes::of(&entry.id.0),
        })
        .collect();
    let mut scratch = items.clone();
    let count = items.len() as i64;
    // SAFETY: both item buffers hold `count` entries borrowing `entries`.
    unsafe {
        assert_eq!(xcode_git_tree_sort(items.as_mut_ptr(), scratch.as_mut_ptr(), count), 1);
        let size = xcode_git_tree_encoded_size(items.as_ptr(), count);
        let mut out = vec![0u8; size as usize];
        let written = xcode_git_tree_encode(items.as_ptr(), count, out.as_mut_ptr(), size);
        assert_eq!(written, size, "tree entries have valid names and modes");
        out
    }
}

pub(crate) fn decode_tree(tree: &[u8]) -> anyhow::Result<Vec<TreeEntry>> {
    let mut entries = Vec::new();
    let mut at = 0i64;
    while (at as usize) < tree.len() {
        let mut entry = NativeTreeEntry::default();
        // SAFETY: `tree` covers its length; `entry` is a valid out pointer.
        let next = unsafe { xcode_git_tree_next(tree.as_ptr(), tree.len() as i64, at, &mut entry) };
        anyhow::ensure!(next > at, "malformed tree object");
        let mode = EntryMode::from_bits(entry.mode).context("unknown tree entry mode")?;
        let mut id = [0u8; ID_BYTES];
        id.copy_from_slice(&tree[entry.id_start as usize..entry.id_start as usize + ID_BYTES]);
        entries.push(TreeEntry {
            name: tree[entry.name_start as usize..entry.name_end as usize].to_vec(),
            mode,
            id: ObjectId(id),
        });
        at = next;
    }
    Ok(entries)
}

pub(crate) struct Signature<'a> {
    pub(crate) name: &'a str,
    pub(crate) email: &'a str,
    pub(crate) seconds: i64,
}

/// A private repository with a work tree: `root/.git` holding loose objects.
pub(crate) struct Repository {
    git_dir: PathBuf,
}

impl Repository {
    /// Creates `root/.git` (which must not exist) with an unborn `main` branch.
    pub(crate) fn init(root: &Path) -> anyhow::Result<Self> {
        let git_dir = root.join(".git");
        fs::create_dir(&git_dir).with_context(|| format!("create {}", git_dir.display()))?;
        for directory in ["objects/info", "objects/pack", "refs/heads", "refs/tags", "info"] {
            fs::create_dir_all(git_dir.join(directory))?;
        }
        fs::write(
            git_dir.join("config"),
            "[core]\n\trepositoryformatversion = 0\n\tfilemode = true\n\tbare = false\n\tlogallrefupdates = true\n",
        )?;
        fs::write(git_dir.join("HEAD"), format!("ref: {DEFAULT_BRANCH}\n"))?;
        Ok(Self { git_dir })
    }

    /// Opens a git directory that is its own common directory (not a linked
    /// work tree) and uses SHA-1.
    pub(crate) fn open(git_dir: &Path) -> anyhow::Result<Self> {
        anyhow::ensure!(git_dir.join("objects").is_dir(), "git directory has no objects");
        anyhow::ensure!(git_dir.join("HEAD").is_file(), "git directory has no HEAD");
        anyhow::ensure!(
            !git_dir.join("commondir").try_exists()?,
            "git directory belongs to another repository"
        );
        let config = fs::read_to_string(git_dir.join("config")).unwrap_or_default();
        anyhow::ensure!(
            !config.contains("objectformat"),
            "only SHA-1 repositories are supported"
        );
        Ok(Self {
            git_dir: git_dir.to_owned(),
        })
    }

    fn object_path(&self, id: &ObjectId) -> PathBuf {
        let hex = id.hex();
        self.git_dir.join("objects").join(&hex[..2]).join(&hex[2..])
    }

    pub(crate) fn write_object(&self, kind: Kind, data: &[u8]) -> anyhow::Result<ObjectId> {
        let id = ObjectId::hash(kind, data);
        let path = self.object_path(&id);
        if path.is_file() {
            return Ok(id);
        }
        // SAFETY: `out` holds the size the package reports for this object.
        let encoded = unsafe {
            let size = xcode_git_loose_encoded_size(kind as i64, data.len() as i64);
            let mut out = vec![0u8; size as usize];
            let written = xcode_git_loose_encode(
                kind as i64,
                data.as_ptr(),
                data.len() as i64,
                out.as_mut_ptr(),
                size,
            );
            anyhow::ensure!(written == size, "encode loose object");
            out
        };
        let directory = path.parent().context("loose object directory")?;
        fs::create_dir_all(directory)?;
        let mut file = tempfile::NamedTempFile::new_in(directory)?;
        file.write_all(&encoded)?;
        file.persist(&path)
            .with_context(|| format!("write object {}", path.display()))?;
        Ok(id)
    }

    /// Reads an object whose content is at most `limit` bytes.
    pub(crate) fn read_object(&self, id: &ObjectId, limit: u64) -> anyhow::Result<(Kind, Vec<u8>)> {
        let path = self.object_path(id);
        let compressed = fs::read(&path).with_context(|| format!("read object {id}"))?;
        // Inflate into a buffer that grows up to the limit plus the header.
        let ceiling = limit.saturating_add(32).min(i32::MAX as u64) as usize;
        let mut capacity = (compressed.len().saturating_mul(4)).clamp(64, ceiling);
        loop {
            let mut out = vec![0u8; capacity];
            // SAFETY: both buffers cover the lengths passed with them.
            let length = unsafe {
                xcode_git_loose_decode(
                    compressed.as_ptr(),
                    compressed.len() as i64,
                    out.as_mut_ptr(),
                    capacity as i64,
                )
            };
            if length >= 0 {
                let (mut kind, mut size) = (0i64, 0i64);
                // SAFETY: `out` holds `length` inflated bytes.
                let header = unsafe {
                    xcode_git_parse_header(out.as_ptr(), length, &mut kind, &mut size)
                };
                anyhow::ensure!(header > 0, "malformed object header");
                anyhow::ensure!(size as u64 <= limit, "object {id} exceeds the size limit");
                let kind = match kind {
                    1 => Kind::Commit,
                    2 => Kind::Tree,
                    3 => Kind::Blob,
                    _ => anyhow::bail!("unsupported object kind"),
                };
                out.truncate(length as usize);
                out.drain(..header as usize);
                return Ok((kind, out));
            }
            anyhow::ensure!(capacity < ceiling, "object {id} is corrupt or exceeds the size limit");
            capacity = capacity.saturating_mul(4).min(ceiling);
        }
    }

    pub(crate) fn read_kind(&self, id: &ObjectId, kind: Kind, limit: u64) -> anyhow::Result<Vec<u8>> {
        let (found, data) = self.read_object(id, limit)?;
        anyhow::ensure!(found == kind, "object {id} is a {found:?}, not a {kind:?}");
        Ok(data)
    }

    fn resolve(&self, name: &str) -> anyhow::Result<Option<ObjectId>> {
        let mut name = name.to_owned();
        for _ in 0..5 {
            let path = self.git_dir.join(&name);
            let text = match fs::read(&path) {
                Ok(text) => text,
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                    let packed = match fs::read(self.git_dir.join("packed-refs")) {
                        Ok(packed) => packed,
                        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                        Err(err) => return Err(err.into()),
                    };
                    let mut id = [0u8; ID_BYTES];
                    // SAFETY: each buffer covers the length passed with it.
                    let found = unsafe {
                        xcode_git_packed_ref(
                            packed.as_ptr(),
                            packed.len() as i64,
                            name.as_ptr(),
                            name.len() as i64,
                            id.as_mut_ptr(),
                        )
                    };
                    return Ok((found == 1).then_some(ObjectId(id)));
                }
                Err(err) => return Err(err.into()),
            };
            let (mut start, mut end) = (0i64, 0i64);
            let mut id = [0u8; ID_BYTES];
            // SAFETY: `text` covers its length; the out pointers are valid.
            let kind = unsafe {
                xcode_git_parse_ref(text.as_ptr(), text.len() as i64, &mut start, &mut end, id.as_mut_ptr())
            };
            match kind {
                1 => {
                    name = std::str::from_utf8(&text[start as usize..end as usize])
                        .context("ref name is not UTF-8")?
                        .to_owned();
                    anyhow::ensure!(
                        name.starts_with("refs/") && !name.split('/').any(|part| part == ".."),
                        "symbolic ref points outside refs/"
                    );
                }
                2 => return Ok(Some(ObjectId(id))),
                _ => anyhow::bail!("malformed ref {name}"),
            }
        }
        anyhow::bail!("symbolic refs nest too deeply")
    }

    /// The tree of the commit HEAD points to.
    pub(crate) fn head_tree_id(&self) -> anyhow::Result<ObjectId> {
        let commit_id = self.resolve("HEAD")?.context("HEAD has no commit")?;
        let commit = self.read_kind(&commit_id, Kind::Commit, 1 << 20)?;
        let mut tree = [0u8; ID_BYTES];
        // SAFETY: `commit` covers its length and `tree` holds 20 bytes.
        let ok = unsafe { xcode_git_commit_tree(commit.as_ptr(), commit.len() as i64, tree.as_mut_ptr()) };
        anyhow::ensure!(ok == 1, "malformed commit {commit_id}");
        Ok(ObjectId(tree))
    }

    pub(crate) fn read_tree(&self, id: &ObjectId) -> anyhow::Result<Vec<TreeEntry>> {
        decode_tree(&self.read_kind(id, Kind::Tree, 64 << 20)?)
    }

    /// Writes a commit and points HEAD's branch at it.
    pub(crate) fn commit(
        &self,
        signature: &Signature<'_>,
        message: &str,
        tree: ObjectId,
        parents: &[ObjectId],
    ) -> anyhow::Result<ObjectId> {
        let native = NativeSignature {
            name: NativeBytes::of(signature.name.as_bytes()),
            email: NativeBytes::of(signature.email.as_bytes()),
            seconds: signature.seconds,
            offset_minutes: 0,
        };
        let parent_bytes: Vec<u8> = parents.iter().flat_map(|parent| parent.0).collect();
        // SAFETY: every buffer covers the length passed with it, and `out`
        // holds the size the package reports.
        let content = unsafe {
            let size = xcode_git_commit_encoded_size(
                parents.len() as i64,
                &native,
                &native,
                message.len() as i64,
            );
            let mut out = vec![0u8; size as usize];
            let written = xcode_git_commit_encode(
                tree.0.as_ptr(),
                parent_bytes.as_ptr(),
                parent_bytes.len() as i64,
                &native,
                &native,
                message.as_ptr(),
                message.len() as i64,
                out.as_mut_ptr(),
                size,
            );
            anyhow::ensure!(written == size, "encode commit (invalid signature?)");
            out
        };
        let id = self.write_object(Kind::Commit, &content)?;
        let head = fs::read_to_string(self.git_dir.join("HEAD"))?;
        let branch = head
            .strip_prefix("ref: ")
            .map(str::trim_end)
            .unwrap_or(DEFAULT_BRANCH);
        anyhow::ensure!(
            branch.starts_with("refs/heads/") && !branch.split('/').any(|part| part == ".."),
            "HEAD does not name a branch"
        );
        let path = self.git_dir.join(branch);
        let directory = path.parent().context("branch directory")?;
        fs::create_dir_all(directory)?;
        let mut file = tempfile::NamedTempFile::new_in(directory)?;
        writeln!(file, "{id}")?;
        file.persist(&path)?;
        Ok(id)
    }
}

#[cfg(test)]
#[path = "objects_tests.rs"]
mod tests;
