//! Streaming network transport with constant O(1) memory overhead and exponential backoff retry.

use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::Path;

/// Maximum retry attempts for transient network or CDN stream drops.
pub const MAX_DOWNLOAD_ATTEMPTS: usize = 3;

/// Chunk buffer size (128 KB) ensuring constant memory overhead regardless of artifact size.
pub const STREAM_CHUNK_SIZE: usize = 128 * 1024;

/// Streams an artifact from `download_url` directly to `tmp_path`, computing SHA256 on the fly,
/// and atomically renames to `artifact_path` on successful completion.
pub fn stream_download_with_retry(
    client: &reqwest::blocking::Client,
    download_url: &str,
    tmp_path: &Path,
    artifact_path: &Path,
    pkg_name: &str,
) -> Result<String, String> {
    let mut last_err = String::new();
    let mut computed_hash = String::new();

    for attempt in 1..=MAX_DOWNLOAD_ATTEMPTS {
        let mut response = match client.get(download_url).send() {
            Ok(resp) => {
                if !resp.status().is_success() {
                    return Err(format!(
                        "Failed to download {}: server returned status {}",
                        download_url,
                        resp.status()
                    ));
                }
                resp
            }
            Err(e) => {
                last_err = format!("HTTP request failed for {}: {}", download_url, e);
                if attempt < MAX_DOWNLOAD_ATTEMPTS {
                    let delay = (attempt as u64).saturating_mul(1000);
                    std::thread::sleep(std::time::Duration::from_millis(delay));
                    continue;
                }
                return Err(last_err);
            }
        };

        let mut out_file = match std::fs::File::create(tmp_path) {
            Ok(f) => f,
            Err(e) => {
                return Err(format!(
                    "Failed to create temporary file {:?}: {}",
                    tmp_path, e
                ));
            }
        };

        let mut hasher = Sha256::new();
        let mut buffer = [0u8; STREAM_CHUNK_SIZE];
        let mut stream_err = None;

        loop {
            match response.read(&mut buffer) {
                Ok(0) => break,
                Ok(n) => {
                    let chunk = buffer.get(..n).unwrap_or(&buffer);
                    hasher.update(chunk);
                    if let Err(e) = out_file.write_all(chunk) {
                        stream_err = Some(format!("Failed to write to {:?}: {}", tmp_path, e));
                        break;
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => {
                    stream_err = Some(format!(
                        "Failed to read response stream for {}: {}",
                        pkg_name, e
                    ));
                    break;
                }
            }
        }

        if let Some(err) = stream_err {
            let _ = std::fs::remove_file(tmp_path);
            last_err = err;
            if attempt < MAX_DOWNLOAD_ATTEMPTS {
                tracing::warn!(
                    name = %pkg_name,
                    attempt,
                    "Stream download interrupted ({}), retrying...",
                    last_err
                );
                let delay = (attempt as u64).saturating_mul(1000);
                std::thread::sleep(std::time::Duration::from_millis(delay));
                continue;
            }
            return Err(last_err);
        }

        if let Err(e) = out_file.flush() {
            let _ = std::fs::remove_file(tmp_path);
            return Err(format!("Failed to flush {:?}: {}", tmp_path, e));
        }
        drop(out_file);

        if let Err(e) = std::fs::rename(tmp_path, artifact_path) {
            let _ = std::fs::remove_file(tmp_path);
            return Err(format!(
                "Failed to rename {:?} to {:?}: {}",
                tmp_path, artifact_path, e
            ));
        }

        computed_hash = format!("{:x}", hasher.finalize());
        break;
    }

    if computed_hash.is_empty() {
        return Err(format!(
            "Failed to download {} after {} attempts: {}",
            pkg_name, MAX_DOWNLOAD_ATTEMPTS, last_err
        ));
    }

    Ok(computed_hash)
}
