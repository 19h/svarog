//! CIG GUID type - Star Citizen's custom GUID format.
//!
//! The CigGuid is a 16-byte identifier used throughout Star Citizen's data files.
//! It uses a non-standard byte ordering that differs from standard UUID format.

use std::fmt;
use std::str::FromStr;

use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout};

use crate::Error;

/// A 16-byte GUID used in Star Citizen files.
///
/// The byte ordering is specific to CIG's format and differs from standard UUID.
/// Format: `XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX`
///
/// # Byte Layout
///
/// The bytes are stored in a specific non-standard order:
/// - String positions 0-7 (first group): bytes 7,6,5,4
/// - String positions 9-12 (second group): bytes 3,2
/// - String positions 14-17 (third group): bytes 1,0
/// - String positions 19-22 (fourth group): bytes 15,14
/// - String positions 24-35 (fifth group): bytes 13,12,11,10,9,8
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default, FromBytes, IntoBytes, Immutable, KnownLayout)]
#[repr(C)]
pub struct CigGuid {
    bytes: [u8; 16],
}

impl CigGuid {
    /// Empty GUID (all zeros).
    pub const EMPTY: Self = Self { bytes: [0; 16] };

    /// Create a new CigGuid from raw bytes.
    #[inline]
    pub const fn from_bytes(bytes: [u8; 16]) -> Self {
        Self { bytes }
    }

    /// Generate a random GUID.
    ///
    /// Uses a simple linear congruential generator seeded from system time.
    /// This is suitable for generating unique IDs but not for cryptographic purposes.
    #[inline]
    pub fn random() -> Self {
        use std::time::{SystemTime, UNIX_EPOCH};

        // Simple LCG seeded from current time and a counter
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

        let time_seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);

        let counter = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut state = time_seed.wrapping_add(counter).wrapping_mul(6364136223846793005);

        let mut bytes = [0u8; 16];
        for chunk in bytes.chunks_exact_mut(8) {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            chunk.copy_from_slice(&state.to_le_bytes());
        }

        Self { bytes }
    }

    /// Get the raw bytes of the GUID.
    #[inline]
    pub const fn as_bytes(&self) -> &[u8; 16] {
        &self.bytes
    }

    /// Check if the GUID is empty (all zeros).
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.bytes == [0; 16]
    }
}

impl fmt::Debug for CigGuid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "CigGuid({})", self)
    }
}

impl fmt::Display for CigGuid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Format: XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX
        // Byte mapping based on .NET implementation:
        // Position 0-7: bytes[7], bytes[6], bytes[5], bytes[4]
        // Position 9-12: bytes[3], bytes[2]
        // Position 14-17: bytes[1], bytes[0]
        // Position 19-22: bytes[15], bytes[14]
        // Position 24-35: bytes[13], bytes[12], bytes[11], bytes[10], bytes[9], bytes[8]
        write!(
            f,
            "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
            self.bytes[7], self.bytes[6], self.bytes[5], self.bytes[4],
            self.bytes[3], self.bytes[2],
            self.bytes[1], self.bytes[0],
            self.bytes[15], self.bytes[14],
            self.bytes[13], self.bytes[12], self.bytes[11], self.bytes[10], self.bytes[9], self.bytes[8]
        )
    }
}

impl FromStr for CigGuid {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.len() != 36 {
            return Err(Error::InvalidGuid(format!(
                "expected 36 characters, got {}",
                s.len()
            )));
        }

        let chars: Vec<char> = s.chars().collect();

        // Validate hyphens
        if chars[8] != '-' || chars[13] != '-' || chars[18] != '-' || chars[23] != '-' {
            return Err(Error::InvalidGuid("invalid hyphen positions".into()));
        }

        let parse_hex = |start: usize| -> Result<u8, Error> {
            let hex_str: String = chars[start..start + 2].iter().collect();
            u8::from_str_radix(&hex_str, 16)
                .map_err(|_| Error::InvalidGuid(format!("invalid hex at position {}", start)))
        };

        let mut bytes = [0u8; 16];

        // Map string positions to byte positions (inverse of Display)
        // First group (0-7): bytes 7,6,5,4
        bytes[7] = parse_hex(0)?;
        bytes[6] = parse_hex(2)?;
        bytes[5] = parse_hex(4)?;
        bytes[4] = parse_hex(6)?;

        // Second group (9-12): bytes 3,2
        bytes[3] = parse_hex(9)?;
        bytes[2] = parse_hex(11)?;

        // Third group (14-17): bytes 1,0
        bytes[1] = parse_hex(14)?;
        bytes[0] = parse_hex(16)?;

        // Fourth group (19-22): bytes 15,14
        bytes[15] = parse_hex(19)?;
        bytes[14] = parse_hex(21)?;

        // Fifth group (24-35): bytes 13,12,11,10,9,8
        bytes[13] = parse_hex(24)?;
        bytes[12] = parse_hex(26)?;
        bytes[11] = parse_hex(28)?;
        bytes[10] = parse_hex(30)?;
        bytes[9] = parse_hex(32)?;
        bytes[8] = parse_hex(34)?;

        Ok(Self { bytes })
    }
}

#[cfg(feature = "serde")]
impl serde::Serialize for CigGuid {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

#[cfg(feature = "serde")]
impl<'de> serde::Deserialize<'de> for CigGuid {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_guid() {
        let guid = CigGuid::EMPTY;
        assert!(guid.is_empty());
        assert_eq!(
            guid.to_string(),
            "00000000-0000-0000-0000-000000000000"
        );
    }

    #[test]
    fn test_roundtrip() {
        let original = "12345678-abcd-ef01-2345-6789abcdef01";
        let guid: CigGuid = original.parse().unwrap();
        assert_eq!(guid.to_string(), original);
    }

    #[test]
    fn test_invalid_length() {
        assert!("too-short".parse::<CigGuid>().is_err());
    }

    #[test]
    fn test_invalid_hyphens() {
        assert!("12345678_abcd-ef01-2345-6789abcdef01".parse::<CigGuid>().is_err());
    }
}
