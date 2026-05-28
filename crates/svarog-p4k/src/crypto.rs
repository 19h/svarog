//! P4K decryption/encryption using AES-128-CBC.
//!
//! The CigDataPatcher helpers are named `AES128_ECB_*`, but their
//! bodies apply the standard CBC XOR chain around each ECB block. The
//! first previous block is all zeroes, so this is AES-128-CBC with a
//! zero IV.

use aes::cipher::generic_array::GenericArray;
#[cfg(test)]
use aes::cipher::BlockDecryptMut;
use aes::cipher::{BlockDecrypt, BlockEncrypt, BlockEncryptMut, KeyInit, KeyIvInit};
use sha2::{Digest, Sha256};
use std::io::{self, Read, Write};

#[cfg(test)]
type Aes128CbcDec = cbc::Decryptor<aes::Aes128>;
type Aes128CbcEnc = cbc::Encryptor<aes::Aes128>;

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
/// The caller is responsible for applying the archive metadata's
/// uncompressed size to discard encryption padding.
///
/// # Arguments
///
/// * `data` - The encrypted data buffer (modified in place)
///
/// # Returns
///
/// The number of decrypted bytes.
#[cfg(test)]
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

    Ok(data.len())
}

/// Decrypt P4K data to a new buffer.
///
/// Returns the full decrypted data including any zero padding.
#[cfg(test)]
pub fn decrypt(data: &[u8]) -> Result<Vec<u8>, &'static str> {
    if data.is_empty() {
        return Ok(Vec::new());
    }

    let mut buffer = data.to_vec();
    let len = decrypt_in_place(&mut buffer)?;
    buffer.truncate(len);
    Ok(buffer)
}

pub(crate) struct DecryptReader<R> {
    inner: R,
    cipher: aes::Aes128,
    previous: [u8; 16],
    pending: [u8; 16],
    pending_pos: usize,
    pending_len: usize,
    eof: bool,
}

impl<R: Read> DecryptReader<R> {
    pub(crate) fn new(inner: R) -> Self {
        Self {
            inner,
            cipher: aes::Aes128::new(GenericArray::from_slice(&P4K_AES_KEY)),
            previous: P4K_AES_IV,
            pending: [0; 16],
            pending_pos: 0,
            pending_len: 0,
            eof: false,
        }
    }

    fn refill(&mut self) -> io::Result<()> {
        if self.eof {
            return Ok(());
        }

        let mut ciphertext = [0u8; 16];
        let mut filled = 0usize;
        while filled < ciphertext.len() {
            let read = self.inner.read(&mut ciphertext[filled..])?;
            if read == 0 {
                if filled == 0 {
                    self.eof = true;
                    return Ok(());
                }
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "encrypted data length must be a multiple of 16 bytes",
                ));
            }
            filled += read;
        }

        let mut block = GenericArray::clone_from_slice(&ciphertext);
        self.cipher.decrypt_block(&mut block);
        for (out, (plain, prev)) in self
            .pending
            .iter_mut()
            .zip(block.iter().zip(self.previous.iter()))
        {
            *out = plain ^ prev;
        }
        self.previous = ciphertext;
        self.pending_pos = 0;
        self.pending_len = 16;
        Ok(())
    }
}

impl<R: Read> Read for DecryptReader<R> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if out.is_empty() {
            return Ok(0);
        }

        let mut copied = 0usize;
        while copied < out.len() {
            if self.pending_pos == self.pending_len {
                self.pending_pos = 0;
                self.pending_len = 0;
                self.refill()?;
                if self.pending_len == 0 {
                    break;
                }
            }

            let available = self.pending_len - self.pending_pos;
            let take = available.min(out.len() - copied);
            out[copied..copied + take]
                .copy_from_slice(&self.pending[self.pending_pos..self.pending_pos + take]);
            self.pending_pos += take;
            copied += take;
        }

        Ok(copied)
    }
}

