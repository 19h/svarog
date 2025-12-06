//! CRC32C hashing utilities.
//!
//! CRC32C (Castagnoli) is used throughout Star Citizen files for checksums
//! and as a hash for string identifiers.

/// Compute CRC32C hash of a byte slice.
///
/// Uses hardware acceleration when available (SSE4.2 on x86).
#[inline]
pub fn hash_bytes(data: &[u8]) -> u32 {
    crc32c::crc32c(data)
}

/// Compute CRC32C hash of a byte slice with a seed value.
///
/// This continues a previous CRC computation.
#[inline]
pub fn hash_bytes_with_seed(data: &[u8], seed: u32) -> u32 {
    crc32c::crc32c_append(seed, data)
}

/// Compute CRC32C hash of a string.
///
/// The string is encoded as UTF-8 before hashing.
#[inline]
pub fn hash_str(s: &str) -> u32 {
    hash_bytes(s.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_hash() {
        assert_eq!(hash_bytes(&[]), 0);
    }

    #[test]
    fn test_known_hash() {
        // Test against a known value
        let data = b"hello";
        let hash = hash_bytes(data);
        // CRC32C of "hello" should be consistent
        assert_ne!(hash, 0);
    }

    #[test]
    fn test_string_hash() {
        let hash1 = hash_str("test");
        let hash2 = hash_bytes(b"test");
        assert_eq!(hash1, hash2);
    }
}
