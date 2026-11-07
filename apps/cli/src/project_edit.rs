use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

use sha2::{Digest, Sha256};

pub(crate) struct ProjectEditError {
    pub code: &'static str,
    pub message: String,
}

impl ProjectEditError {
    pub fn retryable(&self) -> bool {
        self.code == "PROJECT_BUSY"
    }
}

pub(crate) type Result<T> = std::result::Result<T, ProjectEditError>;

pub(crate) fn error(code: &'static str, message: impl Into<String>) -> ProjectEditError {
    ProjectEditError {
        code,
        message: message.into(),
    }
}

pub(crate) fn io_error(cause: impl std::fmt::Display) -> ProjectEditError {
    error("IO_ERROR", cause.to_string())
}

pub(crate) fn revision(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub(crate) struct TemporaryFile {
    pub path: PathBuf,
}

impl TemporaryFile {
    pub fn write(destination: &Path, bytes: &[u8]) -> Result<Self> {
        let path = destination.with_file_name(format!(".cap-edit-{}.tmp", uuid::Uuid::new_v4()));
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)
            .map_err(io_error)?;
        let temporary = Self { path };
        file.write_all(bytes).map_err(io_error)?;
        file.sync_all().map_err(io_error)?;
        Ok(temporary)
    }
}

impl Drop for TemporaryFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

pub(crate) fn write_checked(
    destination: &Path,
    expected: Option<&[u8]>,
    bytes: &[u8],
) -> Result<()> {
    let _lock = acquire_write_lock(destination)?;
    let temporary = TemporaryFile::write(destination, bytes)?;
    let current = match fs::read(destination) {
        Ok(bytes) => Some(bytes),
        Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => None,
        Err(cause) => return Err(io_error(cause)),
    };
    if current.as_deref() != expected {
        return Err(error(
            "REVISION_CONFLICT",
            "The project changed while preparing the edit; read it again before retrying",
        ));
    }
    crate::atomic::replace(&temporary.path, destination).map_err(io_error)
}

fn acquire_write_lock(destination: &Path) -> Result<fs::File> {
    let path = destination.with_file_name(".cap-project-edit.lock");
    let mut options = OpenOptions::new();
    options.create(true).truncate(false).read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(0);
    }
    let file = options.open(path).map_err(|cause| {
        if cfg!(windows) && cause.raw_os_error() == Some(32) {
            error(
                "PROJECT_BUSY",
                "Another CLI edit is being saved; retry shortly",
            )
        } else {
            io_error(cause)
        }
    })?;
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            let cause = std::io::Error::last_os_error();
            return Err(if cause.kind() == std::io::ErrorKind::WouldBlock {
                error(
                    "PROJECT_BUSY",
                    "Another CLI edit is being saved; retry shortly",
                )
            } else {
                io_error(cause)
            });
        }
    }
    // Keep the lock inode: unlinking it lets another process lock a different file at the same path.
    Ok(file)
}
