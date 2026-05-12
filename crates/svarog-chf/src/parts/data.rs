//! CHF data container.
//!
//! The ChfData structure represents the logical contents stored under the
//! `CharacterCustomization` serializer node in a CHF payload.

use svarog_common::{BinaryReader, CigGuid};

use super::dna::Dna;
use super::itemport::ItemPort;
use super::material::Material;
use super::name_hash::NameHash;
use crate::{Error, Result};

/// Current writer version used by the decompiled `SaveCustomHeadFile` path.
pub const CHF_CURRENT_VERSION: u32 = 9;

/// Lowest version accepted by the decompiled readers.
pub const CHF_MIN_VERSION: u32 = 2;

/// Highest version handled by the decompiled readers in this dump.
pub const CHF_MAX_VERSION: u32 = 9;

/// The main CHF data container.
///
/// Contains all character customization data:
/// - `version`
/// - `modelTag`
/// - `voiceTag`
/// - `dnaByteArray`
/// - `Loadout` item port entries
/// - `Materials` customization entries
/// - `Decals` for versions newer than 7
#[derive(Debug, Clone)]
pub struct ChfData {
    /// Serializer version.
    version: u32,
    /// Character model tag.
    model_tag: CigGuid,
    /// Character voice tag.
    voice_tag: CigGuid,
    /// Parsed DNA facial feature data.
    dna: Dna,
    /// Original serialized bytes from `dnaByteArray`.
    dna_byte_array: Vec<u8>,
    /// Root item port tree.
    item_port: Option<ItemPort>,
    /// Material definitions.
    materials: Vec<Material>,
    /// Character decal definitions.
    decals: Vec<Decal>,
}

impl ChfData {
    /// Create a new empty ChfData.
    pub fn new(model_tag: CigGuid) -> Self {
        Self {
            version: CHF_CURRENT_VERSION,
            model_tag,
            voice_tag: CigGuid::default(),
            dna: Dna::new(),
            dna_byte_array: Dna::new().to_bytes(),
            item_port: None,
            materials: Vec::new(),
            decals: Vec::new(),
        }
    }

    /// Parse ChfData from decompressed bytes.
    ///
    /// The game reads this data through `ISaveGameSerializerHelper`; the fields
    /// here mirror the decompiled `CustomHeadFileUtils::LoadCustomHeadFile`
    /// sequence. For tooling compatibility this also accepts the older compact
    /// binary projection used by prior versions of this crate.
    pub fn parse(data: &[u8]) -> Result<Self> {
        let mut reader = BinaryReader::new(data);

        let (version, model_tag, voice_tag) = match read_versioned_header(&mut reader)? {
            Some(header) => header,
            None => {
                let guid_bytes = reader.read_bytes(16)?;
                (
                    0,
                    CigGuid::from_bytes(guid_bytes.try_into().unwrap()),
                    CigGuid::default(),
                )
            }
        };

        let dna_bytes = reader.read_bytes(super::dna::DNA_SIZE)?;
        let dna = Dna::parse(dna_bytes)?;
        let dna_byte_array = dna_bytes.to_vec();

        let has_item_port = reader.remaining() >= 4;
        let item_port = if has_item_port {
            let pos = reader.position();
            let maybe_port_id = reader.read_u32().ok();

            reader = BinaryReader::new(&data[pos..]);

            if maybe_port_id.is_some() && maybe_port_id != Some(0) {
                match read_item_port(&mut reader) {
                    Ok(port) => Some(port),
                    Err(_) => None,
                }
            } else {
                if reader.remaining() >= 24 {
                    reader.advance(24);
                }
                None
            }
        } else {
            None
        };

        // Read materials
        let mut materials = Vec::new();
        if reader.remaining() >= 4 {
            let material_count = reader.read_u32().unwrap_or(0) as usize;
            for _ in 0..material_count {
                if reader.remaining() < Material::MIN_BINARY_SIZE {
                    break;
                }
                match Material::read(&mut reader) {
                    Ok(mat) => materials.push(mat),
                    Err(_) => break,
                }
            }
        }

        let mut decals = Vec::new();
        if reader.remaining() >= 4 {
            let decal_count = reader.read_u32().unwrap_or(0) as usize;
            for _ in 0..decal_count {
                if reader.remaining() < Decal::BINARY_SIZE {
                    break;
                }
                decals.push(Decal::read(&mut reader)?);
            }
        }

        Ok(Self {
            version,
            model_tag,
            voice_tag,
            dna,
            dna_byte_array,
            item_port,
            materials,
            decals,
        })
    }

    /// Get the serializer version.
    pub fn version(&self) -> u32 {
        self.version
    }

    /// Return true if the version is accepted by the decompiled readers.
    pub fn has_supported_version(&self) -> bool {
        is_supported_version(self.version)
    }

    /// Get the character model tag.
    pub fn model_tag(&self) -> &CigGuid {
        &self.model_tag
    }

