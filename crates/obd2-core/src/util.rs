use crate::error::{CoreError, Result};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static COUNTER: AtomicU64 = AtomicU64::new(0);

pub fn decode_hex_strict(value: &str, max_bytes: usize) -> Result<Vec<u8>> {
    if value.len() % 2 != 0 {
        return Err(CoreError::Invalid("odd hex length".into()));
    }
    if value.len() / 2 > max_bytes {
        return Err(CoreError::Invalid(format!(
            "hex value exceeds {max_bytes} bytes"
        )));
    }
    hex::decode(value).map_err(|_| CoreError::Invalid("malformed hex".into()))
}

pub fn encode_hex_upper(bytes: &[u8]) -> String {
    hex::encode_upper(bytes)
}

pub fn hex_bytes(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

pub fn sanitize_segment(value: &str) -> String {
    let mut out: String = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-') {
                ch
            } else {
                '_'
            }
        })
        .collect();
    out.truncate(48);
    if out.is_empty() {
        "unknown".to_string()
    } else {
        out
    }
}

pub fn unique_suffix() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos() as u64)
        .unwrap_or(0);
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{nanos:x}{counter:x}")
}

/// Temp file + fsync + rename + directory fsync. Cleans up the temp file on error.
pub fn write_atomic(dir: &Path, name: &str, bytes: &[u8]) -> Result<PathBuf> {
    fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o700));
    }
    let target = dir.join(name);
    let temp = dir.join(format!(".{name}.{}.tmp", unique_suffix()));
    let result = (|| -> Result<()> {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temp, &target)?;
        #[cfg(unix)]
        {
            File::open(dir)?.sync_all()?;
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result.map(|()| target)
}
