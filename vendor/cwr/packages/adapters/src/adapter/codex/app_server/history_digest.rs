use super::*;
use sha2::{Digest, Sha256};

pub(super) fn context_digest(value: &Value) -> CwrResult<String> {
    let mut hasher = HashWriter(Sha256::new());
    serde_json::to_writer(&mut hasher, value)?;
    Ok(hasher
        .0
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

struct HashWriter(Sha256);

impl Write for HashWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.update(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
