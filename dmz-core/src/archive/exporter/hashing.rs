use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;

/// Formats raw digest bytes as lowercase hexadecimal.
pub fn format_sha256_hex(digest: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        let _ = write!(&mut out, "{byte:02x}");
    }
    out
}

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
        Ok((
            format_sha256_hex(&self.hasher.finalize()),
            self.bytes_written,
        ))
    }

    pub fn finalize_hash(self) -> (W, String) {
        let hash = format_sha256_hex(&self.hasher.finalize());
        (self.inner, hash)
    }

    pub fn bytes_written(&self) -> u64 {
        self.bytes_written
    }
}

impl<W: Write> Write for HashingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = self.inner.write(buf)?;
        if let Some(written) = buf.get(..n) {
            self.hasher.update(written);
        }
        self.bytes_written += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

/// Computes lowercase SHA-256 hex digest by streaming from an open reader.
pub fn calculate_reader_sha256<R: Read>(reader: &mut R) -> Result<String, std::io::Error> {
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 65536];

    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        if let Some(chunk) = buffer.get(..count) {
            hasher.update(chunk);
        }
    }

    Ok(format_sha256_hex(&hasher.finalize()))
}

pub fn calculate_file_sha256(path: &Path) -> Result<String, std::io::Error> {
    let mut file = File::open(path)?;
    calculate_reader_sha256(&mut file)
}
