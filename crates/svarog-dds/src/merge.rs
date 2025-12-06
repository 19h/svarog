//! DDS mipmap merging.

use std::fs;
use std::path::Path;

use svarog_common::BinaryReader;

use crate::header::{block_size, mipmap_size, DdsHeader, DdsHeaderDxt10};
use crate::{Error, Result, DDS_MAGIC};

/// Merge a split DDS file into a complete DDS.
///
/// This function looks for split mipmap files (`.dds.0`, `.dds.1`, etc.)
/// and merges them with the base DDS file.
///
/// # Arguments
///
/// * `path` - Path to the base DDS file (without the `.N` suffix)
///
/// # Returns
///
/// The merged DDS file as a byte vector.
pub fn merge_dds<P: AsRef<Path>>(path: P) -> Result<Vec<u8>> {
    let path = path.as_ref();
    let base_path = path.to_string_lossy();

    // Read the base file
    let base_data = fs::read(path)?;

    // Find split files
    let mut split_files: Vec<(u8, Vec<u8>)> = Vec::new();

    for i in 0..=9 {
        let split_path = format!("{}.{}", base_path, i);
        if let Ok(data) = fs::read(&split_path) {
            split_files.push((i, data));
        }
    }

    // If no split files, return the base file as-is
    if split_files.is_empty() {
        return Ok(base_data);
    }

    // Sort by number descending (largest mipmap first)
    split_files.sort_by(|a, b| b.0.cmp(&a.0));

    merge_dds_data(&base_data, &split_files)
}

/// Merge DDS data from base file and split mipmap files.
pub fn merge_dds_data(base_data: &[u8], split_files: &[(u8, Vec<u8>)]) -> Result<Vec<u8>> {
    if base_data.len() < 4 {
        return Err(Error::InvalidHeader("file too small".into()));
    }

    // Verify magic
    let magic: [u8; 4] = base_data[..4].try_into().unwrap();
    if &magic != DDS_MAGIC {
        return Err(Error::InvalidMagic(magic));
    }

    // Parse header
    let mut reader = BinaryReader::new(&base_data[4..]);
    let header: DdsHeader = reader.read_struct()?;

    let has_dx10 = header.is_dx10();
    let dx10_header: Option<DdsHeaderDxt10> = if has_dx10 {
        Some(reader.read_struct()?)
    } else {
        None
    };

    let header_size = 4 + std::mem::size_of::<DdsHeader>()
        + if has_dx10 {
            std::mem::size_of::<DdsHeaderDxt10>()
        } else {
            0
        };

    // Get the small mipmaps from the base file
    let small_mipmaps = &base_data[header_size..];

    // Calculate mipmap sizes
    let mip_sizes = calculate_mipmap_sizes(&header, dx10_header.as_ref());

    // Determine block size (used for alignment in future optimization)
    let _blk_size = block_size(header.pixel_format.four_cc, dx10_header.map(|h| h.dxgi_format));

    // Calculate the number of faces (for cubemaps)
    let num_faces = if split_files.is_empty() {
        1
    } else {
        let largest_mip_size = mip_sizes[0];
        split_files[0].1.len() / largest_mip_size
    };

    // Build output
    let mut output = Vec::with_capacity(base_data.len() * 2);

    // Write header
    output.extend_from_slice(&base_data[..header_size]);

    // Write mipmaps for each face
    let split_mip_count = split_files.len();
    let mut small_offset = 0;

    for face in 0..num_faces {
        for (mip_level, &mip_size) in mip_sizes.iter().enumerate() {
            if mip_level < split_mip_count {
                // Use split file data
                let split_data = &split_files[mip_level].1;
                let offset = face * mip_size;
                let end = offset + mip_size;

                if end <= split_data.len() {
                    output.extend_from_slice(&split_data[offset..end]);
                } else {
                    // Fallback to small mipmaps if split file is incomplete
                    output.extend_from_slice(&small_mipmaps[small_offset..small_offset + mip_size]);
                    small_offset += mip_size;
                }
            } else {
                // Use small mipmap data from base file
                output.extend_from_slice(&small_mipmaps[small_offset..small_offset + mip_size]);
                small_offset += mip_size;
            }
        }
    }

    Ok(output)
}

/// Calculate the sizes of each mipmap level.
fn calculate_mipmap_sizes(header: &DdsHeader, dx10: Option<&DdsHeaderDxt10>) -> Vec<usize> {
    let mut sizes = Vec::with_capacity(header.mipmap_count as usize);
    let blk_size = block_size(header.pixel_format.four_cc, dx10.map(|h| h.dxgi_format));

    for i in 0..header.mipmap_count {
        let width = (header.width >> i).max(1);
        let height = (header.height >> i).max(1);
        sizes.push(mipmap_size(width, height, blk_size));
    }

    sizes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mipmap_size_calculation() {
        // 4x4 block minimum
        assert_eq!(mipmap_size(1, 1, 16), 16);
        assert_eq!(mipmap_size(4, 4, 16), 16);
        assert_eq!(mipmap_size(8, 8, 16), 64);
        assert_eq!(mipmap_size(1024, 1024, 16), 1024 * 1024);
    }
}
