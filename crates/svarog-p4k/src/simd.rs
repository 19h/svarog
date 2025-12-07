//! SIMD-accelerated operations for P4K file processing.
//!
//! Provides optimized implementations for:
//! - Finding the end of content (skipping null padding)
//! - Searching for EOCD signatures
//!
//! Supports:
//! - x86_64: AVX2 (32 bytes), SSE2 (16 bytes)
//! - aarch64: NEON (16 bytes)
//! - Fallback: Scalar implementation for all other architectures

// x86_64 SIMD imports
#[cfg(target_arch = "x86_64")]
use std::arch::x86_64::*;

// ARM64 NEON imports
#[cfg(target_arch = "aarch64")]
use std::arch::aarch64::*;

/// Chunk size for scalar fallback operations.
const CHUNK_SIZE: usize = 64;

/// Find the end of actual content by scanning backwards for non-zero bytes.
///
/// P4K files often have large amounts of null padding at the end.
/// This function efficiently finds where the real content ends.
///
/// Uses SIMD acceleration when available:
/// - AVX2 on x86_64 (32 bytes at a time)
/// - SSE2 on x86_64 (16 bytes at a time)
/// - NEON on aarch64 (16 bytes at a time)
/// - Scalar fallback (64 bytes at a time)
#[inline]
pub fn find_content_end(data: &[u8]) -> usize {
    #[cfg(target_arch = "x86_64")]
    {
        // Runtime feature detection for x86_64
        if is_x86_feature_detected!("avx2") {
            return unsafe { find_content_end_avx2(data) };
        }
        if is_x86_feature_detected!("sse2") {
            return unsafe { find_content_end_sse2(data) };
        }
        // Fall through to scalar if no SIMD available
        return find_content_end_scalar(data);
    }

    #[cfg(target_arch = "aarch64")]
    {
        // NEON is always available on aarch64
        return unsafe { find_content_end_neon(data) };
    }

    // Fallback for other architectures (wasm, riscv, etc.)
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    {
        find_content_end_scalar(data)
    }
}

/// AVX2 implementation - processes 32 bytes at a time.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn find_content_end_avx2(data: &[u8]) -> usize {
    let len = data.len();
    if len == 0 {
        return 0;
    }

    let zeros = _mm256_setzero_si256();
    let mut pos = len;

    // Process 32-byte aligned chunks from the end
    while pos >= 32 {
        let chunk_start = pos - 32;
        let chunk = _mm256_loadu_si256(data.as_ptr().add(chunk_start) as *const __m256i);
        let cmp = _mm256_cmpeq_epi8(chunk, zeros);
        let mask = _mm256_movemask_epi8(cmp) as u32;

        if mask != 0xFFFFFFFF {
            // Found non-zero byte(s)
            let leading_zeros = (!mask).leading_zeros() as usize;
            return chunk_start + (32 - leading_zeros);
        }
        pos = chunk_start;
    }

    // Handle remaining bytes with scalar
    find_last_nonzero_scalar(&data[..pos])
}

/// SSE2 implementation - processes 16 bytes at a time.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse2")]
unsafe fn find_content_end_sse2(data: &[u8]) -> usize {
    let len = data.len();
    if len == 0 {
        return 0;
    }

    let zeros = _mm_setzero_si128();
    let mut pos = len;

    // Process 16-byte chunks from the end
    while pos >= 16 {
        let chunk_start = pos - 16;
        let chunk = _mm_loadu_si128(data.as_ptr().add(chunk_start) as *const __m128i);
        let cmp = _mm_cmpeq_epi8(chunk, zeros);
        let mask = _mm_movemask_epi8(cmp) as u16;

        if mask != 0xFFFF {
            // Found non-zero byte(s)
            let leading_zeros = (!mask).leading_zeros() as usize - 16; // u16 has 16 extra bits
            return chunk_start + (16 - leading_zeros);
        }
        pos = chunk_start;
    }

    // Handle remaining bytes with scalar
    find_last_nonzero_scalar(&data[..pos])
}

/// NEON implementation for ARM64 - processes 16 bytes at a time.
#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "neon")]
unsafe fn find_content_end_neon(data: &[u8]) -> usize {
    let len = data.len();
    if len == 0 {
        return 0;
    }

    let zeros = vdupq_n_u8(0);
    let mut pos = len;

    // Process 16-byte chunks from the end
    while pos >= 16 {
        let chunk_start = pos - 16;
        let chunk = vld1q_u8(data.as_ptr().add(chunk_start));
        let cmp = vceqq_u8(chunk, zeros);

        // Check if all bytes are zero
        // vmaxvq_u8 returns the maximum value in the vector
        // If all comparisons are 0xFF (equal to zero), max is 0xFF
        // If any comparison is 0x00 (not equal to zero), max could still be 0xFF
        // We need to check if ALL are 0xFF

        // Use horizontal operations to check if all bytes matched zero
        let cmp_low = vget_low_u8(cmp);
        let cmp_high = vget_high_u8(cmp);

        // AND all bytes together - if result is 0xFF, all were zero
        let and1 = vand_u8(cmp_low, cmp_high);
        let and2 = vand_u8(and1, vext_u8(and1, and1, 4));
        let and3 = vand_u8(and2, vext_u8(and2, and2, 2));
        let and4 = vand_u8(and3, vext_u8(and3, and3, 1));
        let all_zero = vget_lane_u8(and4, 0);

        if all_zero != 0xFF {
            // Found non-zero byte(s), scan to find exact position
            for i in (0..16).rev() {
                if *data.get_unchecked(chunk_start + i) != 0 {
                    return chunk_start + i + 1;
                }
            }
        }
        pos = chunk_start;
    }

    // Handle remaining bytes with scalar
    find_last_nonzero_scalar(&data[..pos])
}

