//! P4K decryption using AES-128-CBC.
//!
//! P4K archives use AES-128-CBC encryption with a hardcoded key and zero IV.

use aes::cipher::{BlockDecryptMut, KeyIvInit};
use aes::cipher::generic_array::GenericArray;

type Aes128CbcDec = cbc::Decryptor<aes::Aes128>;

/// The AES-128 key used for P4K encryption.
///
/// This is hardcoded in the game client and is not a secret.
const P4K_AES_KEY: [u8; 16] = [
    0x5E, 0x7A, 0x20, 0x02, 0x30, 0x2E, 0xEB, 0x1A, 0x3B, 0xB6, 0x17, 0xC3, 0x0F, 0xDE, 0x1E, 0x47,
];

/// The initialization vector (all zeros).
const P4K_AES_IV: [u8; 16] = [0u8; 16];

/// Decrypt P4K data in place.
///
/// The data length must be a multiple of the AES block size (16 bytes).
/// Trailing null bytes are removed from the result.
///
/// # Arguments
///
/// * `data` - The encrypted data buffer (modified in place)
///
/// # Returns
///
/// The number of valid bytes after decryption (removing trailing zeros).
pub fn decrypt_in_place(data: &mut [u8]) -> Result<usize, &'static str> {
    if data.is_empty() {
        return Ok(0);
    }

    // Pad to block size if needed (shouldn't happen with valid P4K data)
    if data.len() % 16 != 0 {
        return Err("data length must be a multiple of 16 bytes");
    }

    // Create decryptor
    let key = GenericArray::from_slice(&P4K_AES_KEY);
    let iv = GenericArray::from_slice(&P4K_AES_IV);
    let decryptor = Aes128CbcDec::new(key, iv);

    // Decrypt in place
    decryptor
        .decrypt_padded_mut::<aes::cipher::block_padding::NoPadding>(data)
        .map_err(|_| "decryption failed")?;

    // Find the last non-null byte (trim zero padding)
    let last_non_null = data.iter().rposition(|&b| b != 0).map(|i| i + 1).unwrap_or(0);

    Ok(last_non_null)
}

/// Decrypt P4K data to a new buffer.
///
/// Returns the decrypted data with trailing zeros removed.
pub fn decrypt(data: &[u8]) -> Result<Vec<u8>, &'static str> {
    if data.is_empty() {
        return Ok(Vec::new());
    }

    let mut buffer = data.to_vec();
    let len = decrypt_in_place(&mut buffer)?;
    buffer.truncate(len);
    Ok(buffer)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_decrypt_empty() {
        let result = decrypt(&[]).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_decrypt_invalid_length() {
        let mut data = vec![0u8; 15]; // Not a multiple of 16
        assert!(decrypt_in_place(&mut data).is_err());
    }
}
