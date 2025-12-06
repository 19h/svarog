//! Name hash utilities for CHF parsing.
//!
//! CHF files use CRC32C hashes to identify field names and item types.
//! This module provides a lookup dictionary to reverse these hashes to
//! human-readable names.

use std::collections::HashMap;
use std::sync::LazyLock;

use svarog_common::crc;

/// A name hash using CRC32C.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NameHash(pub u32);

impl NameHash {
    /// Create a new name hash from a string.
    pub fn from_str(s: &str) -> Self {
        Self(crc::hash_str(s))
    }

    /// Create a name hash from a raw value.
    pub const fn from_raw(value: u32) -> Self {
        Self(value)
    }

    /// Get the raw hash value.
    pub fn value(&self) -> u32 {
        self.0
    }

    /// Look up the name for this hash.
    pub fn to_name(&self) -> Option<&'static str> {
        NAME_LOOKUP.get(&self.0).copied()
    }

    /// Get the name or a hex representation if unknown.
    pub fn to_name_or_hex(&self) -> String {
        self.to_name()
            .map(|s| s.to_string())
            .unwrap_or_else(|| format!("0x{:08X}", self.0))
    }
}

impl std::fmt::Display for NameHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.to_name() {
            Some(name) => write!(f, "{}", name),
            None => write!(f, "0x{:08X}", self.0),
        }
    }
}

/// Known name strings and their CRC32C hashes.
///
/// This list is derived from the .NET implementation and includes
/// common item port names, body parts, and other CHF-related strings.
static KNOWN_NAMES: &[&str] = &[
    // Gender identifiers
    "gender",
    "male",
    "female",
    // DNA/Face parts
    "dna",
    "head",
    "eyebrow_left",
    "eyebrow_right",
    "eye_left",
    "eye_right",
    "ear_left",
    "ear_right",
    "cheek_left",
    "cheek_right",
    "nose",
    "mouth",
    "jaw",
    "crown",
    // Body parts
    "body",
    "torso",
    "arms",
    "legs",
    "hands",
    "feet",
    // Item ports
    "itemport",
    "hardpoint",
    "port_head",
    "port_body",
    "port_hands",
    "port_feet",
    "port_torso_undersuit",
    "port_torso_armor",
    "port_arms_undersuit",
    "port_arms_armor",
    "port_legs_undersuit",
    "port_legs_armor",
    "port_hands_undersuit",
    "port_hands_armor",
    "port_feet_undersuit",
    "port_feet_armor",
    "port_backpack",
    "port_helmet",
    "port_visor",
    "port_weapon_primary",
    "port_weapon_secondary",
    "port_weapon_sidearm",
    "port_weapon_melee",
    "port_tool",
    "port_gadget",
    "port_utility",
    // Materials
    "material",
    "submaterial",
    "texture",
    "diffuse",
    "normal",
    "specular",
    "gloss",
    "emissive",
    "opacity",
    "ao",
    "metalness",
    "roughness",
    // Colors
    "color",
    "color_primary",
    "color_secondary",
    "color_tertiary",
    "color_accent",
    "skin_color",
    "hair_color",
    "eye_color",
    // Hair
    "hair",
    "hair_style",
    "hair_length",
    "facial_hair",
    "beard",
    "mustache",
    "eyebrows",
    // Face features
    "face",
    "face_shape",
    "face_width",
    "face_height",
    "forehead",
    "cheekbones",
    "chin",
    "neck",
    // Eyes
    "eyes",
    "eye_shape",
    "eye_size",
    "eye_spacing",
    "eye_depth",
    "pupil_size",
    "iris_color",
    // Nose details
    "nose_bridge",
    "nose_tip",
    "nose_width",
    "nostrils",
    // Mouth details
    "lip_shape",
    "lip_size",
    "lip_fullness",
    // Wrinkles and details
    "wrinkles",
    "freckles",
    "moles",
    "scars",
    "tattoos",
    // Loadout
    "loadout",
    "equipment",
    "clothing",
    "armor",
    "undersuit",
    // Common CIG asset names
    "cig",
    "sc",
    "star_citizen",
    "pu",
    "ac",
    "sm",
    // Attachment points
    "attach",
    "attach_point",
    "bone",
    "socket",
    // Additional known hashes from the .NET source
    "Head",
    "EyebrowLeft",
    "EyebrowRight",
    "EyeLeft",
    "EyeRight",
    "EarLeft",
    "EarRight",
    "CheekLeft",
    "CheekRight",
    "Nose",
    "Mouth",
    "Jaw",
    "Crown",
];

/// Lookup table from CRC32C hash to name.
static NAME_LOOKUP: LazyLock<HashMap<u32, &'static str>> = LazyLock::new(|| {
    let mut map = HashMap::with_capacity(KNOWN_NAMES.len());
    for &name in KNOWN_NAMES {
        let hash = crc::hash_str(name);
        map.insert(hash, name);
    }
    map
});

/// Register additional names to the lookup table.
/// Note: This won't work with LazyLock, so we provide a function
/// to check if a hash is known.
pub fn is_known_hash(hash: u32) -> bool {
    NAME_LOOKUP.contains_key(&hash)
}

/// Get all known hashes.
pub fn known_hashes() -> impl Iterator<Item = (u32, &'static str)> {
    NAME_LOOKUP.iter().map(|(&k, &v)| (k, v))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_name_hash_roundtrip() {
        let hash = NameHash::from_str("head");
        assert_eq!(hash.to_name(), Some("head"));
    }

    #[test]
    fn test_unknown_hash() {
        let hash = NameHash::from_raw(0xDEADBEEF);
        assert_eq!(hash.to_name(), None);
        assert_eq!(hash.to_name_or_hex(), "0xDEADBEEF");
    }

    #[test]
    fn test_display() {
        let known = NameHash::from_str("dna");
        assert_eq!(format!("{}", known), "dna");

        let unknown = NameHash::from_raw(0x12345678);
        assert_eq!(format!("{}", unknown), "0x12345678");
    }
}
