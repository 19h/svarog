//! SIMD-accelerated utilities for common operations.
//!
//! This module provides cross-platform SIMD acceleration for:
//! - Null-terminated string scanning
//! - Byte pattern searching
//! - Bulk memory operations
//!
//! Architecture support:
//! - x86_64: AVX2, SSE2
//! - aarch64: NEON
//! - Fallback: Scalar implementations

#[cfg(target_arch = "x86_64")]
use std::arch::x86_64::*;

#[cfg(target_arch = "aarch64")]
use std::arch::aarch64::*;

/// Find the first null byte in a slice, returning its index.
/// Uses SIMD acceleration when available.
#[inline]
pub fn find_null(data: &[u8]) -> Option<usize> {
    // Use memchr which already has excellent SIMD implementations
    memchr::memchr(0, data)
}

/// Find the first occurrence of a byte in a slice.
#[inline]
pub fn find_byte(needle: u8, data: &[u8]) -> Option<usize> {
    memchr::memchr(needle, data)
}

/// Find the last occurrence of a byte in a slice.
#[inline]
pub fn find_byte_reverse(needle: u8, data: &[u8]) -> Option<usize> {
    memchr::memrchr(needle, data)
}

/// Find a multi-byte pattern in a slice.
#[inline]
pub fn find_pattern(needle: &[u8], haystack: &[u8]) -> Option<usize> {
    memchr::memmem::find(haystack, needle)
}

/// Find a multi-byte pattern in a slice, searching from the end.
#[inline]
pub fn find_pattern_reverse(needle: &[u8], haystack: &[u8]) -> Option<usize> {
    memchr::memmem::rfind(haystack, needle)
}

/// Check if a slice contains only zero bytes.
/// Uses SIMD acceleration for large slices.
#[inline]
pub fn is_all_zeros(data: &[u8]) -> bool {
    #[cfg(target_arch = "x86_64")]
    {
        if is_x86_feature_detected!("avx2") && data.len() >= 32 {
            return unsafe { is_all_zeros_avx2(data) };
        }
        if is_x86_feature_detected!("sse2") && data.len() >= 16 {
            return unsafe { is_all_zeros_sse2(data) };
        }
    }

    #[cfg(target_arch = "aarch64")]
    {
        if data.len() >= 16 {
            return unsafe { is_all_zeros_neon(data) };
        }
    }

    is_all_zeros_scalar(data)
}

/// Count non-zero bytes in a slice.
/// Uses SIMD acceleration when available.
#[inline]
pub fn count_nonzero(data: &[u8]) -> usize {
    #[cfg(target_arch = "x86_64")]
    {
        if is_x86_feature_detected!("avx2") && data.len() >= 32 {
            return unsafe { count_nonzero_avx2(data) };
        }
    }

    #[cfg(target_arch = "aarch64")]
    {
        if data.len() >= 16 {
            return unsafe { count_nonzero_neon(data) };
        }
    }

    count_nonzero_scalar(data)
}

/// Find the last non-zero byte in a slice.
/// Returns the index + 1 (i.e., the length of content).
#[inline]
pub fn find_content_end(data: &[u8]) -> usize {
    #[cfg(target_arch = "x86_64")]
    {
        if is_x86_feature_detected!("avx2") {
            return unsafe { find_content_end_avx2(data) };
        }
        if is_x86_feature_detected!("sse2") {
            return unsafe { find_content_end_sse2(data) };
        }
    }

    #[cfg(target_arch = "aarch64")]
    {
        return unsafe { find_content_end_neon(data) };
    }

    #[allow(unreachable_code)]
    find_content_end_scalar(data)
}

