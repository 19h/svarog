//! Material and texture data structures.
//!
//! Materials define the visual appearance of character parts. Each material
//! can have multiple sub-materials, each with textures and shader parameters.

use svarog_common::{BinaryReader, CigGuid};

use super::name_hash::NameHash;
use crate::Result;

/// RGBA color with floating-point components.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColorRgba {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl ColorRgba {
    /// Create a new color.
    pub fn new(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self { r, g, b, a }
    }

    /// Create a color from bytes (0-255 range).
    pub fn from_bytes(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self {
            r: r as f32 / 255.0,
            g: g as f32 / 255.0,
            b: b as f32 / 255.0,
            a: a as f32 / 255.0,
        }
    }

    /// Convert to byte representation.
    pub fn to_bytes(&self) -> [u8; 4] {
        [
            (self.r * 255.0).round() as u8,
            (self.g * 255.0).round() as u8,
            (self.b * 255.0).round() as u8,
            (self.a * 255.0).round() as u8,
        ]
    }

    /// Read from binary data (4 bytes RGBA).
    pub fn read(reader: &mut BinaryReader<'_>) -> Result<Self> {
        let r = reader.read_u8()?;
        let g = reader.read_u8()?;
        let b = reader.read_u8()?;
        let a = reader.read_u8()?;
        Ok(Self::from_bytes(r, g, b, a))
    }

    /// Create a white color.
    pub const fn white() -> Self {
        Self {
            r: 1.0,
            g: 1.0,
            b: 1.0,
            a: 1.0,
        }
    }

    /// Create a black color.
    pub const fn black() -> Self {
        Self {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: 1.0,
        }
    }

    /// Create a transparent color.
    pub const fn transparent() -> Self {
        Self {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: 0.0,
        }
    }
}

impl Default for ColorRgba {
    fn default() -> Self {
        Self::white()
    }
}

/// A material parameter with a typed value.
#[derive(Debug, Clone)]
pub enum MaterialParam {
    /// Floating-point parameter.
    Float { name: NameHash, value: f32 },
    /// Color parameter.
    Color { name: NameHash, value: ColorRgba },
}

impl MaterialParam {
    /// Get the name hash of this parameter.
    pub fn name(&self) -> NameHash {
        match self {
            MaterialParam::Float { name, .. } => *name,
            MaterialParam::Color { name, .. } => *name,
        }
    }

    /// Get the float value if this is a float parameter.
    pub fn as_float(&self) -> Option<f32> {
        match self {
            MaterialParam::Float { value, .. } => Some(*value),
            _ => None,
        }
    }

    /// Get the color value if this is a color parameter.
    pub fn as_color(&self) -> Option<ColorRgba> {
        match self {
            MaterialParam::Color { value, .. } => Some(*value),
            _ => None,
        }
    }
}

/// A texture reference.
#[derive(Debug, Clone)]
pub struct Texture {
    /// The type of texture (diffuse, normal, specular, etc.).
    pub texture_type: NameHash,
    /// The texture resource path or identifier.
    pub path: String,
}

impl Texture {
    /// Create a new texture reference.
    pub fn new(texture_type: NameHash, path: impl Into<String>) -> Self {
        Self {
            texture_type,
            path: path.into(),
        }
    }

    /// Read a texture from binary data.
    pub fn read(reader: &mut BinaryReader<'_>) -> Result<Self> {
        let texture_type = NameHash::from_raw(reader.read_u32()?);
        let path_len = reader.read_u32()? as usize;
        let path_bytes = reader.read_bytes(path_len)?;
        let path = String::from_utf8_lossy(path_bytes).into_owned();

        Ok(Self { texture_type, path })
    }

    /// Write to bytes.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&self.texture_type.value().to_le_bytes());
        bytes.extend_from_slice(&(self.path.len() as u32).to_le_bytes());
        bytes.extend_from_slice(self.path.as_bytes());
        bytes
    }
}

/// A sub-material within a material.
#[derive(Debug, Clone)]
pub struct SubMaterial {
    /// The name hash of this sub-material.
    name: NameHash,
    /// Textures used by this sub-material.
    textures: Vec<Texture>,
    /// Float parameters.
    float_params: Vec<(NameHash, f32)>,
    /// Color parameters.
    color_params: Vec<(NameHash, ColorRgba)>,
}

impl SubMaterial {
    /// Create a new empty sub-material.
    pub fn new(name: NameHash) -> Self {
        Self {
            name,
            textures: Vec::new(),
            float_params: Vec::new(),
            color_params: Vec::new(),
        }
    }

    /// Read a sub-material from binary data.
    pub fn read(reader: &mut BinaryReader<'_>) -> Result<Self> {
        let name = NameHash::from_raw(reader.read_u32()?);

        // Read textures
        let texture_count = reader.read_u32()? as usize;
        let mut textures = Vec::with_capacity(texture_count);
        for _ in 0..texture_count {
            textures.push(Texture::read(reader)?);
        }

        // Read float parameters
        let float_count = reader.read_u32()? as usize;
        let mut float_params = Vec::with_capacity(float_count);
        for _ in 0..float_count {
            let param_name = NameHash::from_raw(reader.read_u32()?);
            let value = reader.read_f32()?;
            float_params.push((param_name, value));
        }

        // Read color parameters
        let color_count = reader.read_u32()? as usize;
        let mut color_params = Vec::with_capacity(color_count);
        for _ in 0..color_count {
            let param_name = NameHash::from_raw(reader.read_u32()?);
            let color = ColorRgba::read(reader)?;
            color_params.push((param_name, color));
        }

        Ok(Self {
            name,
            textures,
            float_params,
            color_params,
        })
    }

