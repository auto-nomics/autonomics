use sha2::{Digest as ShaDigest, Sha256};

#[derive(Debug, Clone, Default)]
pub struct TransferDigest {
    hasher: Sha256,
    bytes: u64,
}

impl TransferDigest {
    pub fn update(&mut self, chunk: &[u8]) {
        ShaDigest::update(&mut self.hasher, chunk);
        self.bytes += chunk.len() as u64;
    }

    pub fn finish(self) -> String {
        format!("{:x}", ShaDigest::finalize(self.hasher.clone()))
    }

    pub const fn bytes(&self) -> u64 {
        self.bytes
    }
}

pub fn verify_sha256(expected: &str, actual: &str, bytes: u64) -> Result<(), String> {
    let expected = expected.trim().to_ascii_lowercase();
    let actual = actual.trim().to_ascii_lowercase();
    if expected.len() != 64 || !expected.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("expected_sha256 must be exactly 64 hexadecimal characters".to_string());
    }
    if !expected.eq(&actual) {
        return Err(format!("sha256 mismatch after downloading {bytes} bytes"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn computes_and_verifies_sha256() {
        let mut digest = TransferDigest::default();
        digest.update(b"hello ");
        digest.update(b"world");
        let byte_count = digest.bytes();
        let actual = digest.finish();
        assert_eq!(
            actual,
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
        assert_eq!(byte_count, 11);
        assert!(verify_sha256(&actual, &actual, 11).is_ok());
        assert!(verify_sha256("bad", &actual, 11).is_err());
    }
}
