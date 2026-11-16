use crate::FileMetadata;
use std::io;
use std::path::Path;

mod unix;

use unix as imp;

fn invalid_output(reason: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, reason)
}

/// A destination bound before sandboxed tools run. Only ordinary path-based files are truncated.
pub struct BoundOutput {
    file: std::fs::File,
    destination: Destination,
}

enum Destination {
    Path(std::path::PathBuf),
    Stream { is_stdout: bool },
}

impl BoundOutput {
    pub fn open(path: &Path) -> io::Result<Self> {
        // Check raw syntax before absolute() or components() discards directory markers.
        let bytes = path.as_os_str().as_encoded_bytes();
        let leaf = bytes.rsplit(|b| *b == b'/').next();
        if matches!(leaf, None | Some(b"" | b"." | b"..")) {
            return Err(invalid_output("output must name a file"));
        }
        if let Some(output) = unix::inherited_output(path)? {
            return Ok(output);
        }
        let path = unix::output_path(path)?;
        let file = imp::open_output(&path)?;
        if file.metadata()?.is_file() {
            imp::reject_multiple_links(&file)?;
        }
        Ok(Self {
            file,
            destination: Destination::Path(path),
        })
    }

    /// Whether the caller explicitly selected the inherited standard output stream.
    pub fn is_stdout(&self) -> bool {
        matches!(self.destination, Destination::Stream { is_stdout: true })
    }

    pub fn write(mut self, contents: &[u8]) -> io::Result<()> {
        use std::io::Write;
        if let Destination::Path(path) = &self.destination {
            imp::verify_output_path(&self.file, path)?;
            if self.file.metadata()?.is_file() {
                imp::reject_multiple_links(&self.file)?;
                self.file.set_len(0)?;
            }
        }
        self.file.write_all(contents)?;
        // A caller may reopen the pathname after a successful exit. Detect a visible swap.
        if let Destination::Path(path) = &self.destination {
            imp::verify_output_path(&self.file, path)?;
        }
        Ok(())
    }
}

pub(crate) fn open_file(path: &Path) -> io::Result<std::fs::File> {
    imp::open_file_sync(path)
}

pub(crate) async fn write_file(path: &Path, contents: Vec<u8>) -> io::Result<()> {
    imp::write_file(path.to_path_buf(), contents).await
}

pub(crate) async fn metadata(path: &Path) -> io::Result<FileMetadata> {
    imp::metadata(path.to_path_buf()).await
}

pub(crate) async fn create_directory(path: &Path, recursive: bool) -> io::Result<()> {
    imp::create_directory(path.to_path_buf(), recursive).await
}

pub(crate) async fn remove(path: &Path, recursive: bool, force: bool) -> io::Result<()> {
    imp::remove(path.to_path_buf(), recursive, force).await
}

#[cfg(test)]
mod output_tests {
    //! Exercise the destination as a caller of the final-message writer.
    use super::BoundOutput;
    use pretty_assertions::assert_eq;
    use std::fs;

    #[test]
    fn regular_output_is_truncated_only_when_written() -> std::io::Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("message");
        fs::write(&path, "old message")?;
        let output = BoundOutput::open(&path)?;
        assert_eq!(fs::read_to_string(&path)?, "old message");
        output.write(b"new")?;
        assert_eq!(fs::read_to_string(&path)?, "new");
        Ok(())
    }

    #[test]
    fn rejects_added_links_and_replaced_paths_before_writing() -> std::io::Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("message");
        fs::write(&path, "new")?;
        let output = BoundOutput::open(&path)?;
        let alias = dir.path().join("alias");
        fs::hard_link(&path, &alias)?;
        assert!(BoundOutput::open(&alias).is_err());
        assert!(output.write(b"overwritten").is_err());
        assert_eq!(fs::read(&alias)?, b"new");
        fs::remove_file(&alias)?;
        let output = BoundOutput::open(&path)?;
        fs::rename(&path, &alias)?;
        fs::write(&path, "replacement")?;
        assert!(output.write(b"overwritten").is_err());
        assert_eq!(fs::read(&path)?, b"replacement");
        assert_eq!(fs::read(&alias)?, b"new");
        Ok(())
    }

    #[test]
    fn output_only_accepts_leading_parent_components() -> std::io::Result<()> {
        let dir = tempfile::tempdir()?;
        fs::create_dir_all(dir.path().join("a/b"))?;
        let path = dir.path().join("a/b/../../message");
        assert!(
            matches!(BoundOutput::open(&path), Err(e) if e.kind() == std::io::ErrorKind::InvalidInput)
        );
        assert!(!dir.path().join("message").exists());
        let mut expected = std::env::current_dir()?;
        expected.pop();
        assert_eq!(
            super::unix::output_path("./../message".as_ref())?,
            expected.join("message")
        );
        Ok(())
    }

    #[test]
    fn writes_fifo_device_and_inherited_pipe() -> anyhow::Result<()> {
        use std::io::Read;
        use std::os::fd::AsRawFd;
        use std::os::unix::fs::OpenOptionsExt;
        let dir = tempfile::tempdir()?;
        let fifo = dir.path().join("pipe");
        assert!(
            std::process::Command::new("mkfifo")
                .arg(&fifo)
                .status()?
                .success()
        );
        let mut reader = fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(&fifo)?;
        BoundOutput::open(&fifo)?.write(b"fifo")?;
        let mut buf = [0; 4];
        reader.read_exact(&mut buf)?;
        assert_eq!(buf, *b"fifo");
        BoundOutput::open(std::path::Path::new("/dev/null"))?.write(b"device")?;
        let stdout = BoundOutput::open(std::path::Path::new("/dev/stdout"))?;
        assert!(stdout.is_stdout());
        let read_only = rustix::io::dup(&reader)?;
        let alias = format!("/dev/fd/{}", read_only.as_raw_fd());
        assert!(
            matches!(BoundOutput::open(alias.as_ref()), Err(e) if e.kind() == std::io::ErrorKind::InvalidInput)
        );
        let writer = fs::OpenOptions::new().write(true).open(&fifo)?;
        // Match the inheritable pipe descriptor passed by shell process substitution.
        let inherited = rustix::io::dup(&writer)?;
        let alias = format!("/dev/fd/{}", inherited.as_raw_fd());
        BoundOutput::open(alias.as_ref())?.write(b"pipe")?;
        reader.read_exact(&mut buf)?;
        assert_eq!(buf, *b"pipe");
        Ok(())
    }
}
