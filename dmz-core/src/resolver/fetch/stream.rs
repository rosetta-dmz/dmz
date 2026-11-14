//! Streaming network transport with constant O(1) memory overhead and exponential backoff retry.

use crate::diagnostics::CoreMetrics;
use crate::resolver::ResolverError;
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Maximum retry attempts for transient network or CDN stream drops.
pub const MAX_DOWNLOAD_ATTEMPTS: usize = 3;

/// Chunk buffer size (128 KB) ensuring constant memory overhead regardless of artifact size.
pub const STREAM_CHUNK_SIZE: usize = 128 * 1024;

/// Maximum backoff delay honored from `Retry-After` headers (seconds).
const MAX_RETRY_AFTER_SECS: u64 = 30;

fn is_retryable_status(status: reqwest::StatusCode) -> bool {
    matches!(status.as_u16(), 408 | 429 | 500 | 502 | 503 | 504)
}

fn parse_retry_after_secs(headers: &reqwest::header::HeaderMap) -> Option<u64> {
    headers
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.trim().parse::<u64>().ok())
}

static FETCH_JITTER_SEQ: AtomicU64 = AtomicU64::new(1);

fn compute_jitter_ms(attempt: usize) -> u64 {
    let seq = FETCH_JITTER_SEQ.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::from(d.subsec_nanos()) ^ d.as_secs().wrapping_mul(0x517c_c1b7_2722_0a95))
        .unwrap_or(0);
    let attempt_u64 = u64::try_from(attempt).unwrap_or(1);
    let mixed = (nanos ^ seq.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ attempt_u64)
        .wrapping_mul(0xBF58_476D_1CE4_E5B9);
    (mixed ^ (mixed >> 31)) % 250
}

fn compute_backoff_ms(attempt: usize, retry_after_secs: Option<u64>) -> u64 {
    if let Some(secs) = retry_after_secs {
        return secs.min(MAX_RETRY_AFTER_SECS).saturating_mul(1000);
    }
    let shift = u32::try_from(attempt.saturating_sub(1)).unwrap_or(5).min(5);
    let exp = 500u64.saturating_mul(1u64 << shift);
    let jitter = compute_jitter_ms(attempt);
    exp.saturating_add(jitter)
}

