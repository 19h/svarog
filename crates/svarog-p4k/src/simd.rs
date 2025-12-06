//! SIMD-accelerated operations for P4K archive processing.
//!
//! This module provides highly optimized routines for:
//! - Finding null padding at end of files
//! - Searching for ZIP signatures
//! - Bulk memory operations

use std::arch::x86_64::*;

/// Find the end of actual content by scanning backwards for non-zero bytes.
/// Uses SIMD for massive speedup on the ~500MB null padding in P4K files.
///
/// # Safety
/// This function uses SIMD intrinsics and requires proper CPU feature detection.
#[inline]
pub fn find_content_end(data: &[u8]) -> usize {
    // Use SIMD if available, otherwise fall back to scalar
    #[cfg(target_arch = "x86_64")]
    {
        if is_x86_feature_detected!("avx2") {
            return unsafe { find_content_end_avx2(data) };
        }
        if is_x86_feature_detected!("sse2") {
            return unsafe { find_content_end_sse2(data) };
        }
    }
    find_content_end_scalar(data)
}

/// AVX2 implementation - processes 32 bytes at a time
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn find_content_end_avx2(data: &[u8]) -> usize {
    const CHUNK_SIZE: usize = 32;

    if data.len() < CHUNK_SIZE {
        return find_content_end_scalar(data);
    }

    let zero = _mm256_setzero_si256();
    let mut pos = data.len();

    // Process 32-byte aligned chunks from the end
    while pos >= CHUNK_SIZE {
        pos -= CHUNK_SIZE;

        // Load 32 bytes (unaligned load is fine on modern CPUs)
        let chunk = _mm256_loadu_si256(data.as_ptr().add(pos) as *const __m256i);

        // Compare with zero vector
        let cmp = _mm256_cmpeq_epi8(chunk, zero);
        let mask = _mm256_movemask_epi8(cmp) as u32;

        // If not all zeros, find the last non-zero byte
        if mask != 0xFFFFFFFF {
            // Some bytes are non-zero - find the highest one
            let non_zero_mask = !mask;
            let highest_bit = 31 - non_zero_mask.leading_zeros() as usize;
            return pos + highest_bit + 1;
        }
    }

    // Handle remaining bytes
    if pos > 0 {
        for i in (0..pos).rev() {
            if data[i] != 0 {
                return i + 1;
            }
        }
    }

    data.len()
}

/// SSE2 implementation - processes 16 bytes at a time
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse2")]
unsafe fn find_content_end_sse2(data: &[u8]) -> usize {
    const CHUNK_SIZE: usize = 16;

    if data.len() < CHUNK_SIZE {
        return find_content_end_scalar(data);
    }

    let zero = _mm_setzero_si128();
    let mut pos = data.len();

    // Process 16-byte chunks from the end
    while pos >= CHUNK_SIZE {
        pos -= CHUNK_SIZE;

        let chunk = _mm_loadu_si128(data.as_ptr().add(pos) as *const __m128i);
        let cmp = _mm_cmpeq_epi8(chunk, zero);
        let mask = _mm_movemask_epi8(cmp) as u32;

        if mask != 0xFFFF {
            let non_zero_mask = (!mask) & 0xFFFF;
            let highest_bit = 15 - (non_zero_mask.leading_zeros() - 16) as usize;
            return pos + highest_bit + 1;
        }
    }

    // Handle remaining bytes
    if pos > 0 {
        for i in (0..pos).rev() {
            if data[i] != 0 {
                return i + 1;
            }
        }
    }

    data.len()
}

