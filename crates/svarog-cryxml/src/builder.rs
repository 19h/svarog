//! Builder for constructing CryXmlB documents.
//!
//! This module provides a builder pattern for creating CryXmlB files
//! either programmatically or from XML text.

use std::collections::HashMap;

use crate::{CryXmlAttribute, CryXmlHeader, CryXmlNode, Error, Result};

/// A node being built, before final serialization.
#[derive(Debug, Clone)]
pub struct BuilderNode {
    /// Tag name of the element.
    pub tag: String,
    /// Text content (usually empty for CryXmlB).
    pub content: String,
    /// Attributes as key-value pairs.
    pub attributes: Vec<(String, String)>,
    /// Child nodes.
    pub children: Vec<BuilderNode>,
}

impl BuilderNode {
    /// Create a new builder node with the given tag name.
    pub fn new(tag: impl Into<String>) -> Self {
        Self {
            tag: tag.into(),
            content: String::new(),
            attributes: Vec::new(),
            children: Vec::new(),
        }
    }

    /// Set the text content of this node.
    pub fn content(mut self, content: impl Into<String>) -> Self {
        self.content = content.into();
        self
    }

    /// Add an attribute to this node.
    pub fn attr(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.attributes.push((key.into(), value.into()));
        self
    }

    /// Add a child node.
    pub fn child(mut self, child: BuilderNode) -> Self {
        self.children.push(child);
        self
    }

    /// Add multiple children.
    pub fn children(mut self, children: impl IntoIterator<Item = BuilderNode>) -> Self {
        self.children.extend(children);
        self
    }
}

/// Builder for constructing CryXmlB documents.
///
/// # Example
///
/// ```no_run
/// use svarog_cryxml::builder::{CryXmlBuilder, BuilderNode};
///
/// let root = BuilderNode::new("Material")
///     .attr("Name", "MyMaterial")
///     .child(BuilderNode::new("Textures")
///         .child(BuilderNode::new("Texture")
///             .attr("Map", "Diffuse")
///             .attr("File", "texture.dds")));
///
/// let builder = CryXmlBuilder::new(root);
/// let bytes = builder.build().unwrap();
/// ```
#[derive(Debug)]
pub struct CryXmlBuilder {
    root: BuilderNode,
}

impl CryXmlBuilder {
    /// Create a new builder with the given root node.
    pub fn new(root: BuilderNode) -> Self {
        Self { root }
    }

    /// Build the CryXmlB binary representation.
    pub fn build(&self) -> Result<Vec<u8>> {
        // Step 1: Collect all unique strings and build string table
        let mut string_table = StringTable::new();
        self.collect_strings(&self.root, &mut string_table);

        // Step 2: Flatten the tree into arrays
        let mut nodes: Vec<CryXmlNode> = Vec::new();
        let mut child_indices: Vec<i32> = Vec::new();
        let mut attributes: Vec<CryXmlAttribute> = Vec::new();

        self.flatten_node(&self.root, -1, &mut nodes, &mut child_indices, &mut attributes, &string_table)?;

        // Step 3: Calculate positions
        let header_size = std::mem::size_of::<CryXmlHeader>() as u32;
        let node_size = std::mem::size_of::<CryXmlNode>() as u32;
        let attr_size = std::mem::size_of::<CryXmlAttribute>() as u32;

        // Layout: Magic | Header | Nodes | ChildIndices | Attributes | StringData
        // (This matches the order used by the game's CryXmlB files)
        let magic_size = CryXmlHeader::MAGIC_LEN as u32;
        let node_table_position = magic_size + header_size;
        let node_table_size = nodes.len() as u32 * node_size;

        let child_table_position = node_table_position + node_table_size;
        let child_table_size = child_indices.len() as u32 * 4;

        let attribute_table_position = child_table_position + child_table_size;
        let attribute_table_size = attributes.len() as u32 * attr_size;

        let string_data_position = attribute_table_position + attribute_table_size;
        let string_data = string_table.into_bytes();
        let string_data_size = string_data.len() as u32;

        // Total size for xml_size field (everything after magic)
        let xml_size = header_size + node_table_size + child_table_size + attribute_table_size + string_data_size;

        // Step 4: Build header
        let header = CryXmlHeader {
            xml_size,
            node_table_position,
            node_count: nodes.len() as u32,
            attribute_table_position,
            attribute_count: attributes.len() as u32,
            child_table_position,
            child_count: child_indices.len() as u32,
            string_data_position,
            string_data_size,
        };

        // Step 5: Write everything
        let total_size = magic_size + xml_size;
        let mut output = Vec::with_capacity(total_size as usize);

        // Magic
        output.extend_from_slice(CryXmlHeader::MAGIC);

        // Header
        output.extend_from_slice(zerocopy::IntoBytes::as_bytes(&header));

        // Nodes
        for node in &nodes {
            output.extend_from_slice(zerocopy::IntoBytes::as_bytes(node));
        }

        // Child indices (before attributes to match game file layout)
        for idx in &child_indices {
            output.extend_from_slice(&idx.to_le_bytes());
        }

        // Attributes
        for attr in &attributes {
            output.extend_from_slice(zerocopy::IntoBytes::as_bytes(attr));
        }

        // String data
        output.extend_from_slice(&string_data);

        Ok(output)
    }