/// Encrypt P4K payload bytes to a new buffer.
///
/// P4K encryption uses AES-128-CBC with a zero IV and zero padding to
/// the next AES block. Readers must use the stored uncompressed size to
/// distinguish real trailing zero bytes from encryption padding.
pub fn encrypt(data: &[u8]) -> Result<Vec<u8>, &'static str> {
    if data.is_empty() {
        return Ok(Vec::new());
    }

    let padded_len = data
        .len()
        .checked_add(15)
        .map(|v| v & !15)
        .ok_or("encrypted data length overflow")?;
    let mut buffer = vec![0u8; padded_len];
    buffer[..data.len()].copy_from_slice(data);

    let key = GenericArray::from_slice(&P4K_AES_KEY);
    let iv = GenericArray::from_slice(&P4K_AES_IV);
    let encryptor = Aes128CbcEnc::new(key, iv);
    encryptor
        .encrypt_padded_mut::<aes::cipher::block_padding::NoPadding>(&mut buffer, padded_len)
        .map_err(|_| "encryption failed")?;

    Ok(buffer)
}

pub(crate) fn encrypt_reader_to_writer<R: Read, W: Write>(
    reader: &mut R,
    writer: &mut W,
) -> io::Result<u64> {
    const BUFFER_SIZE: usize = 128 * 1024;

    let cipher = aes::Aes128::new(GenericArray::from_slice(&P4K_AES_KEY));
    let mut previous = P4K_AES_IV;
    let mut buffer = [0u8; BUFFER_SIZE];
    let mut partial = [0u8; 16];
    let mut partial_len = 0usize;
    let mut written = 0u64;

    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }

        let mut chunk = &buffer[..read];
        if partial_len > 0 {
            let take = (16 - partial_len).min(chunk.len());
            partial[partial_len..partial_len + take].copy_from_slice(&chunk[..take]);
            partial_len += take;
            chunk = &chunk[take..];

            if partial_len == 16 {
                encrypt_block(&cipher, &mut previous, &partial, writer)?;
                written += 16;
                partial = [0u8; 16];
                partial_len = 0;
            }
        }

        while chunk.len() >= 16 {
            let block: &[u8; 16] = chunk[..16].try_into().expect("slice length checked");
            encrypt_block(&cipher, &mut previous, block, writer)?;
            written += 16;
            chunk = &chunk[16..];
        }

        if !chunk.is_empty() {
            partial[..chunk.len()].copy_from_slice(chunk);
            partial_len = chunk.len();
        }
    }

    if partial_len > 0 {
        partial[partial_len..].fill(0);
        encrypt_block(&cipher, &mut previous, &partial, writer)?;
        written += 16;
    }

    Ok(written)
}

pub(crate) struct EncryptWriter<W> {
    inner: W,
    cipher: aes::Aes128,
    previous: [u8; 16],
    partial: [u8; 16],
    partial_len: usize,
    written: u64,
    finished: bool,
}

impl<W: Write> EncryptWriter<W> {
    pub(crate) fn new(inner: W) -> Self {
        Self {
            inner,
            cipher: aes::Aes128::new(GenericArray::from_slice(&P4K_AES_KEY)),
            previous: P4K_AES_IV,
            partial: [0; 16],
            partial_len: 0,
            written: 0,
            finished: false,
        }
    }

    pub(crate) fn finish(mut self) -> io::Result<(W, u64)> {
        if !self.finished {
            if self.partial_len > 0 {
                self.partial[self.partial_len..].fill(0);
                encrypt_block(
                    &self.cipher,
                    &mut self.previous,
                    &self.partial,
                    &mut self.inner,
                )?;
                self.written += 16;
            }
            self.inner.flush()?;
            self.finished = true;
        }

        Ok((self.inner, self.written))
    }
}

impl<W: Write> Write for EncryptWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.finished {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "encrypted writer already finished",
            ));
        }

        let mut chunk = buf;
        if self.partial_len > 0 {
            let take = (16 - self.partial_len).min(chunk.len());
            self.partial[self.partial_len..self.partial_len + take].copy_from_slice(&chunk[..take]);
            self.partial_len += take;
            chunk = &chunk[take..];

            if self.partial_len == 16 {
                encrypt_block(
                    &self.cipher,
                    &mut self.previous,
                    &self.partial,
                    &mut self.inner,
                )?;
                self.written += 16;
                self.partial = [0; 16];
                self.partial_len = 0;
            }
        }

        while chunk.len() >= 16 {
            let block: &[u8; 16] = chunk[..16].try_into().expect("slice length checked");
            encrypt_block(&self.cipher, &mut self.previous, block, &mut self.inner)?;
            self.written += 16;
            chunk = &chunk[16..];
        }

        if !chunk.is_empty() {
            self.partial[..chunk.len()].copy_from_slice(chunk);
            self.partial_len = chunk.len();
        }

        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