/// Scalar implementation - processes 64 bytes at a time.
#[inline]
pub fn find_content_end_scalar(data: &[u8]) -> usize {
    let len = data.len();
    if len == 0 {
        return 0;
    }

    let mut pos = len;

    // Process chunks from the end
    while pos >= CHUNK_SIZE {
        let chunk_start = pos - CHUNK_SIZE;
        let chunk = &data[chunk_start..pos];

        // Check if entire chunk is zeros using u64 reads
        let mut all_zero = true;
        for i in (0..CHUNK_SIZE).step_by(8) {
            let bytes: [u8; 8] = chunk[i..i + 8].try_into().unwrap();
            if u64::from_ne_bytes(bytes) != 0 {
                all_zero = false;
                break;
            }
        }

        if !all_zero {
            // Found non-zero, scan to find exact position
            return find_last_nonzero_scalar(&data[..pos]);
        }
        pos = chunk_start;
    }

    find_last_nonzero_scalar(&data[..pos])
}

/// Find the last non-zero byte position in a slice.
#[inline]
fn find_last_nonzero_scalar(data: &[u8]) -> usize {
    for i in (0..data.len()).rev() {
        if data[i] != 0 {
            return i + 1;
        }
    }
    0
}

/// Search for the EOCD signature (0x06054b50) in the given range.
///
/// Uses SIMD-accelerated search when available.
#[inline]
pub fn find_eocd_signature(data: &[u8], search_start: usize, search_end: usize) -> Option<usize> {
    if search_end <= search_start || search_end > data.len() {
        return None;
    }

    let search_slice = &data[search_start..search_end];

    // EOCD signature bytes: 0x50, 0x4b, 0x05, 0x06 (little-endian 0x06054b50)
    const EOCD_SIG: [u8; 4] = [0x50, 0x4b, 0x05, 0x06];

    // Use memchr for fast byte searching - it uses SIMD internally
    memchr::memmem::rfind(search_slice, &EOCD_SIG).map(|pos| search_start + pos)
}

/// Search for the EOCD64 locator signature (0x07064b50).
#[allow(dead_code)]
#[inline]
pub fn find_eocd64_locator_signature(
    data: &[u8],
    search_start: usize,
    search_end: usize,
) -> Option<usize> {
    if search_end <= search_start || search_end > data.len() {
        return None;
    }

    let search_slice = &data[search_start..search_end];
    const EOCD64_LOC_SIG: [u8; 4] = [0x50, 0x4b, 0x06, 0x07];

    memchr::memmem::rfind(search_slice, &EOCD64_LOC_SIG).map(|pos| search_start + pos)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_content_end_all_zeros() {
        let data = vec![0u8; 1000];
        assert_eq!(find_content_end(&data), 0);
    }

    #[test]
    fn test_find_content_end_no_zeros() {
        let data = vec![1u8; 1000];
        assert_eq!(find_content_end(&data), 1000);
    }

    #[test]
    fn test_find_content_end_trailing_zeros() {
        let mut data = vec![1u8; 500];
        data.extend(vec![0u8; 500]);
        assert_eq!(find_content_end(&data), 500);
    }

    #[test]
    fn test_find_content_end_small() {
        let data = [1, 2, 3, 0, 0];
        assert_eq!(find_content_end(&data), 3);
    }

    #[test]
    fn test_find_content_end_empty() {
        let data: [u8; 0] = [];
        assert_eq!(find_content_end(&data), 0);
    }

    #[test]
    fn test_find_content_end_single_nonzero() {
        let mut data = vec![0u8; 1000];
        data[500] = 1;
        assert_eq!(find_content_end(&data), 501);
    }

    #[test]
    fn test_find_eocd_signature() {
        let mut data = vec![0u8; 1000];
        // Insert EOCD signature at position 500
        data[500] = 0x50;
        data[501] = 0x4b;
        data[502] = 0x05;
        data[503] = 0x06;

        assert_eq!(find_eocd_signature(&data, 0, 1000), Some(500));
        assert_eq!(find_eocd_signature(&data, 0, 500), None);
        assert_eq!(find_eocd_signature(&data, 501, 1000), None);
    }

    #[test]
    fn test_find_eocd64_locator_signature() {
        let mut data = vec![0u8; 1000];
        // Insert EOCD64 locator signature
        data[500] = 0x50;
        data[501] = 0x4b;
        data[502] = 0x06;
        data[503] = 0x07;

        assert_eq!(find_eocd64_locator_signature(&data, 0, 1000), Some(500));
    }

    #[test]
    fn test_scalar_consistency() {
        // Test that scalar gives same results
        let mut data = vec![0u8; 200];
        data[100] = 42;
        data[150] = 1;

        let scalar_result = find_content_end_scalar(&data);
        let simd_result = find_content_end(&data);

        assert_eq!(scalar_result, simd_result);
        assert_eq!(simd_result, 151);
    }
}
