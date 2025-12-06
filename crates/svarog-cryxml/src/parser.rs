//! CryXmlB parser.

use std::io::Write;

use svarog_common::BinaryReader;

use crate::{CryXmlAttribute, CryXmlHeader, CryXmlNode, Error, Result};

/// Parsed CryXmlB document.
///
/// This struct holds all the parsed data from a CryXmlB file and provides
/// methods to traverse and convert the document.
#[derive(Debug)]
pub struct CryXml {
    nodes: Vec<CryXmlNode>,
    child_indices: Vec<i32>,
    attributes: Vec<CryXmlAttribute>,
    string_data: Vec<u8>,
}

impl CryXml {
    /// Check if data is a CryXmlB file by checking the magic bytes.
    pub fn is_cryxml(data: &[u8]) -> bool {
        data.len() >= CryXmlHeader::MAGIC_LEN && &data[..CryXmlHeader::MAGIC_LEN] == CryXmlHeader::MAGIC
    }

    /// Parse a CryXmlB file from bytes.
    ///
    /// # Arguments
    ///
    /// * `data` - The raw bytes of the CryXmlB file
    ///
    /// # Returns
    ///
    /// A parsed `CryXml` document, or an error if parsing fails.
    pub fn parse(data: &[u8]) -> Result<Self> {
        // Check magic
        if !Self::is_cryxml(data) {
            return Err(Error::InvalidMagic {
                actual: data[..CryXmlHeader::MAGIC_LEN.min(data.len())].to_vec(),
            });
        }

        let mut reader = BinaryReader::new(&data[CryXmlHeader::MAGIC_LEN..]);
        let header: CryXmlHeader = reader.read_struct()?;

        // Read nodes
        let mut nodes = Vec::with_capacity(header.node_count as usize);
        for _ in 0..header.node_count {
            nodes.push(reader.read_struct::<CryXmlNode>()?);
        }

        // Read child indices
        let mut child_indices = Vec::with_capacity(header.child_count as usize);
        for _ in 0..header.child_count {
            child_indices.push(reader.read_i32()?);
        }

        // Read attributes
        let mut attributes = Vec::with_capacity(header.attribute_count as usize);
        for _ in 0..header.attribute_count {
            attributes.push(reader.read_struct::<CryXmlAttribute>()?);
        }

        // Read string data
        let string_data = reader.read_bytes(header.string_data_size as usize)?.to_vec();

        Ok(Self {
            nodes,
            child_indices,
            attributes,
            string_data,
        })
    }

    /// Get a string from the string table by offset.
    /// Uses SIMD-accelerated null-terminator search via memchr.
    pub fn get_string(&self, offset: u32) -> Result<&str> {
        let offset = offset as usize;
        if offset >= self.string_data.len() {
            return Err(Error::StringOffsetOutOfBounds {
                offset: offset as u32,
                size: self.string_data.len(),
            });
        }

        // Find null terminator using SIMD-accelerated memchr
        let end = svarog_common::memchr::memchr(0, &self.string_data[offset..])
            .unwrap_or(self.string_data.len() - offset);

        let slice = &self.string_data[offset..offset + end];

        // Handle empty strings
        if slice.is_empty() {
            return Ok("");
        }

        std::str::from_utf8(slice).map_err(Error::Utf8)
    }

    /// Get the root node.
    pub fn root(&self) -> Option<&CryXmlNode> {
        self.nodes.first()
    }

    /// Get a node by index.
    pub fn node(&self, index: usize) -> Option<&CryXmlNode> {
        self.nodes.get(index)
    }

    /// Get the children of a node.
    pub fn children(&self, node: &CryXmlNode) -> impl Iterator<Item = &CryXmlNode> {
        let start = node.first_child_index as usize;
        let end = start + node.child_count as usize;

        self.child_indices[start..end]
            .iter()
            .filter_map(|&idx| self.nodes.get(idx as usize))
    }

    /// Get the attributes of a node.
    pub fn node_attributes(&self, node: &CryXmlNode) -> &[CryXmlAttribute] {
        let start = node.first_attribute_index as usize;
        let end = start + node.attribute_count as usize;
        &self.attributes[start..end]
    }

    /// Convert to XML string.
    #[cfg(feature = "xml-output")]
    pub fn to_xml_string(&self) -> Result<String> {
        let mut output = Vec::new();
        self.write_xml(&mut output)?;
        String::from_utf8(output).map_err(|e| Error::Xml(e.to_string()))
    }

    /// Write XML to a writer.
    #[cfg(feature = "xml-output")]
    pub fn write_xml<W: Write>(&self, writer: &mut W) -> Result<()> {
        use quick_xml::events::{BytesDecl, Event};
        use quick_xml::Writer;

        let mut xml_writer = Writer::new_with_indent(writer, b' ', 2);

        // Write XML declaration
        xml_writer
            .write_event(Event::Decl(BytesDecl::new("1.0", Some("utf-8"), None)))
            .map_err(|e| Error::Xml(e.to_string()))?;

        // Write root and children
        if let Some(root) = self.root() {
            self.write_element(&mut xml_writer, root)?;
        }

        Ok(())
    }

    /// Write a single element and its children.
    #[cfg(feature = "xml-output")]
    fn write_element<W: Write>(
        &self,
        writer: &mut quick_xml::Writer<W>,
        node: &CryXmlNode,
    ) -> Result<()> {
        use quick_xml::events::{BytesEnd, BytesStart, Event};

        let tag_name = self.get_string(node.tag_string_offset)?;

        // Create start element
        let mut elem = BytesStart::new(tag_name);

        // Add attributes
        for attr in self.node_attributes(node) {
            let key = self.get_string(attr.key_string_offset)?;
            let value = self.get_string(attr.value_string_offset)?;

            // Skip xmlns attributes as they can cause issues
            if key.starts_with("xmlns") {
                continue;
            }

            elem.push_attribute((key, value));
        }

        if node.child_count == 0 {
            // Self-closing element
            writer
                .write_event(Event::Empty(elem))
                .map_err(|e| Error::Xml(e.to_string()))?;
        } else {
            // Element with children
            writer
                .write_event(Event::Start(elem))
                .map_err(|e| Error::Xml(e.to_string()))?;

            // Write children
            for child in self.children(node) {
                self.write_element(writer, child)?;
            }

            writer
                .write_event(Event::End(BytesEnd::new(tag_name)))
                .map_err(|e| Error::Xml(e.to_string()))?;
        }

        Ok(())
    }

    /// Enumerate all unique strings in the document.
    ///
    /// This is useful for CRC32C hash dictionary building.
    pub fn all_strings(&self) -> std::collections::HashSet<&str> {
        let mut strings = std::collections::HashSet::new();

        for node in &self.nodes {
            if let Ok(s) = self.get_string(node.tag_string_offset) {
                strings.insert(s);
            }
        }

        for attr in &self.attributes {
            if let Ok(s) = self.get_string(attr.key_string_offset) {
                strings.insert(s);
            }
            if let Ok(s) = self.get_string(attr.value_string_offset) {
                strings.insert(s);
            }
        }

        strings
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_cryxml() {
        assert!(CryXml::is_cryxml(b"CryXmlB\0extra data"));
        assert!(!CryXml::is_cryxml(b"NotCryXml"));
        assert!(!CryXml::is_cryxml(b"short"));
    }

    #[test]
    fn test_invalid_magic() {
        let result = CryXml::parse(b"InvalidMagic");
        assert!(matches!(result, Err(Error::InvalidMagic { .. })));
    }
}
