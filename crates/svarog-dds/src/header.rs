//! DDS header structures.

use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout};

/// DDS file header.
#[derive(Debug, Clone, Copy, FromBytes, IntoBytes, Immutable, KnownLayout)]
#[repr(C, packed)]
pub struct DdsHeader {
    /// Header size (should be 124).
    pub size: u32,
    /// Header flags.
    pub flags: u32,
    /// Image height.
    pub height: u32,
    /// Image width.
    pub width: u32,
    /// Pitch or linear size.
    pub pitch_or_linear_size: u32,
    /// Depth (for volume textures).
    pub depth: u32,
    /// Number of mipmap levels.
    pub mipmap_count: u32,
    /// Reserved.
    pub reserved1: [u32; 11],
    /// Pixel format.
    pub pixel_format: DdsPixelFormat,
    /// Surface capabilities.
    pub caps: u32,
    /// Surface capabilities 2.
    pub caps2: u32,
    /// Surface capabilities 3.
    pub caps3: u32,
    /// Surface capabilities 4.
    pub caps4: u32,
    /// Reserved.
    pub reserved2: u32,
}

impl DdsHeader {
    /// Expected header size.
    pub const SIZE: u32 = 124;

    /// Check if this is a DX10 extended header.
    pub fn is_dx10(&self) -> bool {
        self.pixel_format.four_cc == FourCC::DX10
    }
}

/// DDS pixel format.
#[derive(Debug, Clone, Copy, FromBytes, IntoBytes, Immutable, KnownLayout)]
#[repr(C, packed)]
pub struct DdsPixelFormat {
    /// Structure size (should be 32).
    pub size: u32,
    /// Pixel format flags.
    pub flags: u32,
    /// Four-character code for compression.
    pub four_cc: FourCC,
    /// Number of bits per pixel (for uncompressed).
    pub rgb_bit_count: u32,
    /// Red bit mask.
    pub r_bit_mask: u32,
    /// Green bit mask.
    pub g_bit_mask: u32,
    /// Blue bit mask.
    pub b_bit_mask: u32,
    /// Alpha bit mask.
    pub a_bit_mask: u32,
}

/// Four-character code for compression type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, FromBytes, IntoBytes, Immutable, KnownLayout)]
#[repr(transparent)]
pub struct FourCC(pub [u8; 4]);

impl FourCC {
    /// DXT1 compression.
    pub const DXT1: Self = Self(*b"DXT1");
    /// DXT3 compression.
    pub const DXT3: Self = Self(*b"DXT3");
    /// DXT5 compression.
    pub const DXT5: Self = Self(*b"DXT5");
    /// DX10 extended header.
    pub const DX10: Self = Self(*b"DX10");
    /// BC4U compression.
    pub const BC4U: Self = Self(*b"BC4U");
    /// BC4S compression.
    pub const BC4S: Self = Self(*b"BC4S");
    /// BC5U compression.
    pub const BC5U: Self = Self(*b"BC5U");
    /// BC5S compression.
    pub const BC5S: Self = Self(*b"BC5S");
}

/// DX10 extended header.
#[derive(Debug, Clone, Copy, FromBytes, IntoBytes, Immutable, KnownLayout)]
#[repr(C, packed)]
pub struct DdsHeaderDxt10 {
    /// DXGI format.
    pub dxgi_format: u32,
    /// Resource dimension.
    pub resource_dimension: u32,
    /// Misc flags.
    pub misc_flag: u32,
    /// Array size.
    pub array_size: u32,
    /// Misc flags 2.
    pub misc_flags2: u32,
}

impl DdsHeaderDxt10 {
    /// BC4 UNORM format.
    pub const BC4_UNORM: u32 = 80;
    /// BC4 SNORM format.
    pub const BC4_SNORM: u32 = 81;
    /// BC6H UF16 format.
    pub const BC6H_UF16: u32 = 95;
}

/// Get the block size for a compression format.
pub fn block_size(four_cc: FourCC, dx10_format: Option<u32>) -> usize {
    // BC4 and BC1 use 8 bytes per block, others use 16
    match four_cc {
        FourCC::DXT1 | FourCC::BC4U | FourCC::BC4S => 8,
        _ => {
            if let Some(fmt) = dx10_format {
                if fmt == DdsHeaderDxt10::BC4_UNORM || fmt == DdsHeaderDxt10::BC4_SNORM {
                    return 8;
                }
            }
            16
        }
    }
}

/// Calculate the size in bytes of a mipmap level.
pub fn mipmap_size(width: u32, height: u32, block_size: usize) -> usize {
    let blocks_x = ((width as usize) + 3) / 4;
    let blocks_y = ((height as usize) + 3) / 4;
    blocks_x.max(1) * blocks_y.max(1) * block_size
}