/// Streams an artifact from `download_url` directly to `tmp_path`, computing SHA256 on the fly,
/// and atomically renames to `artifact_path` on successful completion.
pub fn stream_download_with_retry(
    client: &reqwest::blocking::Client,
    download_url: &str,
    tmp_path: &Path,
    artifact_path: &Path,
    pkg_name: &str,
) -> Result<String, ResolverError> {
    let mut last_err = String::new();
    let mut computed_hash = String::new();

    for attempt in 1..=MAX_DOWNLOAD_ATTEMPTS {
        let mut response = match client.get(download_url).send() {
            Ok(resp) => {
                if !resp.status().is_success() {
                    let status = resp.status();
                    let err_msg = format!(
                        "Failed to download {}: server returned status {}",
                        download_url, status
                    );
                    if is_retryable_status(status) && attempt < MAX_DOWNLOAD_ATTEMPTS {
                        last_err = err_msg;
                        let retry_after = parse_retry_after_secs(resp.headers());
                        let delay_ms = compute_backoff_ms(attempt, retry_after);
                        CoreMetrics::record_fetch_retry();
                        tracing::warn!(
                            name = %pkg_name,
                            attempt,
                            status = %status,
                            delay_ms,
                            "Transient HTTP status received; backing off before retry"
                        );
                        std::thread::sleep(std::time::Duration::from_millis(delay_ms));
                        continue;
                    }
                    return Err(ResolverError::Network(err_msg));
                }
                resp
            }
            Err(e) => {
                last_err = format!("HTTP request failed for {}: {}", download_url, e);
                if attempt < MAX_DOWNLOAD_ATTEMPTS {
                    let delay_ms = compute_backoff_ms(attempt, None);
                    CoreMetrics::record_fetch_retry();
                    std::thread::sleep(std::time::Duration::from_millis(delay_ms));
                    continue;
                }
                return Err(ResolverError::Network(last_err));
            }
        };

        let mut out_file = match std::fs::File::create(tmp_path) {
            Ok(f) => f,
            Err(e) => {
                return Err(ResolverError::Io(format!(
                    "Failed to create temporary file {:?}: {}",
                    tmp_path, e
                )));
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
                let delay_ms = compute_backoff_ms(attempt, None);
                CoreMetrics::record_fetch_retry();
                tracing::warn!(
                    name = %pkg_name,
                    attempt,
                    delay_ms,
                    "Stream download interrupted ({}), retrying...",
                    last_err
                );
                std::thread::sleep(std::time::Duration::from_millis(delay_ms));
                continue;
            }
            return Err(ResolverError::Network(last_err));
        }

        if let Err(e) = out_file.flush() {
            let _ = std::fs::remove_file(tmp_path);
            return Err(ResolverError::Io(format!(
                "Failed to flush {:?}: {}",
                tmp_path, e
            )));
        }
        drop(out_file);

        crate::install::fast_clone::FastClone::prepare_destination(artifact_path).map_err(|e| {
            let _ = std::fs::remove_file(tmp_path);
            ResolverError::Io(format!(
                "Failed to prepare destination {:?}: {}",
                artifact_path, e
            ))
        })?;

        if let Err(e) = std::fs::rename(tmp_path, artifact_path) {
            let _ = std::fs::remove_file(tmp_path);
            return Err(ResolverError::Io(format!(
                "Failed to rename {:?} to {:?}: {}",
                tmp_path, artifact_path, e
            )));
        }

        computed_hash = crate::archive::exporter::hashing::format_sha256_hex(&hasher.finalize());
        break;
    }

    if computed_hash.is_empty() {
        return Err(ResolverError::Network(format!(
            "Failed to download {} after {} attempts: {}",
            pkg_name, MAX_DOWNLOAD_ATTEMPTS, last_err
        )));
    }

    Ok(computed_hash)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use tempfile::tempdir;

    #[test]
    fn test_compute_backoff_exponential_and_retry_after() {
        let d1 = compute_backoff_ms(1, None);
        let d2 = compute_backoff_ms(2, None);
        assert!((500..750).contains(&d1));
        assert!((1000..1250).contains(&d2));
        assert_eq!(compute_backoff_ms(1, Some(2)), 2000);
        assert_eq!(compute_backoff_ms(1, Some(120)), 30_000);

        let samples: std::collections::HashSet<u64> =
            (0..16).map(|_| compute_backoff_ms(1, None)).collect();
        assert!(
            samples.len() > 1,
            "jitter must decorrelate repeated retries across workers"
        );
    }

    #[test]
    fn test_stream_download_retries_transient_503_and_429() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();

        let server = std::thread::spawn(move || {
            for attempt in 0..3 {
                let (mut stream, _) = listener.accept().unwrap();
                let mut req_buf = [0u8; 1024];
                let _ = stream.read(&mut req_buf);
                if attempt == 0 {
                    let _ = stream.write_all(
                        b"HTTP/1.1 429 Too Many Requests\r\nRetry-After: 0\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    );
                } else if attempt == 1 {
                    let _ = stream.write_all(
                        b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    );
                } else {
                    let body = b"hello-dmz-payload";
                    let header = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = stream.write_all(header.as_bytes());
                    let _ = stream.write_all(body);
                }
            }
        });

        let dir = tempdir().unwrap();
        let tmp_path = dir.path().join("pkg.tmp");
        let artifact_path = dir.path().join("pkg.archive");
        let client = reqwest::blocking::Client::builder().build().unwrap();
        let url = format!("http://{}/pkg.tar.gz", addr);

        let before = CoreMetrics::snapshot().fetch_retries;
        let hash = stream_download_with_retry(&client, &url, &tmp_path, &artifact_path, "test-pkg")
            .unwrap();
        let after = CoreMetrics::snapshot().fetch_retries;

        assert!(after >= before + 2);
        assert_eq!(std::fs::read(&artifact_path).unwrap(), b"hello-dmz-payload");
        assert!(!hash.is_empty());
        server.join().unwrap();
    }
}