    /// Get the name hash.
    pub fn name(&self) -> NameHash {
        self.name
    }

    /// Get the textures.
    pub fn textures(&self) -> &[Texture] {
        &self.textures
    }

    /// Get mutable access to textures.
    pub fn textures_mut(&mut self) -> &mut Vec<Texture> {
        &mut self.textures
    }

    /// Get the float parameters.
    pub fn float_params(&self) -> &[(NameHash, f32)] {
        &self.float_params
    }

    /// Get the color parameters.
    pub fn color_params(&self) -> &[(NameHash, ColorRgba)] {
        &self.color_params
    }

    /// Add a texture.
    pub fn add_texture(&mut self, texture: Texture) {
        self.textures.push(texture);
    }

    /// Add a float parameter.
    pub fn add_float_param(&mut self, name: NameHash, value: f32) {
        self.float_params.push((name, value));
    }

    /// Add a color parameter.
    pub fn add_color_param(&mut self, name: NameHash, value: ColorRgba) {
        self.color_params.push((name, value));
    }

    /// Write to bytes.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::new();

        bytes.extend_from_slice(&self.name.value().to_le_bytes());

        // Write textures
        bytes.extend_from_slice(&(self.textures.len() as u32).to_le_bytes());
        for texture in &self.textures {
            bytes.extend_from_slice(&texture.to_bytes());
        }

        // Write float parameters
        bytes.extend_from_slice(&(self.float_params.len() as u32).to_le_bytes());
        for (name, value) in &self.float_params {
            bytes.extend_from_slice(&name.value().to_le_bytes());
            bytes.extend_from_slice(&value.to_le_bytes());
        }

        // Write color parameters
        bytes.extend_from_slice(&(self.color_params.len() as u32).to_le_bytes());
        for (name, color) in &self.color_params {
            bytes.extend_from_slice(&name.value().to_le_bytes());
            bytes.extend_from_slice(&color.to_bytes());
        }

        bytes
    }
}

/// A material definition.
#[derive(Debug, Clone)]
pub struct Material {
    /// The name hash of this material.
    name: NameHash,
    /// The GUID of this material.
    guid: CigGuid,
    /// Additional parameters (legacy field).
    additional_params: Vec<u8>,
    /// Sub-materials.
    sub_materials: Vec<SubMaterial>,
}

impl Material {
    /// Create a new empty material.
    pub fn new(name: NameHash, guid: CigGuid) -> Self {
        Self {
            name,
            guid,
            additional_params: Vec::new(),
            sub_materials: Vec::new(),
        }
    }

    /// Read a material from binary data.
    pub fn read(reader: &mut BinaryReader<'_>) -> Result<Self> {
        let name = NameHash::from_raw(reader.read_u32()?);
        let guid_bytes = reader.read_bytes(16)?;
        let guid = CigGuid::from_bytes(guid_bytes.try_into().unwrap());

        // Read additional params
        let params_len = reader.read_u32()? as usize;
        let additional_params = reader.read_bytes(params_len)?.to_vec();

        // Read sub-materials
        let sub_count = reader.read_u32()? as usize;
        let mut sub_materials = Vec::with_capacity(sub_count);
        for _ in 0..sub_count {
            sub_materials.push(SubMaterial::read(reader)?);
        }

        Ok(Self {
            name,
            guid,
            additional_params,
            sub_materials,
        })
    }

    /// Get the name hash.
    pub fn name(&self) -> NameHash {
        self.name
    }

    /// Get the GUID.
    pub fn guid(&self) -> &CigGuid {
        &self.guid
    }

    /// Get the additional parameters.
    pub fn additional_params(&self) -> &[u8] {
        &self.additional_params
    }

    /// Get the sub-materials.
    pub fn sub_materials(&self) -> &[SubMaterial] {
        &self.sub_materials
    }

    /// Get mutable access to sub-materials.
    pub fn sub_materials_mut(&mut self) -> &mut Vec<SubMaterial> {
        &mut self.sub_materials
    }

    /// Add a sub-material.
    pub fn add_sub_material(&mut self, sub: SubMaterial) {
        self.sub_materials.push(sub);
    }

    /// Write to bytes.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::new();

        bytes.extend_from_slice(&self.name.value().to_le_bytes());
        bytes.extend_from_slice(self.guid.as_bytes());

        // Write additional params
        bytes.extend_from_slice(&(self.additional_params.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&self.additional_params);

        // Write sub-materials
        bytes.extend_from_slice(&(self.sub_materials.len() as u32).to_le_bytes());
        for sub in &self.sub_materials {
            bytes.extend_from_slice(&sub.to_bytes());
        }

        bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_color_rgba() {
        let color = ColorRgba::from_bytes(255, 128, 0, 255);
        assert!((color.r - 1.0).abs() < 0.01);
        assert!((color.g - 0.5).abs() < 0.01);
        assert!((color.b - 0.0).abs() < 0.01);
        assert!((color.a - 1.0).abs() < 0.01);

        let bytes = color.to_bytes();
        assert_eq!(bytes[0], 255);
        assert_eq!(bytes[1], 128);
        assert_eq!(bytes[2], 0);
        assert_eq!(bytes[3], 255);
    }

    #[test]
    fn test_sub_material() {
        let mut sub = SubMaterial::new(NameHash::from_str("skin"));
        sub.add_float_param(NameHash::from_str("roughness"), 0.5);
        sub.add_color_param(NameHash::from_str("tint"), ColorRgba::new(1.0, 0.8, 0.6, 1.0));

        assert_eq!(sub.float_params().len(), 1);
        assert_eq!(sub.color_params().len(), 1);
    }
}
