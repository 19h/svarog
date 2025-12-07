//! Parse XML text into CryXmlB binary format.

use quick_xml::events::Event;
use quick_xml::Reader;

use crate::builder::{BuilderNode, CryXmlBuilder};
use crate::{Error, Result};

impl CryXmlBuilder {
    /// Parse XML text and create a builder that can produce CryXmlB bytes.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use svarog_cryxml::builder::CryXmlBuilder;
    ///
    /// let xml = r#"<?xml version="1.0"?>
    /// <Material Name="TestMaterial">
    ///     <Textures>
    ///         <Texture Map="Diffuse" File="test.dds"/>
    ///     </Textures>
    /// </Material>"#;
    ///
    /// let builder = CryXmlBuilder::from_xml(xml).unwrap();
    /// let bytes = builder.build().unwrap();
    /// ```
    pub fn from_xml(xml: &str) -> Result<Self> {
        let root = parse_xml_to_node(xml)?;
        Ok(Self::new(root))
    }

    /// Parse XML bytes and create a builder.
    pub fn from_xml_bytes(xml: &[u8]) -> Result<Self> {
        let xml_str = std::str::from_utf8(xml).map_err(Error::Utf8)?;
        Self::from_xml(xml_str)
    }
}

/// Parse XML text into a BuilderNode tree.
fn parse_xml_to_node(xml: &str) -> Result<BuilderNode> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut stack: Vec<BuilderNode> = Vec::new();
    let mut root: Option<BuilderNode> = None;

    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                let tag = String::from_utf8_lossy(e.name().as_ref()).into_owned();
                let mut node = BuilderNode::new(tag);

                // Parse attributes
                for attr in e.attributes().flatten() {
                    let key = String::from_utf8_lossy(attr.key.as_ref()).into_owned();
                    let value = String::from_utf8_lossy(&attr.value).into_owned();
                    node.attributes.push((key, value));
                }

                stack.push(node);
            }
            Ok(Event::Empty(e)) => {
                // Self-closing element
                let tag = String::from_utf8_lossy(e.name().as_ref()).into_owned();
                let mut node = BuilderNode::new(tag);

                for attr in e.attributes().flatten() {
                    let key = String::from_utf8_lossy(attr.key.as_ref()).into_owned();
                    let value = String::from_utf8_lossy(&attr.value).into_owned();
                    node.attributes.push((key, value));
                }

                if let Some(parent) = stack.last_mut() {
                    parent.children.push(node);
                } else {
                    root = Some(node);
                }
            }
            Ok(Event::End(_)) => {
                if let Some(node) = stack.pop() {
                    if let Some(parent) = stack.last_mut() {
                        parent.children.push(node);
                    } else {
                        root = Some(node);
                    }
                }
            }
            Ok(Event::Text(e)) => {
                if let Some(node) = stack.last_mut() {
                    let text = e.unescape().map_err(|e| Error::Xml(e.to_string()))?;
                    if !text.trim().is_empty() {
                        node.content = text.into_owned();
                    }
                }
            }
            Ok(Event::Eof) => break,
            Ok(_) => {} // Ignore other events (declarations, comments, etc.)
            Err(e) => return Err(Error::Xml(format!("XML parse error: {}", e))),
        }
    }

    root.ok_or_else(|| Error::Xml("No root element found in XML".to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CryXml;

    #[test]
    fn test_from_xml_simple() {
        let xml = r#"<Root version="1.0"/>"#;
        let builder = CryXmlBuilder::from_xml(xml).unwrap();
        let bytes = builder.build().unwrap();

        let parsed = CryXml::parse(&bytes).unwrap();
        let root = parsed.root().unwrap();
        assert_eq!(parsed.get_string(root.tag_string_offset).unwrap(), "Root");

        let attrs = parsed.node_attributes(root);
        assert_eq!(attrs.len(), 1);
        assert_eq!(parsed.get_string(attrs[0].key_string_offset).unwrap(), "version");
        assert_eq!(parsed.get_string(attrs[0].value_string_offset).unwrap(), "1.0");
    }

    #[test]
    fn test_from_xml_with_declaration() {
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<Material Name="TestMaterial">
    <Textures>
        <Texture Map="Diffuse" File="test.dds"/>
    </Textures>
</Material>"#;

        let builder = CryXmlBuilder::from_xml(xml).unwrap();
        let bytes = builder.build().unwrap();

        let parsed = CryXml::parse(&bytes).unwrap();
        let root = parsed.root().unwrap();
        assert_eq!(parsed.get_string(root.tag_string_offset).unwrap(), "Material");
        let child_count = root.child_count;
        assert_eq!(child_count, 1);
    }

    #[test]
    fn test_from_xml_nested() {
        let xml = r#"<A>
            <B attr="1">
                <C/>
                <D attr="2"/>
            </B>
            <E/>
        </A>"#;

        let builder = CryXmlBuilder::from_xml(xml).unwrap();
        let bytes = builder.build().unwrap();

        let parsed = CryXml::parse(&bytes).unwrap();
        let root = parsed.root().unwrap();
        assert_eq!(parsed.get_string(root.tag_string_offset).unwrap(), "A");
        let root_child_count = root.child_count;
        assert_eq!(root_child_count, 2);

        let children: Vec<_> = parsed.children(root).collect();
        assert_eq!(parsed.get_string(children[0].tag_string_offset).unwrap(), "B");
        let b_child_count = children[0].child_count;
        assert_eq!(b_child_count, 2);
        assert_eq!(parsed.get_string(children[1].tag_string_offset).unwrap(), "E");
    }

    #[test]
    fn test_xml_round_trip() {
        // Create CryXmlB -> XML -> CryXmlB
        let original_xml = r#"<Config version="2.0" name="test">
            <Setting key="option1" value="enabled"/>
            <Setting key="option2" value="disabled"/>
        </Config>"#;

        // XML -> CryXmlB
        let builder1 = CryXmlBuilder::from_xml(original_xml).unwrap();
        let bytes1 = builder1.build().unwrap();

        // CryXmlB -> XML
        let parsed = CryXml::parse(&bytes1).unwrap();
        let xml_output = parsed.to_xml_string().unwrap();

        // XML -> CryXmlB again
        let builder2 = CryXmlBuilder::from_xml(&xml_output).unwrap();
        let bytes2 = builder2.build().unwrap();

        // Parse both and compare structure
        let parsed1 = CryXml::parse(&bytes1).unwrap();
        let parsed2 = CryXml::parse(&bytes2).unwrap();

        let root1 = parsed1.root().unwrap();
        let root2 = parsed2.root().unwrap();

        assert_eq!(
            parsed1.get_string(root1.tag_string_offset).unwrap(),
            parsed2.get_string(root2.tag_string_offset).unwrap()
        );
        // Copy from packed struct to avoid alignment issues
        let attr_count1 = root1.attribute_count;
        let attr_count2 = root2.attribute_count;
        let child_count1 = root1.child_count;
        let child_count2 = root2.child_count;
        assert_eq!(attr_count1, attr_count2);
        assert_eq!(child_count1, child_count2);
    }

    #[test]
    fn test_from_xml_empty() {
        let xml = "";
        let result = CryXmlBuilder::from_xml(xml);
        assert!(result.is_err());
    }

    #[test]
    fn test_from_xml_text_content() {
        let xml = r#"<Root><Child>Hello World</Child></Root>"#;
        let builder = CryXmlBuilder::from_xml(xml).unwrap();
        let bytes = builder.build().unwrap();

        let parsed = CryXml::parse(&bytes).unwrap();
        let root = parsed.root().unwrap();
        let children: Vec<_> = parsed.children(root).collect();

        assert_eq!(children.len(), 1);
        let child = children[0];
        assert_eq!(parsed.get_string(child.content_string_offset).unwrap(), "Hello World");
    }
}