    /// Recursively collect all strings from the tree.
    fn collect_strings(&self, node: &BuilderNode, table: &mut StringTable) {
        table.add(&node.tag);
        table.add(&node.content);

        for (key, value) in &node.attributes {
            table.add(key);
            table.add(value);
        }

        for child in &node.children {
            self.collect_strings(child, table);
        }
    }

    /// Flatten a node and its children into the output arrays.
    /// Returns the index of the flattened node.
    fn flatten_node(
        &self,
        node: &BuilderNode,
        parent_index: i32,
        nodes: &mut Vec<CryXmlNode>,
        child_indices: &mut Vec<i32>,
        attributes: &mut Vec<CryXmlAttribute>,
        string_table: &StringTable,
    ) -> Result<i32> {
        let node_index = nodes.len() as i32;

        // Reserve slot for this node
        let first_attribute_index = attributes.len() as i32;
        let first_child_index = child_indices.len() as i32;

        // Add attributes
        for (key, value) in &node.attributes {
            attributes.push(CryXmlAttribute {
                key_string_offset: string_table.get(key).ok_or_else(|| {
                    Error::Xml(format!("string not found in table: {}", key))
                })?,
                value_string_offset: string_table.get(value).ok_or_else(|| {
                    Error::Xml(format!("string not found in table: {}", value))
                })?,
            });
        }

        // Create node (with placeholder child info)
        let cryxml_node = CryXmlNode {
            tag_string_offset: string_table.get(&node.tag).ok_or_else(|| {
                Error::Xml(format!("string not found in table: {}", node.tag))
            })?,
            content_string_offset: string_table.get(&node.content).ok_or_else(|| {
                Error::Xml(format!("string not found in table: {}", node.content))
            })?,
            attribute_count: node.attributes.len() as u16,
            child_count: node.children.len() as u16,
            parent_index,
            first_attribute_index,
            first_child_index,
            _reserved: 0,
        };
        nodes.push(cryxml_node);

        // Reserve space for child indices
        let child_index_start = child_indices.len();
        for _ in 0..node.children.len() {
            child_indices.push(0); // Placeholder
        }

        // Recursively add children
        for (i, child) in node.children.iter().enumerate() {
            let child_node_index = self.flatten_node(
                child,
                node_index,
                nodes,
                child_indices,
                attributes,
                string_table,
            )?;
            child_indices[child_index_start + i] = child_node_index;
        }

        Ok(node_index)
    }
}

/// Helper for building the string table.
#[derive(Debug)]
struct StringTable {
    strings: Vec<String>,
    offsets: HashMap<String, u32>,
    current_offset: u32,
}

impl StringTable {
    fn new() -> Self {
        Self {
            strings: Vec::new(),
            offsets: HashMap::new(),
            current_offset: 0,
        }
    }

    /// Add a string to the table if not already present.
    fn add(&mut self, s: &str) {
        if !self.offsets.contains_key(s) {
            self.offsets.insert(s.to_string(), self.current_offset);
            self.current_offset += s.len() as u32 + 1; // +1 for null terminator
            self.strings.push(s.to_string());
        }
    }

    /// Get the offset of a string.
    fn get(&self, s: &str) -> Option<u32> {
        self.offsets.get(s).copied()
    }

