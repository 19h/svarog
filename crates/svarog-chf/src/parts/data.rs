//! CHF data container.
//!
//! The ChfData structure represents the decompressed contents of a CHF file,
//! containing character gender, DNA (facial features), equipment ports, and materials.

use svarog_common::{BinaryReader, CigGuid};

use super::dna::Dna;
use super::itemport::ItemPort;
use super::material::Material;
use super::name_hash::NameHash;
use crate::{Error, Result};

/// The main CHF data container.
///
/// Contains all character customization data:
/// - Gender ID (GUID identifying male/female)
/// - DNA (facial feature morphs)
/// - Item port tree (equipment attachment points)
/// - Materials (appearance customizations)
#[derive(Debug, Clone)]
pub struct ChfData {
    /// The gender GUID.
    gender_id: CigGuid,
    /// DNA facial feature data.
    dna: Dna,
    /// Root item port tree.
    item_port: Option<ItemPort>,
    /// Material definitions.
    materials: Vec<Material>,
}

impl ChfData {
    /// Create a new empty ChfData.
    pub fn new(gender_id: CigGuid) -> Self {
        Self {
            gender_id,
            dna: Dna::new(),
            item_port: None,
            materials: Vec::new(),
        }
    }

    /// Parse ChfData from decompressed bytes.
    pub fn parse(data: &[u8]) -> Result<Self> {
        let mut reader = BinaryReader::new(data);

        // Read gender GUID (16 bytes)
        let guid_bytes = reader.read_bytes(16)?;
        let gender_id = CigGuid::from_bytes(guid_bytes.try_into().unwrap());

        // Read DNA (0xD8 bytes)
        let dna_bytes = reader.read_bytes(super::dna::DNA_SIZE)?;
        let dna = Dna::parse(dna_bytes)?;

        // Check if there's an item port tree
        let has_item_port = reader.remaining() >= 4;
        let item_port = if has_item_port {
            // Peek at the next 4 bytes to see if it looks like a valid name hash
            let pos = reader.position();
            let maybe_hash = reader.read_u32().ok();

            // Reset position
            reader = BinaryReader::new(&data[pos..]);

            if maybe_hash.is_some() && maybe_hash != Some(0) {
                // Try to parse item port tree
                match read_item_port(&mut reader) {
                    Ok(port) => Some(port),
                    Err(_) => None,
                }
            } else {
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
                if reader.remaining() < 20 {
                    break;
                }
                match Material::read(&mut reader) {
                    Ok(mat) => materials.push(mat),
                    Err(_) => break,
                }
            }
        }

        Ok(Self {
            gender_id,
            dna,
            item_port,
            materials,
        })
    }

    /// Get the gender GUID.
    pub fn gender_id(&self) -> &CigGuid {
        &self.gender_id
    }

    /// Set the gender GUID.
    pub fn set_gender_id(&mut self, guid: CigGuid) {
        self.gender_id = guid;
    }

    /// Get the DNA data.
    pub fn dna(&self) -> &Dna {
        &self.dna
    }

    /// Get mutable access to DNA data.
    pub fn dna_mut(&mut self) -> &mut Dna {
        &mut self.dna
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

    /// Convert to bytes for writing.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::new();

        // Write gender GUID
        bytes.extend_from_slice(self.gender_id.as_bytes());

        // Write DNA
        bytes.extend_from_slice(&self.dna.to_bytes());

        // Write item port tree
        if let Some(ref port) = self.item_port {
            bytes.extend_from_slice(&port.to_bytes());
        } else {
            // Write a placeholder empty port
            bytes.extend_from_slice(&0u32.to_le_bytes()); // name hash = 0
            bytes.extend_from_slice(&[0u8; 16]); // nil GUID
            bytes.extend_from_slice(&0u32.to_le_bytes()); // no children
        }

        // Write materials
        bytes.extend_from_slice(&(self.materials.len() as u32).to_le_bytes());
        for material in &self.materials {
            bytes.extend_from_slice(&material.to_bytes());
        }

        bytes
    }
}

/// Read an item port tree from a binary reader.
fn read_item_port(reader: &mut BinaryReader<'_>) -> Result<ItemPort> {
    // Read name hash (4 bytes)
    let name_hash = reader.read_u32()?;
    let name = NameHash::from_raw(name_hash);

    // Read GUID (16 bytes) - all zeros means no item attached
    let guid_bytes = reader.read_bytes(16)?;
    let item_guid = {
        let guid = CigGuid::from_bytes(guid_bytes.try_into().unwrap());
        if guid.is_empty() {
            None
        } else {
            Some(guid)
        }
    };

    // Read child count (4 bytes)
    let child_count = reader.read_u32()? as usize;

    // Sanity check to prevent infinite loops
    if child_count > 1000 {
        return Err(Error::SizeMismatch {
            expected: 0,
            actual: child_count,
        });
    }

    // Read children recursively
    let mut children = Vec::with_capacity(child_count);
    for _ in 0..child_count {
        children.push(read_item_port(reader)?);
    }

    Ok(ItemPort::with_children(name, item_guid, children))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chf_data_new() {
        let guid = CigGuid::from_bytes([1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]);
        let data = ChfData::new(guid);

        assert_eq!(data.gender_id().as_bytes(), guid.as_bytes());
        assert!(data.item_port().is_none());
        assert!(data.materials().is_empty());
    }
}
