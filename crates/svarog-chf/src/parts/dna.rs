//! DNA (facial feature) data structures.
//!
//! The DNA system represents character facial features using a blend-based approach.
//! Each face part (eyebrow, eye, nose, etc.) has up to 4 blend targets, where each
//! target specifies a "head ID" (the base morph) and a percentage (blend weight).

use svarog_common::BinaryReader;

use crate::{Error, Result};

/// Size of the DNA data block in bytes.
pub const DNA_SIZE: usize = 0xD8; // 216 bytes

/// Number of DNA parts (12 face parts × 4 blends each = 48).
pub const DNA_PART_COUNT: usize = 48;

/// Number of blends per face part.
pub const BLENDS_PER_FACE_PART: usize = 4;

/// Facial feature parts that can be customized.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum FacePart {
    /// Left eyebrow.
    EyebrowLeft = 0,
    /// Right eyebrow.
    EyebrowRight = 1,
    /// Left eye.
    EyeLeft = 2,
    /// Right eye.
    EyeRight = 3,
    /// Left ear.
    EarLeft = 4,
    /// Right ear.
    EarRight = 5,
    /// Left cheek.
    CheekLeft = 6,
    /// Right cheek.
    CheekRight = 7,
    /// Nose.
    Nose = 8,
    /// Mouth.
    Mouth = 9,
    /// Jaw.
    Jaw = 10,
    /// Crown (top of head).
    Crown = 11,
}

impl FacePart {
    /// Get all face parts in order.
    pub const fn all() -> [FacePart; 12] {
        [
            FacePart::EyebrowLeft,
            FacePart::EyebrowRight,
            FacePart::EyeLeft,
            FacePart::EyeRight,
            FacePart::EarLeft,
            FacePart::EarRight,
            FacePart::CheekLeft,
            FacePart::CheekRight,
            FacePart::Nose,
            FacePart::Mouth,
            FacePart::Jaw,
            FacePart::Crown,
        ]
    }

    /// Get the name of this face part.
    pub const fn name(&self) -> &'static str {
        match self {
            FacePart::EyebrowLeft => "EyebrowLeft",
            FacePart::EyebrowRight => "EyebrowRight",
            FacePart::EyeLeft => "EyeLeft",
            FacePart::EyeRight => "EyeRight",
            FacePart::EarLeft => "EarLeft",
            FacePart::EarRight => "EarRight",
            FacePart::CheekLeft => "CheekLeft",
            FacePart::CheekRight => "CheekRight",
            FacePart::Nose => "Nose",
            FacePart::Mouth => "Mouth",
            FacePart::Jaw => "Jaw",
            FacePart::Crown => "Crown",
        }
    }

    /// Get the starting index in the DNA parts array.
    pub const fn start_index(&self) -> usize {
        (*self as usize) * BLENDS_PER_FACE_PART
    }
}

impl TryFrom<u8> for FacePart {
    type Error = ();

    fn try_from(value: u8) -> std::result::Result<Self, Self::Error> {
        match value {
            0 => Ok(FacePart::EyebrowLeft),
            1 => Ok(FacePart::EyebrowRight),
            2 => Ok(FacePart::EyeLeft),
            3 => Ok(FacePart::EyeRight),
            4 => Ok(FacePart::EarLeft),
            5 => Ok(FacePart::EarRight),
            6 => Ok(FacePart::CheekLeft),
            7 => Ok(FacePart::CheekRight),
            8 => Ok(FacePart::Nose),
            9 => Ok(FacePart::Mouth),
            10 => Ok(FacePart::Jaw),
            11 => Ok(FacePart::Crown),
            _ => Err(()),
        }
    }
}

impl std::fmt::Display for FacePart {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name())
    }
}

/// A single DNA blend target.
///
/// Each face part can have up to 4 blend targets. Each target specifies
/// a head ID (base morph shape) and a percentage (blend weight 0.0-1.0).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DnaPart {
    /// The head ID (base morph shape index).
    pub head_id: u8,
    /// The blend percentage (0.0 to 1.0).
    pub percent: f32,
}

impl DnaPart {
    /// Create a new DNA part.
    pub fn new(head_id: u8, percent: f32) -> Self {
        Self { head_id, percent }
    }

    /// Create a zeroed DNA part.
    pub const fn zero() -> Self {
        Self {
            head_id: 0,
            percent: 0.0,
        }
    }

    /// Check if this part is effectively unused (zero percent).
    pub fn is_zero(&self) -> bool {
        self.percent == 0.0
    }

    /// Read a DNA part from binary data.
    ///
    /// Format: 1 byte head_id, 2 bytes percent (u16 / 65535.0), 1 byte padding
    pub fn read(reader: &mut BinaryReader<'_>) -> Result<Self> {
        let head_id = reader.read_u8()?;
        let percent_raw = reader.read_u16()?;
        let _padding = reader.read_u8()?;

        let percent = percent_raw as f32 / 65535.0;

        Ok(Self { head_id, percent })
    }

