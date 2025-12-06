//! Binary reader for zero-copy parsing of byte slices.
//!
//! This module provides [`BinaryReader`], a cursor-like type that efficiently
//! reads binary data from a byte slice without copying.

use std::io::{self, Read};

use zerocopy::FromBytes;

use crate::{Error, Result};

/// A binary reader that provides zero-copy reading from a byte slice.
///
/// This is similar to .NET's `SpanReader` - it maintains a position and reads
/// data without copying where possible.
///
/// # Example
///
/// ```
/// use svarog_common::BinaryReader;
///
/// let data = [0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08];
/// let mut reader = BinaryReader::new(&data);
///
/// assert_eq!(reader.read_u32().unwrap(), 0x04030201);
/// assert_eq!(reader.read_u32().unwrap(), 0x08070605);
/// assert!(reader.is_empty());
/// ```
#[derive(Debug, Clone)]
pub struct BinaryReader<'a> {
    data: &'a [u8],
    position: usize,
}

impl<'a> BinaryReader<'a> {
    /// Create a new reader from a byte slice.
    #[inline]
    pub const fn new(data: &'a [u8]) -> Self {
        Self { data, position: 0 }
    }

    /// Create a new reader starting at a specific position.
    #[inline]
    pub const fn new_at(data: &'a [u8], position: usize) -> Self {
        Self { data, position }
    }

    /// Get the current position in the buffer.
    #[inline]
    pub const fn position(&self) -> usize {
        self.position
    }

    /// Get the total length of the underlying buffer.
    #[inline]
    pub const fn len(&self) -> usize {
        self.data.len()
    }

    /// Get the number of bytes remaining to read.
    #[inline]
    pub const fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.position)
    }

    /// Check if there are no more bytes to read.
    #[inline]
    pub const fn is_empty(&self) -> bool {
        self.position >= self.data.len()
    }

    /// Seek to an absolute position.
    #[inline]
    pub fn seek(&mut self, position: usize) {
        self.position = position;
    }

    /// Advance the position by a number of bytes.
    #[inline]
    pub fn advance(&mut self, count: usize) {
        self.position = self.position.saturating_add(count);
    }

    /// Get the remaining bytes as a slice.
    #[inline]
    pub fn remaining_bytes(&self) -> &'a [u8] {
        &self.data[self.position.min(self.data.len())..]
    }

    /// Peek at bytes without advancing the position.
    #[inline]
    pub fn peek_bytes(&self, count: usize) -> Result<&'a [u8]> {
        if self.remaining() < count {
            return Err(Error::UnexpectedEof {
                needed: count,
                available: self.remaining(),
            });
        }
        Ok(&self.data[self.position..self.position + count])
    }

    /// Read bytes and advance the position.
    #[inline]
    pub fn read_bytes(&mut self, count: usize) -> Result<&'a [u8]> {
        let bytes = self.peek_bytes(count)?;
        self.position += count;
        Ok(bytes)
    }

    /// Read a single byte.
    #[inline]
    pub fn read_u8(&mut self) -> Result<u8> {
        self.read_bytes(1).map(|b| b[0])
    }

    /// Read a signed byte.
    #[inline]
    pub fn read_i8(&mut self) -> Result<i8> {
        self.read_u8().map(|b| b as i8)
    }

    /// Read a boolean (non-zero = true).
    #[inline]
    pub fn read_bool(&mut self) -> Result<bool> {
        self.read_u8().map(|b| b != 0)
    }

    /// Read a little-endian u16.
    #[inline]
    pub fn read_u16(&mut self) -> Result<u16> {
        let bytes = self.read_bytes(2)?;
        Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
    }

    /// Read a little-endian i16.
    #[inline]
    pub fn read_i16(&mut self) -> Result<i16> {
        let bytes = self.read_bytes(2)?;
        Ok(i16::from_le_bytes([bytes[0], bytes[1]]))
    }

    /// Read a little-endian u32.
    #[inline]
    pub fn read_u32(&mut self) -> Result<u32> {
        let bytes = self.read_bytes(4)?;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    /// Read a little-endian i32.
    #[inline]
    pub fn read_i32(&mut self) -> Result<i32> {
        let bytes = self.read_bytes(4)?;
        Ok(i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    /// Read a little-endian u64.
    #[inline]
    pub fn read_u64(&mut self) -> Result<u64> {
        let bytes = self.read_bytes(8)?;
        Ok(u64::from_le_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        ]))
    }

    /// Read a little-endian i64.
    #[inline]
    pub fn read_i64(&mut self) -> Result<i64> {
        let bytes = self.read_bytes(8)?;
        Ok(i64::from_le_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        ]))
    }

    /// Read a little-endian f32.
    #[inline]
    pub fn read_f32(&mut self) -> Result<f32> {
        let bytes = self.read_bytes(4)?;
        Ok(f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    /// Read a little-endian f64.
    #[inline]
    pub fn read_f64(&mut self) -> Result<f64> {
        let bytes = self.read_bytes(8)?;
        Ok(f64::from_le_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        ]))
    }

    /// Read a null-terminated ASCII string.
    pub fn read_cstring(&mut self) -> Result<&'a str> {
        let start = self.position;
        let remaining = self.remaining_bytes();

        let null_pos = remaining
            .iter()
            .position(|&b| b == 0)
            .ok_or_else(|| Error::MissingNullTerminator)?;

        let string_bytes = &remaining[..null_pos];
        self.position = start + null_pos + 1; // Skip the null terminator

        std::str::from_utf8(string_bytes).map_err(Error::Utf8)
    }

    /// Read a string of a specific length.
    pub fn read_string(&mut self, length: usize) -> Result<&'a str> {
        let bytes = self.read_bytes(length)?;
        std::str::from_utf8(bytes).map_err(Error::Utf8)
    }

    /// Read a string from a fixed-size buffer, stopping at the first null.
    pub fn read_string_in_buffer(&mut self, buffer_size: usize) -> Result<&'a str> {
        let bytes = self.read_bytes(buffer_size)?;
        let null_pos = bytes.iter().position(|&b| b == 0).unwrap_or(buffer_size);
        std::str::from_utf8(&bytes[..null_pos]).map_err(Error::Utf8)
    }

    /// Read a struct using zerocopy.
    ///
    /// The struct must implement `FromBytes` from the zerocopy crate.
    #[inline]
    pub fn read_struct<T: FromBytes>(&mut self) -> Result<T> {
        let size = std::mem::size_of::<T>();
        let bytes = self.read_bytes(size)?;
        T::read_from_bytes(bytes).map_err(|_| Error::UnexpectedEof {
            needed: size,
            available: bytes.len(),
        })
    }

    /// Peek at a value without advancing.
    #[inline]
    pub fn peek_u32(&self) -> Result<u32> {
        let bytes = self.peek_bytes(4)?;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    /// Expect a specific value or return an error.
    pub fn expect<T: PartialEq + std::fmt::Debug + FromBytes>(&mut self, expected: T) -> Result<()> {
        let actual = self.read_struct::<T>()?;
        if actual != expected {
            return Err(Error::ExpectedValue {
                expected: format!("{:?}", expected),
                actual: format!("{:?}", actual),
            });
        }
        Ok(())
    }

    /// Expect specific magic bytes.
    pub fn expect_magic(&mut self, expected: &[u8]) -> Result<()> {
        let actual = self.read_bytes(expected.len())?;
        if actual != expected {
            return Err(Error::InvalidMagic {
                expected: expected.to_vec(),
                actual: actual.to_vec(),
            });
        }
        Ok(())
    }
}