/// Scalar fallback implementation
#[inline]
fn find_content_end_scalar(data: &[u8]) -> usize {
    const CHUNK_SIZE: usize = 64;

    let mut pos = data.len();
    while pos > 0 {
        let chunk_start = pos.saturating_sub(CHUNK_SIZE);
        let chunk = &data[chunk_start..pos];

        // Use memchr for fast byte scanning
        if let Some(last_nonzero) = memchr::memrchr_iter(0, chunk)
            .next()
            .and_then(|first_zero| {
                // Found a zero, but we need the last non-zero
                chunk.iter().rposition(|&b| b != 0)
            })
            .or_else(|| {
                // No zeros found, check if chunk has any content
                if chunk.iter().any(|&b| b != 0) {
                    chunk.iter().rposition(|&b| b != 0)
                } else {
                    None
                }
            })
        {
            return chunk_start + last_nonzero + 1;
        }

        // Entire chunk is zero
        if chunk.iter().all(|&b| b == 0) {
            pos = chunk_start;
            continue;
        }

        // Find exact position
        for i in (0..chunk.len()).rev() {
            if chunk[i] != 0 {
                return chunk_start + i + 1;
            }
        }

        pos = chunk_start;
    }

    data.len()
}

/// SIMD-accelerated search for EOCD signature (0x06054b50)
/// Searches backwards through the buffer.
#[inline]
pub fn find_eocd_signature(data: &[u8], search_start: usize, search_end: usize) -> Option<usize> {
    if search_end <= search_start || search_end > data.len() {
        return None;
    }

    let search_slice = &data[search_start..search_end];

    // Use memchr for fast signature byte search
    // EOCD signature is 0x50 0x4b 0x05 0x06 (little-endian 0x06054b50)
    let mut offset = search_slice.len();

    while offset > 3 {
        // Find the last 0x50 ('P' in "PK")
        match memchr::memrchr(0x50, &search_slice[..offset]) {
            Some(pos) => {
                if pos + 3 < search_slice.len()
                    && search_slice[pos + 1] == 0x4b
                    && search_slice[pos + 2] == 0x05
                    && search_slice[pos + 3] == 0x06
                {
                    return Some(search_start + pos);
                }
                offset = pos;
            }
            None => break,
        }
    }

    None
}

/// Batch process multiple signature searches in parallel chunks
#[cfg(feature = "parallel")]
pub fn find_eocd_parallel(data: &[u8], search_start: usize, search_end: usize) -> Option<usize> {
    use rayon::prelude::*;

    const CHUNK_SIZE: usize = 64 * 1024; // 64KB chunks

    let search_slice = &data[search_start..search_end];
    let chunk_count = (search_slice.len() + CHUNK_SIZE - 1) / CHUNK_SIZE;

    // Search chunks in reverse order (we want the last occurrence)
    (0..chunk_count)
        .into_par_iter()
        .rev()
        .find_map_first(|chunk_idx| {
            let chunk_start = chunk_idx * CHUNK_SIZE;
            let chunk_end = ((chunk_idx + 1) * CHUNK_SIZE).min(search_slice.len());

            // Need to overlap chunks to catch signatures at boundaries
            let overlap_start = chunk_start.saturating_sub(3);
            let chunk = &search_slice[overlap_start..chunk_end];

            find_eocd_signature(chunk, 0, chunk.len())
                .map(|pos| search_start + overlap_start + pos)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_content_end_all_zeros() {
        let data = vec![0u8; 1000];
        assert_eq!(find_content_end(&data), 1000);
    }

    #[test]
    fn test_find_content_end_trailing_zeros() {
        let mut data = vec![0u8; 1000];
        data[499] = 0xFF;
        assert_eq!(find_content_end(&data), 500);
    }

    #[test]
    fn test_find_content_end_no_zeros() {
        let data = vec![0xFFu8; 100];
        assert_eq!(find_content_end(&data), 100);
    }

    #[test]
    fn test_find_eocd_signature() {
        let mut data = vec![0u8; 100];
        // Insert EOCD signature at position 50
        data[50] = 0x50;
        data[51] = 0x4b;
        data[52] = 0x05;
        data[53] = 0x06;

        assert_eq!(find_eocd_signature(&data, 0, 100), Some(50));
    }
}