    /// Write a DNA part to bytes.
    pub fn to_bytes(&self) -> [u8; 4] {
        let percent_raw = (self.percent * 65535.0).round() as u16;
        let mut bytes = [0u8; 4];
        bytes[0] = self.head_id;
        bytes[1..3].copy_from_slice(&percent_raw.to_le_bytes());
        bytes[3] = 0; // padding
        bytes
    }
}

impl Default for DnaPart {
    fn default() -> Self {
        Self::zero()
    }
}

/// The complete DNA data for a character.
///
/// Contains 48 DNA parts organized as 12 face parts with 4 blends each.
#[derive(Debug, Clone)]
pub struct Dna {
    /// All 48 DNA parts (12 face parts × 4 blends).
    parts: [DnaPart; DNA_PART_COUNT],
    /// Additional data at the end of the DNA block (24 bytes).
    /// Purpose unknown, preserved for round-trip compatibility.
    extra: [u8; 24],
}

impl Dna {
    /// Create a new DNA with all zeroed parts.
    pub fn new() -> Self {
        Self {
            parts: [DnaPart::zero(); DNA_PART_COUNT],
            extra: [0u8; 24],
        }
    }

    /// Parse DNA data from bytes.
    pub fn parse(data: &[u8]) -> Result<Self> {
        if data.len() < DNA_SIZE {
            return Err(Error::SizeMismatch {
                expected: DNA_SIZE,
                actual: data.len(),
            });
        }

        let mut reader = BinaryReader::new(data);
        let mut parts = [DnaPart::zero(); DNA_PART_COUNT];

        // Read 48 DNA parts (4 bytes each = 192 bytes)
        for part in &mut parts {
            *part = DnaPart::read(&mut reader)?;
        }

        // Read extra data (24 bytes to reach 216 total)
        let mut extra = [0u8; 24];
        let extra_data = reader.read_bytes(24)?;
        extra.copy_from_slice(extra_data);

        Ok(Self { parts, extra })
    }

    /// Get all DNA parts.
    pub fn parts(&self) -> &[DnaPart; DNA_PART_COUNT] {
        &self.parts
    }

    /// Get mutable access to all DNA parts.
    pub fn parts_mut(&mut self) -> &mut [DnaPart; DNA_PART_COUNT] {
        &mut self.parts
    }

    /// Get the 4 blend targets for a specific face part.
    pub fn face_part_blends(&self, face_part: FacePart) -> &[DnaPart] {
        let start = face_part.start_index();
        &self.parts[start..start + BLENDS_PER_FACE_PART]
    }

    /// Get mutable access to the 4 blend targets for a specific face part.
    pub fn face_part_blends_mut(&mut self, face_part: FacePart) -> &mut [DnaPart] {
        let start = face_part.start_index();
        &mut self.parts[start..start + BLENDS_PER_FACE_PART]
    }

    /// Get the extra data bytes.
    pub fn extra(&self) -> &[u8; 24] {
        &self.extra
    }

    /// Set the extra data bytes.
    pub fn set_extra(&mut self, extra: [u8; 24]) {
        self.extra = extra;
    }

    /// Convert to bytes for writing.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(DNA_SIZE);

        // Write all DNA parts
        for part in &self.parts {
            bytes.extend_from_slice(&part.to_bytes());
        }

        // Write extra data
        bytes.extend_from_slice(&self.extra);

        bytes
    }

    /// Iterate over face parts with their blend data.
    pub fn iter_face_parts(&self) -> impl Iterator<Item = (FacePart, &[DnaPart])> {
        FacePart::all()
            .into_iter()
            .map(move |fp| (fp, self.face_part_blends(fp)))
    }
}

impl Default for Dna {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_face_part_indices() {
        assert_eq!(FacePart::EyebrowLeft.start_index(), 0);
        assert_eq!(FacePart::EyebrowRight.start_index(), 4);
        assert_eq!(FacePart::Crown.start_index(), 44);
    }

    #[test]
    fn test_dna_part_roundtrip() {
        let part = DnaPart::new(42, 0.75);
        let bytes = part.to_bytes();

        let mut reader = BinaryReader::new(&bytes);
        let parsed = DnaPart::read(&mut reader).unwrap();

        assert_eq!(parsed.head_id, 42);
        // Allow small floating point difference due to u16 conversion
        assert!((parsed.percent - 0.75).abs() < 0.001);
    }

    #[test]
    fn test_dna_size() {
        let dna = Dna::new();
        let bytes = dna.to_bytes();
        assert_eq!(bytes.len(), DNA_SIZE);
    }
}
