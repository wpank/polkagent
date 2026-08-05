//! Protocol-safe, opt-in diagnostics for the ACP stdio process.

use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

const DEFAULT_MAX_BYTES: u64 = 1024 * 1024;
const DEFAULT_MAX_FILES: usize = 3;
const MAX_DETAIL_BYTES: usize = 4096;

/// Cloneable handle to an optional, bounded ACP JSONL diagnostic file.
#[derive(Clone, Default)]
pub struct AcpDiagnostics {
    writer: Option<Arc<Mutex<RotatingFile>>>,
}

impl AcpDiagnostics {
    /// Return a disabled sink. This is the default when no explicit path was supplied.
    #[must_use]
    pub fn disabled() -> Self {
        Self::default()
    }

    /// Open an explicitly requested diagnostic file.
    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        Self::open_with_limits(path.as_ref(), DEFAULT_MAX_BYTES, DEFAULT_MAX_FILES)
    }

    fn open_with_limits(path: &Path, max_bytes: u64, max_files: usize) -> io::Result<Self> {
        if max_bytes == 0 || max_files == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "ACP diagnostic limits must be non-zero",
            ));
        }
        prepare_parent(path)?;
        ensure_regular_or_missing(path)?;
        for index in 1..max_files {
            let generation = rotated_path(path, index);
            ensure_regular_or_missing(&generation)?;
            tighten_existing_permissions(&generation)?;
        }
        let file = open_secure_append(path)?;
        Ok(Self {
            writer: Some(Arc::new(Mutex::new(RotatingFile {
                path: path.to_path_buf(),
                file: Some(file),
                max_bytes,
                max_files,
            }))),
        })
    }

    /// Append one controlled diagnostic event, discarding file I/O failures.
    ///
    /// Callers must not pass prompt or response bodies. Known credential and
    /// key patterns are redacted as a defense in depth before serialization.
    pub fn record(&self, level: &str, event: &str, detail: &str) {
        let Some(writer) = &self.writer else {
            return;
        };
        let redacted = polkagent_telemetry::redact_string(detail);
        let detail = truncate_utf8(&redacted, MAX_DETAIL_BYTES);
        let entry = serde_json::json!({
            "timestamp": chrono::Utc::now().to_rfc3339(),
            "level": level,
            "event": event,
            "detail": detail,
            "pid": std::process::id(),
        });
        let Ok(mut line) = serde_json::to_vec(&entry) else {
            return;
        };
        line.push(b'\n');
        if let Ok(mut writer) = writer.lock() {
            let _ = writer.write(&line);
        }
    }
}

struct RotatingFile {
    path: PathBuf,
    file: Option<File>,
    max_bytes: u64,
    max_files: usize,
}

impl RotatingFile {
    fn write(&mut self, line: &[u8]) -> io::Result<()> {
        let current_bytes = self
            .file
            .as_ref()
            .ok_or_else(|| io::Error::other("ACP diagnostic file is unavailable"))?
            .metadata()?
            .len();
        let line_bytes = u64::try_from(line.len()).unwrap_or(u64::MAX);
        if current_bytes > 0 && current_bytes.saturating_add(line_bytes) > self.max_bytes {
            self.rotate()?;
        }
        let file = self
            .file
            .as_mut()
            .ok_or_else(|| io::Error::other("ACP diagnostic file is unavailable"))?;
        file.write_all(line)?;
        file.flush()
    }

    fn rotate(&mut self) -> io::Result<()> {
        if let Some(mut file) = self.file.take() {
            file.flush()?;
        }
        if self.max_files == 1 {
            remove_regular_file_if_present(&self.path)?;
            self.file = Some(open_secure_append(&self.path)?);
            return Ok(());
        }
        let oldest = rotated_path(&self.path, self.max_files - 1);
        remove_regular_file_if_present(&oldest)?;
        for index in (1..self.max_files - 1).rev() {
            let source = rotated_path(&self.path, index);
            if source.exists() {
                ensure_regular_or_missing(&source)?;
                fs::rename(&source, rotated_path(&self.path, index + 1))?;
            }
        }
        if self.path.exists() {
            ensure_regular_or_missing(&self.path)?;
            fs::rename(&self.path, rotated_path(&self.path, 1))?;
        }
        self.file = Some(open_secure_append(&self.path)?);
        Ok(())
    }
}