    /// Convert the string table to bytes.
    fn into_bytes(self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(self.current_offset as usize);
        for s in self.strings {
            bytes.extend_from_slice(s.as_bytes());
            bytes.push(0); // Null terminator
        }
        bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CryXml;

    #[test]
    fn test_builder_basic() {
        let root = BuilderNode::new("Root")
            .attr("version", "1.0");

        let builder = CryXmlBuilder::new(root);
        let bytes = builder.build().unwrap();

        // Should have magic
        assert_eq!(&bytes[..8], b"CryXmlB\0");

        // Should be parseable
        let parsed = CryXml::parse(&bytes).unwrap();
        let root = parsed.root().unwrap();
        assert_eq!(parsed.get_string(root.tag_string_offset).unwrap(), "Root");
    }

    #[test]
    fn test_builder_with_children() {
        let root = BuilderNode::new("Material")
            .attr("Name", "TestMaterial")
            .child(BuilderNode::new("Textures")
                .child(BuilderNode::new("Texture")
                    .attr("Map", "Diffuse")
                    .attr("File", "test.dds")));

        let builder = CryXmlBuilder::new(root);
        let bytes = builder.build().unwrap();

        let parsed = CryXml::parse(&bytes).unwrap();
        let root = parsed.root().unwrap();

        assert_eq!(parsed.get_string(root.tag_string_offset).unwrap(), "Material");
        // Copy from packed struct to avoid alignment issues
        let attr_count = root.attribute_count;
        let child_count = root.child_count;
        assert_eq!(attr_count, 1);
        assert_eq!(child_count, 1);

        // Check child
        let children: Vec<_> = parsed.children(root).collect();
        assert_eq!(children.len(), 1);
        assert_eq!(parsed.get_string(children[0].tag_string_offset).unwrap(), "Textures");
    }

    #[test]
    fn test_round_trip() {
        let original = BuilderNode::new("Config")
            .attr("version", "2.0")
            .attr("name", "test")
            .child(BuilderNode::new("Setting")
                .attr("key", "option1")
                .attr("value", "enabled"))
            .child(BuilderNode::new("Setting")
                .attr("key", "option2")
                .attr("value", "disabled"));

        let builder = CryXmlBuilder::new(original);
        let bytes = builder.build().unwrap();

        // Parse it back
        let parsed = CryXml::parse(&bytes).unwrap();

        // Verify structure
        let root = parsed.root().unwrap();
        assert_eq!(parsed.get_string(root.tag_string_offset).unwrap(), "Config");
        // Copy from packed struct to avoid alignment issues
        let attr_count = root.attribute_count;
        let child_count = root.child_count;
        assert_eq!(attr_count, 2);
        assert_eq!(child_count, 2);

        // Verify attributes
        let attrs = parsed.node_attributes(root);
        assert_eq!(parsed.get_string(attrs[0].key_string_offset).unwrap(), "version");
        assert_eq!(parsed.get_string(attrs[0].value_string_offset).unwrap(), "2.0");
        assert_eq!(parsed.get_string(attrs[1].key_string_offset).unwrap(), "name");
        assert_eq!(parsed.get_string(attrs[1].value_string_offset).unwrap(), "test");
    }

    #[test]
    fn test_real_file_round_trip() {
        // Test with a real CryXmlB file from the game
        let original_bytes = include_bytes!("../testdata/defaulttextures.xml");

        // Parse the original file
        let original = CryXml::parse(original_bytes).unwrap();
        let original_xml = original.to_xml_string().unwrap();

        // Convert XML back to CryXmlB
        let builder = CryXmlBuilder::from_xml(&original_xml).unwrap();
        let rebuilt_bytes = builder.build().unwrap();

        // Parse the rebuilt file
        let rebuilt = CryXml::parse(&rebuilt_bytes).unwrap();
        let rebuilt_xml = rebuilt.to_xml_string().unwrap();

        // The XML output should be identical
        assert_eq!(original_xml, rebuilt_xml);

        // Verify structure of original
        let root = original.root().unwrap();
        assert_eq!(original.get_string(root.tag_string_offset).unwrap(), "textures");
        let child_count = root.child_count;
        assert_eq!(child_count, 88); // 88 texture entries

        // Check first child has text content
        let children: Vec<_> = original.children(root).collect();
        assert_eq!(children.len(), 88);
        let first_child = children[0];
        assert_eq!(original.get_string(first_child.tag_string_offset).unwrap(), "entry");
        assert_eq!(
            original.get_string(first_child.content_string_offset).unwrap(),
            "EngineAssets/Textures/caustics_sampler.dds"
        );
    }
}