fn encrypt_block<W: Write>(
    cipher: &aes::Aes128,
    previous: &mut [u8; 16],
    plaintext: &[u8; 16],
    writer: &mut W,
) -> io::Result<()> {
    let mut block = [0u8; 16];
    for (out, (plain, prev)) in block.iter_mut().zip(plaintext.iter().zip(previous.iter())) {
        *out = plain ^ prev;
    }

    let mut block = GenericArray::clone_from_slice(&block);
    cipher.encrypt_block(&mut block);
    writer.write_all(&block)?;
    previous.copy_from_slice(&block);
    Ok(())
}

/// Compute the SHA-256 digest that CigDataPatcher signs into the
/// 128-byte RSA metadata field.
///
/// `RSA1024_SignMetaData` signs the output of
/// `SHA256_ExecuteComputeFromFileMetaData`, which hashes these fields
/// in order using little-endian integers:
///
/// 1. CIG CRC32C (`u32`)
/// 2. compressed size (`u64`)
/// 3. uncompressed size (`u64`)
/// 4. file name bytes
///
/// When `lowercase_normalize` is true, the dump lowercases file-name
/// bytes and maps `\` to `/` before hashing. It does not call the
/// archive path normalizer, so duplicate separators, `./`, `../`, and
/// trailing spaces remain part of the signed byte stream. The normal
/// P4K-ready creation path uses that lowercasing mode.
pub fn signature_metadata_sha256(
    name: &str,
    crc32: u32,
    compressed_size: u64,
    uncompressed_size: u64,
    lowercase_normalize: bool,
) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(crc32.to_le_bytes());
    hasher.update(compressed_size.to_le_bytes());
    hasher.update(uncompressed_size.to_le_bytes());
    if lowercase_normalize {
        let normalized = name.bytes().map(|byte| match byte {
            b'\\' => b'/',
            b'A'..=b'Z' => byte.to_ascii_lowercase(),
            _ => byte,
        });
        for byte in normalized {
            hasher.update([byte]);
        }
    } else {
        hasher.update(name.as_bytes());
    }

    hasher.finalize().into()
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

    #[test]
    fn test_encrypt_decrypt_roundtrip() {
        let original = b"p4k encrypted payload";
        let encrypted = encrypt(original).unwrap();
        assert_ne!(encrypted, original);
        assert_eq!(encrypted.len() % 16, 0);
        let decrypted = decrypt(&encrypted).unwrap();
        assert_eq!(&decrypted[..original.len()], original);
        assert!(decrypted[original.len()..].iter().all(|&byte| byte == 0));
    }

    #[test]
    fn test_decrypt_preserves_trailing_zeroes_and_padding() {
        let original = b"payload with real zeroes\0\0";
        let encrypted = encrypt(original).unwrap();
        let decrypted = decrypt(&encrypted).unwrap();
        assert_eq!(&decrypted[..original.len()], original);
        assert_eq!(decrypted.len(), encrypted.len());
    }

    #[test]
    fn test_encrypt_matches_dump_cbc_chain() {
        let original = b"0123456789ABCDEF0123456789ABCDEF";
        let encrypted = encrypt(original).unwrap();
        let expected = [
            0xFC, 0xBA, 0x3C, 0xF3, 0x73, 0xF0, 0xE0, 0x49, 0x97, 0xAE, 0xD1, 0xAA, 0x45, 0x9B,
            0x7F, 0xCC, 0xEC, 0x17, 0x1E, 0x23, 0xAD, 0x89, 0x70, 0x17, 0xED, 0x6E, 0xC9, 0x64,
            0x78, 0xA7, 0x0B, 0xB8,
        ];
        assert_eq!(encrypted, expected);
        assert_eq!(decrypt(&encrypted).unwrap(), original);
    }

    #[test]
    fn streaming_encrypt_matches_buffer_encrypt() {
        let cases: &[&[u8]] = &[
            b"",
            b"partial block",
            b"0123456789ABCDEF",
            b"0123456789ABCDEFtail",
            b"0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF",
        ];

        for case in cases {
            let expected = encrypt(case).unwrap();
            let mut reader = std::io::Cursor::new(case);
            let mut actual = Vec::new();
            let written = encrypt_reader_to_writer(&mut reader, &mut actual).unwrap();
            assert_eq!(written, actual.len() as u64);
            assert_eq!(actual, expected);
        }
    }

    #[test]
    fn streaming_encrypt_writer_matches_buffer_encrypt() {
        let original = b"streamed encrypted writer payload with split writes";
        let expected = encrypt(original).unwrap();
        let mut writer = EncryptWriter::new(Vec::new());
        writer.write_all(&original[..7]).unwrap();
        writer.write_all(&original[7..31]).unwrap();
        writer.write_all(&original[31..]).unwrap();
        let (actual, written) = writer.finish().unwrap();
        assert_eq!(written, actual.len() as u64);
        assert_eq!(actual, expected);
    }

    #[test]
    fn streaming_decrypt_matches_buffer_decrypt() {
        let cases: &[&[u8]] = &[
            b"",
            b"partial block",
            b"0123456789ABCDEF",
            b"0123456789ABCDEFtail",
            b"0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF",
        ];

        for case in cases {
            let encrypted = encrypt(case).unwrap();
            let expected = decrypt(&encrypted).unwrap();
            let mut actual = Vec::new();
            let mut reader = DecryptReader::new(std::io::Cursor::new(encrypted));
            reader.read_to_end(&mut actual).unwrap();
            assert_eq!(actual, expected);
        }
    }

    #[test]
    fn streaming_decrypt_rejects_partial_cipher_block() {
        let mut reader = DecryptReader::new(std::io::Cursor::new(vec![0u8; 15]));
        let mut output = Vec::new();
        let err = reader.read_to_end(&mut output).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    }

    #[test]
    fn signature_metadata_digest_matches_dump_field_order() {
        let digest =
            signature_metadata_sha256("Data\\Textures/Ship.DDS", 0x1234_ABCD, 42, 9000, true);

        let mut manual = Sha256::new();
        manual.update(0x1234_ABCDu32.to_le_bytes());
        manual.update(42u64.to_le_bytes());
        manual.update(9000u64.to_le_bytes());
        manual.update(b"data/textures/ship.dds");
        assert_eq!(digest, <[u8; 32]>::from(manual.finalize()));
    }

    #[test]
    fn signature_metadata_digest_can_preserve_name_bytes() {
        let normalized =
            signature_metadata_sha256("Data\\Textures/Ship.DDS", 0x1234_ABCD, 42, 9000, true);
        let raw =
            signature_metadata_sha256("Data\\Textures/Ship.DDS", 0x1234_ABCD, 42, 9000, false);
        assert_ne!(normalized, raw);
    }

    #[test]
    fn signature_metadata_lowercase_mode_does_not_full_normalize_path() {
        let digest =
            signature_metadata_sha256("Data\\.//Foo/../Bar.DDS   ", 0x1111_2222, 33, 44, true);

        let mut manual = Sha256::new();
        manual.update(0x1111_2222u32.to_le_bytes());
        manual.update(33u64.to_le_bytes());
        manual.update(44u64.to_le_bytes());
        manual.update(b"data/.//foo/../bar.dds   ");
        assert_eq!(digest, <[u8; 32]>::from(manual.finalize()));

        let mut fully_normalized = Sha256::new();
        fully_normalized.update(0x1111_2222u32.to_le_bytes());
        fully_normalized.update(33u64.to_le_bytes());
        fully_normalized.update(44u64.to_le_bytes());
        fully_normalized.update(b"data/bar.dds");
        assert_ne!(digest, <[u8; 32]>::from(fully_normalized.finalize()));
    }
}