    /// Set the character model tag.
    pub fn set_model_tag(&mut self, guid: CigGuid) {
        self.model_tag = guid;
    }

    /// Get the character voice tag.
    pub fn voice_tag(&self) -> &CigGuid {
        &self.voice_tag
    }

    /// Set the character voice tag.
    pub fn set_voice_tag(&mut self, guid: CigGuid) {
        self.voice_tag = guid;
    }

    /// Compatibility alias for older callers. This is `modelTag` in the game serializer.
    pub fn gender_id(&self) -> &CigGuid {
        &self.model_tag
    }

    /// Compatibility alias for older callers. This sets `modelTag`.
    pub fn set_gender_id(&mut self, guid: CigGuid) {
        self.model_tag = guid;
    }

    /// Get the DNA data.
    pub fn dna(&self) -> &Dna {
        &self.dna
    }

    /// Get mutable access to DNA data.
    pub fn dna_mut(&mut self) -> &mut Dna {
        &mut self.dna
    }

    /// Get the raw `dnaByteArray` payload.
    pub fn dna_byte_array(&self) -> &[u8] {
        &self.dna_byte_array
    }

    /// Get the item port tree, if present.
    pub fn item_port(&self) -> Option<&ItemPort> {
        self.item_port.as_ref()
    }

    /// Get mutable access to the item port tree.
    pub fn item_port_mut(&mut self) -> Option<&mut ItemPort> {
        self.item_port.as_mut()
    }

    /// Set the item port tree.
    pub fn set_item_port(&mut self, port: Option<ItemPort>) {
        self.item_port = port;
    }

    /// Get the materials.
    pub fn materials(&self) -> &[Material] {
        &self.materials
    }

    /// Get mutable access to materials.
    pub fn materials_mut(&mut self) -> &mut Vec<Material> {
        &mut self.materials
    }

    /// Add a material.
    pub fn add_material(&mut self, material: Material) {
        self.materials.push(material);
    }

    /// Find a material by name hash.
    pub fn find_material(&self, name: NameHash) -> Option<&Material> {
        self.materials.iter().find(|m| m.name() == name)
    }

    /// Get decals.
    pub fn decals(&self) -> &[Decal] {
        &self.decals
    }

    /// Get mutable access to decals.
    pub fn decals_mut(&mut self) -> &mut Vec<Decal> {
        &mut self.decals
    }

    /// Add a decal.
    pub fn add_decal(&mut self, decal: Decal) {
        self.decals.push(decal);
    }

    /// Convert to bytes for writing.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::new();

        bytes.extend_from_slice(&self.version.to_le_bytes());
        bytes.extend_from_slice(self.model_tag.as_bytes());
        bytes.extend_from_slice(self.voice_tag.as_bytes());

        bytes.extend_from_slice(&self.dna.to_bytes());

        if let Some(ref port) = self.item_port {
            bytes.extend_from_slice(&port.to_bytes());
        } else {
            bytes.extend_from_slice(&0u32.to_le_bytes());
            bytes.extend_from_slice(&[0u8; 16]);
            bytes.extend_from_slice(&0u32.to_le_bytes());
        }

        bytes.extend_from_slice(&(self.materials.len() as u32).to_le_bytes());
        for material in &self.materials {
            bytes.extend_from_slice(&material.to_bytes());
        }

        bytes.extend_from_slice(&(self.decals.len() as u32).to_le_bytes());
        for decal in &self.decals {
            bytes.extend_from_slice(&decal.to_bytes());
        }

        bytes
    }
}

fn read_versioned_header(reader: &mut BinaryReader<'_>) -> Result<Option<(u32, CigGuid, CigGuid)>> {
    if reader.remaining() < 4 + 16 + 16 + super::dna::DNA_SIZE {
        return Ok(None);
    }

    let start = reader.position();
    let version = reader.read_u32()?;
    if !is_supported_version(version) {
        reader.seek(start);
        return Ok(None);
    }

    let model_bytes = reader.read_bytes(16)?;
    let model_tag = CigGuid::from_bytes(model_bytes.try_into().unwrap());
    let voice_bytes = reader.read_bytes(16)?;
    let voice_tag = CigGuid::from_bytes(voice_bytes.try_into().unwrap());

    Ok(Some((version, model_tag, voice_tag)))
}

/// Return true if a serializer version is accepted by the decompiled readers.
pub const fn is_supported_version(version: u32) -> bool {
    version >= CHF_MIN_VERSION && version <= CHF_MAX_VERSION
}

