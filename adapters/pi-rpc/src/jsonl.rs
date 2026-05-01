use bytes::BytesMut;
use tokio::io::{AsyncRead, AsyncReadExt};

/// Byte-level `\n`-delimited JSONL reader.
///
/// Pi's RPC mode writes UTF-8 with LF-only delimiters and explicitly avoids
/// Node's readline because it splits on U+2028/U+2029. We do the same on
/// principle — this reader splits only on byte 0x0A.
pub struct JsonlReader<R> {
    reader: R,
    buf: BytesMut,
}

impl<R: AsyncRead + Unpin> JsonlReader<R> {
    pub fn new(reader: R) -> Self {
        Self {
            reader,
            buf: BytesMut::with_capacity(8 * 1024),
        }
    }

    /// Read the next complete JSON line. Returns Ok(None) on EOF.
    pub async fn next_line(&mut self) -> anyhow::Result<Option<String>> {
        loop {
            if let Some(idx) = memchr::memchr(b'\n', &self.buf) {
                let line = self.buf.split_to(idx + 1);
                let s = std::str::from_utf8(&line[..idx])?.to_string();
                if s.is_empty() {
                    continue;
                }
                return Ok(Some(s));
            }

            let mut chunk = [0u8; 4096];
            let n = self.reader.read(&mut chunk).await?;
            if n == 0 {
                if self.buf.is_empty() {
                    return Ok(None);
                }
                // Trailing partial line without newline.
                let s = std::str::from_utf8(&self.buf)?.to_string();
                self.buf.clear();
                return Ok(Some(s));
            }
            self.buf.extend_from_slice(&chunk[..n]);
        }
    }
}
