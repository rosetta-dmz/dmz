use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;

/// Transparent streaming writer calculating cryptographic SHA-256 in a single pass
/// without intermediate memory buffering or post-compression disk re-reading.
pub struct HashingWriter<W: Write> {
    inner: W,
    hasher: Sha256,
    bytes_written: u64,
}

impl<W: Write> HashingWriter<W> {
    pub fn new(inner: W) -> Self {
        Self {
            inner,
            hasher: Sha256::new(),
            bytes_written: 0,
        }
    }

    pub fn finalize(mut self) -> std::io::Result<(String, u64)> {
        self.inner.flush()?;
        Ok((format!("{:x}", self.hasher.finalize()), self.bytes_written))
    }

    pub fn finalize_hash(self) -> (W, String) {
        let hash = format!("{:x}", self.hasher.finalize());
        (self.inner, hash)
    }

    pub fn bytes_written(&self) -> u64 {
        self.bytes_written
    }
}

impl<W: Write> Write for HashingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.hasher.update(&buf[..n]);
        self.bytes_written += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

pub fn calculate_file_sha256(path: &Path) -> Result<String, std::io::Error> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 65536];

    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }

    Ok(format!("{:x}", hasher.finalize()))
}