/// Read an item port tree from a binary reader.
fn read_item_port(reader: &mut BinaryReader<'_>) -> Result<ItemPort> {
    let item_port_def_id = reader.read_u32()?;
    let name = NameHash::from_raw(item_port_def_id);

    let guid_bytes = reader.read_bytes(16)?;
    let item_guid = {
        let guid = CigGuid::from_bytes(guid_bytes.try_into().unwrap());
        if guid.is_empty() {
            None
        } else {
            Some(guid)
        }
    };

    let child_count = reader.read_u32()? as usize;

    if child_count > 1000 {
        return Err(Error::SizeMismatch {
            expected: 0,
            actual: child_count,
        });
    }

    let mut children = Vec::with_capacity(child_count);
    for _ in 0..child_count {
        children.push(read_item_port(reader)?);
    }

    Ok(ItemPort::with_children(name, item_guid, children))
}

/// A decal entry from the version 8+ `Decals` serializer node.
#[derive(Debug, Clone, PartialEq)]
pub struct Decal {
    pub decal_material_guid: CigGuid,
    pub projection_center: [f32; 3],
    pub projection_direction: [f32; 3],
    pub angle: f32,
    pub diameter: f32,
    pub decal_alpha: f32,
}

impl Decal {
    /// Binary size of the compact projection used by this crate.
    pub const BINARY_SIZE: usize = 16 + 12 + 12 + 4 + 4 + 4;

    pub fn read(reader: &mut BinaryReader<'_>) -> Result<Self> {
        let guid_bytes = reader.read_bytes(16)?;
        let decal_material_guid = CigGuid::from_bytes(guid_bytes.try_into().unwrap());
        let projection_center = [reader.read_f32()?, reader.read_f32()?, reader.read_f32()?];
        let projection_direction = [reader.read_f32()?, reader.read_f32()?, reader.read_f32()?];
        let angle = reader.read_f32()?;
        let diameter = reader.read_f32()?;
        let decal_alpha = reader.read_f32()?;

        Ok(Self {
            decal_material_guid,
            projection_center,
            projection_direction,
            angle,
            diameter,
            decal_alpha,
        })
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(Self::BINARY_SIZE);
        bytes.extend_from_slice(self.decal_material_guid.as_bytes());
        for value in self
            .projection_center
            .iter()
            .chain(self.projection_direction.iter())
            .chain([&self.angle, &self.diameter, &self.decal_alpha])
        {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chf_data_new() {
        let guid = CigGuid::from_bytes([1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]);
        let data = ChfData::new(guid);

        assert_eq!(data.version(), CHF_CURRENT_VERSION);
        assert_eq!(data.model_tag().as_bytes(), guid.as_bytes());
        assert_eq!(data.gender_id().as_bytes(), guid.as_bytes());
        assert!(data.item_port().is_none());
        assert!(data.materials().is_empty());
        assert!(data.decals().is_empty());
        assert!(data.has_supported_version());
    }

    #[test]
    fn version_guard_matches_decompiled_readers() {
        assert!(!is_supported_version(1));
        assert!(is_supported_version(2));
        assert!(is_supported_version(9));
        assert!(!is_supported_version(10));
    }

    #[test]
    fn parses_versioned_character_customization_projection() {
        let model_tag =
            CigGuid::from_bytes([1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]);
        let voice_tag =
            CigGuid::from_bytes([16, 15, 14, 13, 12, 11, 10, 9, 8, 7, 6, 5, 4, 3, 2, 1]);

        let mut bytes = Vec::new();
        bytes.extend_from_slice(&CHF_CURRENT_VERSION.to_le_bytes());
        bytes.extend_from_slice(model_tag.as_bytes());
        bytes.extend_from_slice(voice_tag.as_bytes());
        bytes.extend_from_slice(&Dna::new().to_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&[0u8; 16]);
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());

        let data = ChfData::parse(&bytes).unwrap();

        assert_eq!(data.version(), CHF_CURRENT_VERSION);
        assert_eq!(data.model_tag().as_bytes(), model_tag.as_bytes());
        assert_eq!(data.voice_tag().as_bytes(), voice_tag.as_bytes());
        assert_eq!(data.dna_byte_array().len(), super::super::dna::DNA_SIZE);
    }

    #[test]
    fn skips_empty_loadout_placeholder_before_decals() {
        let decal = Decal {
            decal_material_guid: CigGuid::from_bytes([3; 16]),
            projection_center: [1.0, 2.0, 3.0],
            projection_direction: [0.0, 1.0, 0.0],
            angle: 90.0,
            diameter: 0.5,
            decal_alpha: 0.75,
        };

        let mut bytes = Vec::new();
        bytes.extend_from_slice(&CHF_CURRENT_VERSION.to_le_bytes());
        bytes.extend_from_slice(CigGuid::default().as_bytes());
        bytes.extend_from_slice(CigGuid::default().as_bytes());
        bytes.extend_from_slice(&Dna::new().to_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&[0u8; 16]);
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(&decal.to_bytes());

        let data = ChfData::parse(&bytes).unwrap();

        assert!(data.item_port().is_none());
        assert_eq!(data.decals(), std::slice::from_ref(&decal));
    }
}