fn prepare_parent(path: &Path) -> io::Result<()> {
    let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    else {
        return Ok(());
    };
    let existed = parent.exists();
    fs::create_dir_all(parent)?;
    #[cfg(unix)]
    if !existed {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn ensure_regular_or_missing(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() => Ok(()),
        Ok(_) => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "refusing non-regular ACP diagnostic path: {}",
                path.display()
            ),
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn remove_regular_file_if_present(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() => fs::remove_file(path),
        Ok(_) => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "refusing non-regular ACP diagnostic path: {}",
                path.display()
            ),
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn rotated_path(path: &Path, index: usize) -> PathBuf {
    let mut value = OsString::from(path.as_os_str());
    value.push(format!(".{index}"));
    value.into()
}

#[cfg(unix)]
fn open_secure_append(path: &Path) -> io::Result<File> {
    use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};

    let mut options = OpenOptions::new();
    options
        .create(true)
        .append(true)
        .mode(0o600)
        .custom_flags(nix::fcntl::OFlag::O_NOFOLLOW.bits());
    let file = options.open(path)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(file)
}

#[cfg(not(unix))]
fn open_secure_append(path: &Path) -> io::Result<File> {
    OpenOptions::new().create(true).append(true).open(path)
}

fn tighten_existing_permissions(path: &Path) -> io::Result<()> {
    if !path.exists() {
        return Ok(());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

fn truncate_utf8(value: &str, max_bytes: usize) -> &str {
    if value.len() <= max_bytes {
        return value;
    }
    let mut end = max_bytes;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::*;

    #[test]
    fn redacts_known_secrets_and_rotates_with_bounded_retention() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let path = temp.path().join("private/acp.jsonl");
        let diagnostics =
            AcpDiagnostics::open_with_limits(&path, 240, 3).expect("open diagnostics");

        for index in 0..12 {
            diagnostics.record(
                "error",
                "acp.test",
                &format!("entry {index}: sk-acpDiagnosticFixture123"),
            );
        }

        assert!(path.exists());
        assert!(rotated_path(&path, 1).exists());
        assert!(rotated_path(&path, 2).exists());
        assert!(!rotated_path(&path, 3).exists());
        for candidate in [path.clone(), rotated_path(&path, 1), rotated_path(&path, 2)] {
            let content = fs::read_to_string(candidate).expect("read diagnostic generation");
            assert!(!content.contains("sk-acpDiagnosticFixture123"));
            assert!(content.contains("sk-***REDACTED***"));
            for line in content.lines() {
                let parsed: serde_json::Value =
                    serde_json::from_str(line).expect("valid diagnostic JSONL");
                assert_eq!(parsed["event"], "acp.test");
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn creates_restrictive_file_and_new_parent_permissions() {
        use std::os::unix::fs::PermissionsExt as _;

        let temp = tempfile::tempdir().expect("temporary directory");
        let parent = temp.path().join("private");
        let path = parent.join("acp.jsonl");
        AcpDiagnostics::open(&path).expect("open diagnostics");

        let file_mode = fs::metadata(path)
            .expect("file metadata")
            .permissions()
            .mode()
            & 0o777;
        let dir_mode = fs::metadata(parent)
            .expect("directory metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(file_mode, 0o600);
        assert_eq!(dir_mode, 0o700);
    }

    #[cfg(unix)]
    #[test]
    fn refuses_a_symlink_destination() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().expect("temporary directory");
        let target = temp.path().join("target");
        fs::write(&target, "leave unchanged").expect("write target");
        let link = temp.path().join("acp.jsonl");
        symlink(&target, &link).expect("create symlink");

        assert!(AcpDiagnostics::open(link).is_err());
        assert_eq!(
            fs::read_to_string(target).expect("read target"),
            "leave unchanged"
        );
    }
}
