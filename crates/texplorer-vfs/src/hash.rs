//! Cancellable streaming SHA-256 and MD5 checksum calculation and verification.

use std::fmt::Write as _;
use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use md5::{Digest as _, Md5};
use sha2::Sha256;

/// Computed cryptographic checksums for a local file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileHashes {
    pub sha256: String,
    pub md5: String,
    pub bytes_hashed: u64,
}

/// Computes SHA-256 and MD5 in a single 256 KiB streaming pass with cooperative cancellation.
pub fn compute_file_hashes(path: &Path, cancel: &AtomicBool) -> Result<FileHashes, String> {
    if path.is_dir() {
        return Err("Cannot compute checksums for a directory.".to_string());
    }
    let mut file = File::open(path)
        .map_err(|e| format!("Failed opening \"{}\" for hashing: {e}", path.display()))?;
    let mut sha = Sha256::new();
    let mut md5 = Md5::new();
    let mut buf = vec![0u8; 256 * 1024];
    let mut total = 0u64;

    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err("Hash calculation cancelled.".to_string());
        }
        let n = file
            .read(&mut buf)
            .map_err(|e| format!("Failed reading \"{}\": {e}", path.display()))?;
        if n == 0 {
            break;
        }
        sha.update(&buf[..n]);
        md5.update(&buf[..n]);
        total = total.saturating_add(n as u64);
    }

    Ok(FileHashes {
        sha256: to_hex_lower(&sha.finalize()),
        md5: to_hex_lower(&md5.finalize()),
        bytes_hashed: total,
    })
}

/// Verifies `expected` (case-insensitive, trimmed) against either SHA-256 or MD5.
/// Returns `Some("SHA-256")` or `Some("MD5")` on match, or `None` on mismatch.
pub fn verify_hash_match(hashes: &FileHashes, expected: &str) -> Option<&'static str> {
    let clean: String = expected
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .map(|c| c.to_ascii_lowercase())
        .collect();
    if clean.is_empty() {
        return None;
    }
    if clean == hashes.sha256 {
        Some("SHA-256")
    } else if clean == hashes.md5 {
        Some("MD5")
    } else {
        None
    }
}

fn to_hex_lower(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(&mut out, "{b:02x}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn computes_and_verifies_sha256_and_md5() {
        let tmp =
            std::env::temp_dir().join(format!("texplorer_hash_test_{}.txt", std::process::id()));
        std::fs::write(&tmp, b"hello texplorer").unwrap();

        let cancel = AtomicBool::new(false);
        let hashes = compute_file_hashes(&tmp, &cancel).unwrap();
        assert_eq!(hashes.bytes_hashed, 15);
        assert_eq!(hashes.sha256.len(), 64);
        assert_eq!(hashes.md5.len(), 32);

        let upper_sha = hashes.sha256.to_ascii_uppercase();
        assert_eq!(verify_hash_match(&hashes, &upper_sha), Some("SHA-256"));
        assert_eq!(verify_hash_match(&hashes, &hashes.md5), Some("MD5"));
        assert_eq!(verify_hash_match(&hashes, "deadbeef"), None);

        let _ = std::fs::remove_file(&tmp);
    }
}
