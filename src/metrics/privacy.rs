//! Privacy boundary for developer query telemetry.

use std::fs::OpenOptions;
use std::io::{ErrorKind as IoErrorKind, Read, Write};
use std::path::{Path, PathBuf};

use ring::hmac;
use ring::rand::{SecureRandom, SystemRandom};

use crate::error::{Error, ErrorKind, Result};

pub const TELEMETRY_KEY_FILE: &str = "telemetry.key";
const KEY_LEN: usize = 32;

pub fn key_path(root: &Path) -> PathBuf {
    root.join(".mdkb").join(TELEMETRY_KEY_FILE)
}

pub fn key_exists(root: &Path) -> bool {
    key_path(root).is_file()
}

/// Load the repository-local telemetry key, creating it atomically when absent.
pub fn load_or_create_key(root: &Path) -> Result<Vec<u8>> {
    let path = key_path(root);
    if let Ok(key) = read_key(&path) {
        return Ok(key);
    }

    let parent = path.parent().expect("telemetry key always has a parent");
    std::fs::create_dir_all(parent).map_err(|error| io_error(parent, "create directory", error))?;

    let mut key = vec![0_u8; KEY_LEN];
    SystemRandom::new()
        .fill(&mut key)
        .map_err(|_| Error::other("operating-system random generator failed"))?;

    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }

    match options.open(&path) {
        Ok(mut file) => {
            file.write_all(&key)
                .map_err(|error| io_error(&path, "write telemetry key", error))?;
            file.sync_all()
                .map_err(|error| io_error(&path, "sync telemetry key", error))?;
            Ok(key)
        }
        Err(error) if error.kind() == IoErrorKind::AlreadyExists => read_key(&path),
        Err(error) => Err(io_error(&path, "create telemetry key", error)),
    }
}

/// HMAC a normalized query so repeated searches can be correlated inside one
/// repository without making predictable prompts reversible by dictionary.
pub fn hash_query(root: &Path, query: &str) -> Result<String> {
    let key = load_or_create_key(root)?;
    Ok(hash_query_with_key(&key, query))
}

fn hash_query_with_key(key: &[u8], query: &str) -> String {
    use std::fmt::Write as _;

    let normalized = query
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let key = hmac::Key::new(hmac::HMAC_SHA256, key);
    let tag = hmac::sign(&key, normalized.as_bytes());
    tag.as_ref().iter().fold(
        String::with_capacity(tag.as_ref().len() * 2),
        |mut output, byte| {
            write!(output, "{byte:02x}").expect("writing to a String cannot fail");
            output
        },
    )
}

fn read_key(path: &Path) -> Result<Vec<u8>> {
    let mut key = Vec::new();
    let mut file =
        std::fs::File::open(path).map_err(|error| io_error(path, "read telemetry key", error))?;
    file.read_to_end(&mut key)
        .map_err(|error| io_error(path, "read telemetry key", error))?;
    if key.len() != KEY_LEN {
        return Err(Error::other(format!(
            "invalid telemetry key at {}: expected {KEY_LEN} bytes, found {}",
            path.display(),
            key.len()
        )));
    }
    Ok(key)
}

fn io_error(path: &Path, operation: &str, error: std::io::Error) -> Error {
    Error::from(ErrorKind::Io {
        path: path.to_path_buf(),
        operation: format!("{operation}: {error}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hmac_normalizes_queries_and_separates_keys() {
        let a = hash_query_with_key(&[1; KEY_LEN], "  Configure   AUTH ");
        let b = hash_query_with_key(&[1; KEY_LEN], "configure auth");
        let other_key = hash_query_with_key(&[2; KEY_LEN], "configure auth");
        assert_eq!(a, b);
        assert_ne!(a, other_key);
        assert_eq!(a.len(), 64);
    }

    #[test]
    fn key_creation_is_stable_and_private() {
        let root = tempfile::tempdir().unwrap();
        let first = load_or_create_key(root.path()).unwrap();
        let second = load_or_create_key(root.path()).unwrap();
        assert_eq!(first, second);
        assert_eq!(first.len(), KEY_LEN);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(key_path(root.path()))
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600);
        }
    }
}