// ============================================================================
// x86_64 AVX2 implementations
// ============================================================================

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn is_all_zeros_avx2(data: &[u8]) -> bool {
    let zeros = _mm256_setzero_si256();
    let mut i = 0;

    // Process 32 bytes at a time
    while i + 32 <= data.len() {
        let chunk = _mm256_loadu_si256(data.as_ptr().add(i) as *const __m256i);
        let cmp = _mm256_cmpeq_epi8(chunk, zeros);
        if _mm256_movemask_epi8(cmp) != -1i32 {
            return false;
        }
        i += 32;
    }

    // Check remaining bytes
    data[i..].iter().all(|&b| b == 0)
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn count_nonzero_avx2(data: &[u8]) -> usize {
    let zeros = _mm256_setzero_si256();
    let mut count = 0usize;
    let mut i = 0;

    while i + 32 <= data.len() {
        let chunk = _mm256_loadu_si256(data.as_ptr().add(i) as *const __m256i);
        let cmp = _mm256_cmpeq_epi8(chunk, zeros);
        let mask = _mm256_movemask_epi8(cmp) as u32;
        count += (32 - mask.count_ones()) as usize;
        i += 32;
    }

    // Count remaining bytes
    count + data[i..].iter().filter(|&&b| b != 0).count()
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn find_content_end_avx2(data: &[u8]) -> usize {
    let len = data.len();
    if len == 0 {
        return 0;
    }

    let zeros = _mm256_setzero_si256();
    let mut pos = len;

    while pos >= 32 {
        let chunk_start = pos - 32;
        let chunk = _mm256_loadu_si256(data.as_ptr().add(chunk_start) as *const __m256i);
        let cmp = _mm256_cmpeq_epi8(chunk, zeros);
        let mask = _mm256_movemask_epi8(cmp) as u32;

        if mask != 0xFFFFFFFF {
            let leading_zeros = (!mask).leading_zeros() as usize;
            return chunk_start + (32 - leading_zeros);
        }
        pos = chunk_start;
    }

    find_last_nonzero_scalar(&data[..pos])
}

// ============================================================================
// x86_64 SSE2 implementations
// ============================================================================

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse2")]
unsafe fn is_all_zeros_sse2(data: &[u8]) -> bool {
    let zeros = _mm_setzero_si128();
    let mut i = 0;

    while i + 16 <= data.len() {
        let chunk = _mm_loadu_si128(data.as_ptr().add(i) as *const __m128i);
        let cmp = _mm_cmpeq_epi8(chunk, zeros);
        if _mm_movemask_epi8(cmp) != 0xFFFF {
            return false;
        }
        i += 16;
    }

    data[i..].iter().all(|&b| b == 0)
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse2")]
unsafe fn find_content_end_sse2(data: &[u8]) -> usize {
    let len = data.len();
    if len == 0 {
        return 0;
    }

    let zeros = _mm_setzero_si128();
    let mut pos = len;

    while pos >= 16 {
        let chunk_start = pos - 16;
        let chunk = _mm_loadu_si128(data.as_ptr().add(chunk_start) as *const __m128i);
        let cmp = _mm_cmpeq_epi8(chunk, zeros);
        let mask = _mm_movemask_epi8(cmp) as u16;

        if mask != 0xFFFF {
            let leading_zeros = (!mask).leading_zeros() as usize - 16;
            return chunk_start + (16 - leading_zeros);
        }
        pos = chunk_start;
    }

    find_last_nonzero_scalar(&data[..pos])
}

// ============================================================================
// ARM64 NEON implementations
// ============================================================================

#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "neon")]
unsafe fn is_all_zeros_neon(data: &[u8]) -> bool {
    let zeros = vdupq_n_u8(0);
    let mut i = 0;

    while i + 16 <= data.len() {
        let chunk = vld1q_u8(data.as_ptr().add(i));
        let cmp = vceqq_u8(chunk, zeros);

        // Check if all bytes matched (all 0xFF)
        let min = vminvq_u8(cmp);
        if min != 0xFF {
            return false;
        }
        i += 16;
    }

    data[i..].iter().all(|&b| b == 0)
}

#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "neon")]
unsafe fn count_nonzero_neon(data: &[u8]) -> usize {
    let zeros = vdupq_n_u8(0);
    let mut count = 0usize;
    let mut i = 0;

    while i + 16 <= data.len() {
        let chunk = vld1q_u8(data.as_ptr().add(i));
        let cmp = vceqq_u8(chunk, zeros);

        // Count zeros in comparison result (0xFF = zero byte, 0x00 = non-zero)
        // Invert and count
        let non_zero = vmvnq_u8(cmp);
        let sum = vaddvq_u8(non_zero);
        count += (sum / 255) as usize;
        i += 16;
    }

    count + data[i..].iter().filter(|&&b| b != 0).count()
}

#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "neon")]
unsafe fn find_content_end_neon(data: &[u8]) -> usize {
    let len = data.len();
    if len == 0 {
        return 0;
    }

    let zeros = vdupq_n_u8(0);
    let mut pos = len;

    while pos >= 16 {
        let chunk_start = pos - 16;
        let chunk = vld1q_u8(data.as_ptr().add(chunk_start));
        let cmp = vceqq_u8(chunk, zeros);

        // Check if all zeros
        let min = vminvq_u8(cmp);
        if min != 0xFF {
            // Found non-zero, find exact position
            for i in (0..16).rev() {
                if *data.get_unchecked(chunk_start + i) != 0 {
                    return chunk_start + i + 1;
                }
            }
        }
        pos = chunk_start;
    }

    find_last_nonzero_scalar(&data[..pos])
}

// ============================================================================
// Scalar fallback implementations
// ============================================================================

#[inline]
fn is_all_zeros_scalar(data: &[u8]) -> bool {
    // Use u64 reads for better throughput
    let mut i = 0;
    while i + 8 <= data.len() {
        let bytes: [u8; 8] = data[i..i + 8].try_into().unwrap();
        if u64::from_ne_bytes(bytes) != 0 {
            return false;
        }
        i += 8;
    }
    data[i..].iter().all(|&b| b == 0)
}

#[inline]
fn count_nonzero_scalar(data: &[u8]) -> usize {
    data.iter().filter(|&&b| b != 0).count()
}

#[inline]
pub fn find_content_end_scalar(data: &[u8]) -> usize {
    let len = data.len();
    if len == 0 {
        return 0;
    }

    const CHUNK: usize = 64;
    let mut pos = len;

    // Process in chunks of 64 bytes using u64 reads
    while pos >= CHUNK {
        let chunk_start = pos - CHUNK;
        let mut all_zero = true;

        for i in (0..CHUNK).step_by(8) {
            let offset = chunk_start + i;
            let bytes: [u8; 8] = data[offset..offset + 8].try_into().unwrap();
            if u64::from_ne_bytes(bytes) != 0 {
                all_zero = false;
                break;
            }
        }

        if !all_zero {
            return find_last_nonzero_scalar(&data[..pos]);
        }
        pos = chunk_start;
    }

    find_last_nonzero_scalar(&data[..pos])
}

#[inline]
fn find_last_nonzero_scalar(data: &[u8]) -> usize {
    for i in (0..data.len()).rev() {
        if data[i] != 0 {
            return i + 1;
        }
    }
    0
}

/// Compare two slices for equality using SIMD.
#[inline]
pub fn slice_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }

    #[cfg(target_arch = "x86_64")]
    {
        if is_x86_feature_detected!("avx2") && a.len() >= 32 {
            return unsafe { slice_eq_avx2(a, b) };
        }
    }

    #[cfg(target_arch = "aarch64")]
    {
        if a.len() >= 16 {
            return unsafe { slice_eq_neon(a, b) };
        }
    }

    a == b
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn slice_eq_avx2(a: &[u8], b: &[u8]) -> bool {
    let mut i = 0;

    while i + 32 <= a.len() {
        let va = _mm256_loadu_si256(a.as_ptr().add(i) as *const __m256i);
        let vb = _mm256_loadu_si256(b.as_ptr().add(i) as *const __m256i);
        let cmp = _mm256_cmpeq_epi8(va, vb);
        if _mm256_movemask_epi8(cmp) != -1i32 {
            return false;
        }
        i += 32;
    }

    a[i..] == b[i..]
}

#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "neon")]
unsafe fn slice_eq_neon(a: &[u8], b: &[u8]) -> bool {
    let mut i = 0;

    while i + 16 <= a.len() {
        let va = vld1q_u8(a.as_ptr().add(i));
        let vb = vld1q_u8(b.as_ptr().add(i));
        let cmp = vceqq_u8(va, vb);
        let min = vminvq_u8(cmp);
        if min != 0xFF {
            return false;
        }
        i += 16;
    }

    a[i..] == b[i..]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_null() {
        assert_eq!(find_null(b"hello\0world"), Some(5));
        assert_eq!(find_null(b"hello"), None);
        assert_eq!(find_null(b"\0hello"), Some(0));
    }

    #[test]
    fn test_is_all_zeros() {
        assert!(is_all_zeros(&[0u8; 100]));
        assert!(!is_all_zeros(&[0, 0, 1, 0]));
        assert!(is_all_zeros(&[]));
    }

    #[test]
    fn test_count_nonzero() {
        assert_eq!(count_nonzero(&[0, 1, 0, 2, 0, 3]), 3);
        assert_eq!(count_nonzero(&[0u8; 100]), 0);
        assert_eq!(count_nonzero(&[1u8; 100]), 100);
    }

    #[test]
    fn test_find_content_end() {
        let mut data = vec![1u8; 500];
        data.extend(vec![0u8; 500]);
        assert_eq!(find_content_end(&data), 500);

        assert_eq!(find_content_end(&[0u8; 100]), 0);
        assert_eq!(find_content_end(&[1u8; 100]), 100);
    }

    #[test]
    fn test_slice_eq() {
        let a = vec![1u8; 100];
        let b = vec![1u8; 100];
        let c = vec![2u8; 100];

        assert!(slice_eq(&a, &b));
        assert!(!slice_eq(&a, &c));
        assert!(!slice_eq(&a[..50], &b));
    }

    #[test]
    fn test_find_pattern() {
        let data = b"hello world";
        assert_eq!(find_pattern(b"world", data), Some(6));
        assert_eq!(find_pattern(b"foo", data), None);
    }
}
