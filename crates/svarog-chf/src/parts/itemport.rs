//! ItemPort data structures.
//!
//! ItemPorts represent a hierarchical tree of attachment points for character
//! equipment. Each item port has a name (stored as CRC32C hash), an optional
//! GUID for the attached item, and zero or more child ports.

use svarog_common::{BinaryReader, CigGuid};

use super::name_hash::NameHash;
use crate::Result;

/// An item port in the character's equipment tree.
///
/// Item ports form a recursive tree structure where each port can have
/// child ports. This is used for equipment attachment (e.g., a torso
/// armor port might have child ports for arm attachments).
#[derive(Debug, Clone)]
pub struct ItemPort {
    /// The name hash of this port.
    name: NameHash,
    /// The GUID of the item attached to this port, if any.
    item_guid: Option<CigGuid>,
    /// Child item ports.
    children: Vec<ItemPort>,
}

impl ItemPort {
    /// Create a new empty item port.
    pub fn new(name: NameHash) -> Self {
        Self {
            name,
            item_guid: None,
            children: Vec::new(),
        }
    }

    /// Create a new item port with an attached item.
    pub fn with_item(name: NameHash, item_guid: CigGuid) -> Self {
        Self {
            name,
            item_guid: Some(item_guid),
            children: Vec::new(),
        }
    }

    /// Create an item port with all fields specified.
    pub fn with_children(
        name: NameHash,
        item_guid: Option<CigGuid>,
        children: Vec<ItemPort>,
    ) -> Self {
        Self {
            name,
            item_guid,
            children,
        }
    }

    /// Parse an item port tree from binary data.
    pub fn parse(data: &[u8]) -> Result<Self> {
        let mut reader = BinaryReader::new(data);
        Self::read(&mut reader)
    }

    /// Read an item port from a binary reader.
    fn read(reader: &mut BinaryReader<'_>) -> Result<Self> {
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

        // Read children recursively
        let mut children = Vec::with_capacity(child_count);
        for _ in 0..child_count {
            children.push(Self::read(reader)?);
        }

        Ok(Self {
            name,
            item_guid,
            children,
        })
    }

    /// Get the name hash of this port.
    pub fn name(&self) -> NameHash {
        self.name
    }

    /// Get the name of this port (if known).
    pub fn name_str(&self) -> Option<&'static str> {
        self.name.to_name()
    }

    /// Get the GUID of the attached item, if any.
    pub fn item_guid(&self) -> Option<&CigGuid> {
        self.item_guid.as_ref()
    }

    /// Set the attached item GUID.
    pub fn set_item_guid(&mut self, guid: Option<CigGuid>) {
        self.item_guid = guid;
    }

    /// Get the child ports.
    pub fn children(&self) -> &[ItemPort] {
        &self.children
    }

    /// Get mutable access to child ports.
    pub fn children_mut(&mut self) -> &mut Vec<ItemPort> {
        &mut self.children
    }

    /// Add a child port.
    pub fn add_child(&mut self, child: ItemPort) {
        self.children.push(child);
    }

    /// Check if this port has an item attached.
    pub fn has_item(&self) -> bool {
        self.item_guid.is_some()
    }

    /// Check if this port has any children.
    pub fn has_children(&self) -> bool {
        !self.children.is_empty()
    }

    /// Find a child port by name hash.
    pub fn find_child(&self, name: NameHash) -> Option<&ItemPort> {
        self.children.iter().find(|c| c.name == name)
    }

    /// Find a child port by name hash (mutable).
    pub fn find_child_mut(&mut self, name: NameHash) -> Option<&mut ItemPort> {
        self.children.iter_mut().find(|c| c.name == name)
    }

    /// Recursively find a port by name hash in the tree.
    pub fn find_recursive(&self, name: NameHash) -> Option<&ItemPort> {
        if self.name == name {
            return Some(self);
        }
        for child in &self.children {
            if let Some(found) = child.find_recursive(name) {
                return Some(found);
            }
        }
        None
    }

    /// Get the total number of ports in this tree (including self).
    pub fn count(&self) -> usize {
        1 + self.children.iter().map(|c| c.count()).sum::<usize>()
    }

    /// Get the depth of this tree.
    pub fn depth(&self) -> usize {
        if self.children.is_empty() {
            1
        } else {
            1 + self.children.iter().map(|c| c.depth()).max().unwrap_or(0)
        }
    }

    /// Convert to bytes for writing.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        self.write_to(&mut bytes);
        bytes
    }

    /// Write to a byte buffer.
    fn write_to(&self, bytes: &mut Vec<u8>) {
        // Write name hash
        bytes.extend_from_slice(&self.name.value().to_le_bytes());

        // Write GUID (or zeros if none)
        match &self.item_guid {
            Some(guid) => bytes.extend_from_slice(guid.as_bytes()),
            None => bytes.extend_from_slice(&[0u8; 16]),
        }

        // Write child count
        bytes.extend_from_slice(&(self.children.len() as u32).to_le_bytes());

        // Write children recursively
        for child in &self.children {
            child.write_to(bytes);
        }
    }

    /// Iterate over all ports in the tree (pre-order traversal).
    pub fn iter(&self) -> ItemPortIter<'_> {
        ItemPortIter {
            stack: vec![self],
        }
    }
}

/// Iterator over all item ports in a tree.
pub struct ItemPortIter<'a> {
    stack: Vec<&'a ItemPort>,
}

impl<'a> Iterator for ItemPortIter<'a> {
    type Item = &'a ItemPort;

    fn next(&mut self) -> Option<Self::Item> {
        let port = self.stack.pop()?;
        // Push children in reverse order so they're visited left-to-right
        for child in port.children.iter().rev() {
            self.stack.push(child);
        }
        Some(port)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_itemport_tree() {
        let mut root = ItemPort::new(NameHash::from_str("root"));
        root.add_child(ItemPort::new(NameHash::from_str("child1")));
        root.add_child(ItemPort::new(NameHash::from_str("child2")));

        assert_eq!(root.count(), 3);
        assert_eq!(root.depth(), 2);
        assert!(root.find_child(NameHash::from_str("child1")).is_some());
    }

    #[test]
    fn test_itemport_roundtrip() {
        let mut root = ItemPort::new(NameHash::from_str("head"));
        root.add_child(ItemPort::with_item(
            NameHash::from_str("helmet"),
            CigGuid::from_bytes([1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]),
        ));

        let bytes = root.to_bytes();
        let parsed = ItemPort::parse(&bytes).unwrap();

        assert_eq!(parsed.name().value(), root.name().value());
        assert_eq!(parsed.children().len(), 1);
        assert!(parsed.children()[0].has_item());
    }

    #[test]
    fn test_itemport_iter() {
        let mut root = ItemPort::new(NameHash::from_str("root"));
        let mut child = ItemPort::new(NameHash::from_str("child"));
        child.add_child(ItemPort::new(NameHash::from_str("grandchild")));
        root.add_child(child);

        let names: Vec<_> = root.iter().map(|p| p.name().value()).collect();
        assert_eq!(names.len(), 3);
    }
}