/// Trait for reading binary data from streams.
///
/// This extends `Read` with methods for reading fixed-size structures.
/// Currently unused but will be useful for streaming reads.
#[allow(dead_code)]
pub trait ReadExt: Read {
    /// Read a structure from the stream.
    fn read_struct<T: FromBytes + Default>(&mut self) -> io::Result<T> {
        let size = std::mem::size_of::<T>();
        let mut bytes = vec![0u8; size];
        self.read_exact(&mut bytes)?;
        T::read_from_bytes(&bytes)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("{:?}", e)))
    }

    /// Read an array of structures from the stream.
    fn read_array<T: FromBytes + Clone + Default>(&mut self, count: usize) -> io::Result<Vec<T>> {
        let elem_size = std::mem::size_of::<T>();
        let total_size = count * elem_size;
        let mut bytes = vec![0u8; total_size];
        self.read_exact(&mut bytes)?;

        let mut result = Vec::with_capacity(count);
        for chunk in bytes.chunks_exact(elem_size) {
            let item = T::read_from_bytes(chunk)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("{:?}", e)))?;
            result.push(item);
        }
        Ok(result)
    }
}

impl<R: Read> ReadExt for R {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_read_primitives() {
        let data = [
            0x01u8, 0x02, 0x03, 0x04, // u32: 0x04030201
            0xFF, 0xFF, 0xFF, 0xFF, // u32: 0xFFFFFFFF
        ];
        let mut reader = BinaryReader::new(&data);

        assert_eq!(reader.read_u32().unwrap(), 0x04030201);
        assert_eq!(reader.read_u32().unwrap(), 0xFFFFFFFF);
        assert!(reader.is_empty());
    }

    #[test]
    fn test_read_cstring() {
        let data = b"hello\0world\0";
        let mut reader = BinaryReader::new(data);

        assert_eq!(reader.read_cstring().unwrap(), "hello");
        assert_eq!(reader.read_cstring().unwrap(), "world");
    }

    #[test]
    fn test_peek_does_not_advance() {
        let data = [0x01, 0x02, 0x03, 0x04];
        let mut reader = BinaryReader::new(&data);

        assert_eq!(reader.peek_u32().unwrap(), 0x04030201);
        assert_eq!(reader.position(), 0);
        assert_eq!(reader.read_u32().unwrap(), 0x04030201);
        assert_eq!(reader.position(), 4);
    }

    #[test]
    fn test_eof_error() {
        let data = [0x01, 0x02];
        let mut reader = BinaryReader::new(&data);

        assert!(reader.read_u32().is_err());
    }
}
