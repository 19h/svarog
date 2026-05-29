//! End-to-end integration test for P4K v2 ("JiJi") support.
//!
//! The official CigDataPatcher writer is not available outside of
//! the closed-source toolchain, so this test builds a synthetic v2
//! archive in-memory using the exact byte layout pinned down in the
//! decompilation (see `zip::eocd_v2` and `zip::central_dir_v2` for
//! the field-by-field offset commentary).
//!
//! What this covers:
//!
//! 1. `P4kArchive::open` correctly detects v2 via the "JiJi" magic
//!    even when null padding precedes the trailer.
//! 2. The CDR + name table are parsed into entries with the right
//!    names, sizes and order.
//! 3. Payload reads for `Store` and `Zstd` entries decode correctly
//!    using the v2 "no local header" code path.

#![allow(clippy::field_reassign_with_default, clippy::too_many_arguments)]

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};
use svarog_p4k::zip::CompressionMethod;
use svarog_p4k::zip::{
    CentralDirectoryHeaderV2, Eocd2Record, CDR_V2_ENTRY_SIZE, EOCD_V2_MAGIC, EOCD_V2_SIZE,
    EOCD_V2_VERSION,
};
use svarog_p4k::P4kArchive;
use svarog_p4k::P4kVersion;
use svarog_p4k::{convert_v1_to_v2, P4kBuilder, P4kWriterOptions};

/// Compression method ID for raw / stored data.
const CM_STORE: u32 = 0;
/// Compression method ID for raw DEFLATE.
const CM_DEFLATE: u32 = 8;
/// Compression method ID for Zstandard (P4K-specific).
const CM_ZSTD: u32 = 100;
/// Compression method ID for the second zstd method accepted by the official code.
const CM_ZSTD_ALT: u32 = 93;
/// Compression method ID for Xbox/zlib-wrapped DEFLATE.
const CM_DEFLATE_ZLIB: u32 = 101;

/// Sector size used when building the synthetic archive. Real
/// archives use 0x1000 (4096); a smaller value here keeps the test
/// files compact while still exercising the EOCDR field.
const SECTOR_SIZE: u64 = 0x100;

fn p4k_crc32(data: &[u8]) -> u32 {
    svarog_common::crc::hash_bytes(data)
}

/// Build a synthetic v2 archive containing the supplied entries.
/// Returns the raw bytes ready to be written to disk.
///
/// Layout produced:
///
/// ```text
///   [payload bytes for entry 0..N, concatenated]
///   [install block: u64 bytes_already_written_to_disk per entry]
///   [zero padding to 64 KiB CDR boundary]
///   [CDR: N * 0xCC bytes]
///   [name table: central_directory_record_text_size bytes]
///   [zero padding to place EOCDR at end of aligned EOF buffer]
///   [EOCDR: 0xAF bytes including trailing "JiJi" magic]
///   [optional null padding]
/// ```
fn build_v2_archive(entries: &[V2EntrySpec], trailing_padding: usize) -> Vec<u8> {
    use zerocopy::IntoBytes;

    // 1. Lay out the payload area and compute per-entry data offsets.
    let mut payload: Vec<u8> = Vec::new();
    let mut data_offsets: Vec<u64> = Vec::with_capacity(entries.len());
    for e in entries {
        payload.resize(
            (payload.len() + SECTOR_SIZE as usize - 1) & !(SECTOR_SIZE as usize - 1),
            0,
        );
        data_offsets.push(payload.len() as u64);
        payload.extend_from_slice(&e.payload);
    }
    payload.resize(
        (payload.len() + SECTOR_SIZE as usize - 1) & !(SECTOR_SIZE as usize - 1),
        0,
    );

    // 2. Build the name table and remember each entry's offset.
    let mut name_table: Vec<u8> = Vec::new();
    let mut name_offsets: Vec<u64> = Vec::with_capacity(entries.len());
    for e in entries {
        name_offsets.push(name_table.len() as u64);
        name_table.extend_from_slice(e.name.as_bytes());
        name_table.push(0); // null terminator
    }
    let central_directory_record_text_size = name_table.len() as u64;

    // 3. Build the CDR.
    let mut cdr: Vec<u8> = Vec::with_capacity(entries.len() * CDR_V2_ENTRY_SIZE);
    for (i, e) in entries.iter().enumerate() {
        let mut entry_bytes = [0u8; CDR_V2_ENTRY_SIZE];
        let entry = CentralDirectoryHeaderV2 {
            compression_method: e.compression_method as u16,
            last_mod_file_time: 0,
            last_mod_file_date: 0,
            crc32: e.crc32,
            compressed_size: e.payload.len() as u64,
            uncompressed_size: e.uncompressed_size,
            offset_to_file_data: data_offsets[i],
            offset_to_filename: name_offsets[i],
            signature: [0u8; 128],
            encryption_flag: if e.encrypted { 1 } else { 0 },
            sha256: [0u8; 32],
        };
        entry_bytes.copy_from_slice(entry.as_bytes());
        cdr.extend_from_slice(&entry_bytes);
    }

    // 4. Assemble the file. The install block follows payloads.
    let mut file: Vec<u8> = Vec::new();
    file.extend_from_slice(&payload);
    let end_of_payload_offset = file.len() as u64;
    for e in entries {
        file.extend_from_slice(&(e.payload.len() as u64).to_le_bytes());
    }
    file.resize((file.len() + 0xFFFF) & !0xFFFF, 0);
    let cdr_start = file.len() as u64;
    file.extend_from_slice(&cdr);
    let name_table_start = file.len() as u64;
    file.extend_from_slice(&name_table);

    // 5. Build and append the EOCDR.
    let mut eocdr_bytes = [0u8; EOCD_V2_SIZE];
    let eocdr = Eocd2Record {
        number_of_entries: entries.len() as u64,
        number_of_extension_entries: 0,
        central_directory_record_offset: cdr_start,
        central_directory_record_size: cdr.len() as u64,
        central_directory_record_extension_size: 0,
        central_directory_record_text_offset: name_table_start,
        central_directory_record_text_size,
        central_directory_record_extension_text_size: 0,
        end_of_payload_offset,
        num_freelist_blocks: 0,
        trie_cache_offset: 0,
        trie_cache_size: 0,
        physical_sector_size: SECTOR_SIZE,
        status_flags: 0,
        manifest_sha256: [0u8; 64],
        version: EOCD_V2_VERSION,
        magic: EOCD_V2_MAGIC,
    };
    eocdr_bytes.copy_from_slice(eocdr.as_bytes());
    let eof_used = cdr.len() + name_table.len() + EOCD_V2_SIZE;
    let eof_aligned = (eof_used + SECTOR_SIZE as usize - 1) & !(SECTOR_SIZE as usize - 1);
    file.extend(std::iter::repeat(0u8).take(eof_aligned - eof_used));
    file.extend_from_slice(&eocdr_bytes);

    // 6. Optional trailing null padding (mirrors the ~500MB padding
    //    real P4K files have at EOF). The reader must scan past this
    //    via `find_content_end` before finding the magic.
    file.extend(std::iter::repeat(0u8).take(trailing_padding));

    file
}

fn insert_v2_name_table_slack_before_entry(
    mut file: Vec<u8>,
    entry_index: usize,
    slack: &[u8],
) -> Vec<u8> {
    let eocdr_start = file.len() - EOCD_V2_SIZE;
    let eocdr =
        <Eocd2Record as zerocopy::FromBytes>::read_from_bytes(&file[eocdr_start..]).unwrap();
    let cdr_start = eocdr.central_directory_record_offset as usize;
    let name_table_start = eocdr.central_directory_record_text_offset as usize;
    let name_table_end = name_table_start + eocdr.central_directory_record_text_size as usize;
    assert!(entry_index < eocdr.number_of_entries as usize);
    assert!(name_table_end + slack.len() <= eocdr_start);

    let target_entry = cdr_start + entry_index * CDR_V2_ENTRY_SIZE;
    let target_name_off = u64::from_le_bytes(
        file[target_entry + 0x22..target_entry + 0x2A]
            .try_into()
            .unwrap(),
    );
    let insert_at = name_table_start + target_name_off as usize;
    file.splice(insert_at..insert_at, slack.iter().copied());
    file.drain(eocdr_start..eocdr_start + slack.len());

    for index in 0..eocdr.number_of_entries as usize {
        let entry = cdr_start + index * CDR_V2_ENTRY_SIZE;
        let range = entry + 0x22..entry + 0x2A;
        let old = u64::from_le_bytes(file[range.clone()].try_into().unwrap());
        if old >= target_name_off {
            file[range].copy_from_slice(&(old + slack.len() as u64).to_le_bytes());
        }
    }
    let central_directory_record_text_size =
        eocdr.central_directory_record_text_size + slack.len() as u64;
    file[eocdr_start + 0x30..eocdr_start + 0x38]
        .copy_from_slice(&central_directory_record_text_size.to_le_bytes());
    file
}

struct V2EntrySpec {
    name: String,
    payload: Vec<u8>,
    uncompressed_size: u64,
    compression_method: u32,
    crc32: u32,
    encrypted: bool,
}

#[test]
fn detect_and_open_minimal_v2_archive() {
    let entries = vec![V2EntrySpec {
        name: "hello.txt".to_string(),
        payload: b"hello world".to_vec(),
        uncompressed_size: 11,
        compression_method: CM_STORE,
        crc32: p4k_crc32(b"hello world"),
        encrypted: false,
    }];
    let bytes = build_v2_archive(&entries, 0);

    let tmp = tempfile_path("svarog_p4k_v2_minimal.p4k");
    fs::write(&tmp, &bytes).unwrap();
    let archive = P4kArchive::open(&tmp).unwrap();
    assert_eq!(archive.version(), P4kVersion::V2);
    assert_eq!(archive.entry_count(), 1);
    let entry = archive.get(0).unwrap();
    assert_eq!(entry.name, "hello.txt");
    assert_eq!(entry.uncompressed_size, 11);
    assert_eq!(entry.compressed_size, 11);
    assert!(!entry.is_encrypted);

    let data = archive.read(&entry).unwrap();
    assert_eq!(data, b"hello world");

    let _ = fs::remove_file(&tmp);
}

#[test]
fn detect_v2_through_null_padding() {
    // Real-world v2 archives still have the same null-padding habit
    // as v1: bytes past EOF get zeroed out. The SIMD content-end
    // scan must hop over them to find the "JiJi" magic.
    let entries = vec![V2EntrySpec {
        name: "padded.bin".to_string(),
        payload: vec![0xAB; 64],
        uncompressed_size: 64,
        compression_method: CM_STORE,
        crc32: p4k_crc32(&[0xAB; 64]),
        encrypted: false,
    }];
    let bytes = build_v2_archive(&entries, 4096);

    let tmp = tempfile_path("svarog_p4k_v2_padded.p4k");
    fs::write(&tmp, &bytes).unwrap();
    let archive = P4kArchive::open(&tmp).unwrap();
    assert_eq!(archive.version(), P4kVersion::V2);
    let entry = archive.get(0).unwrap();
    let data = archive.read(&entry).unwrap();
    assert_eq!(data, vec![0xAB; 64]);

    let _ = fs::remove_file(&tmp);
}

#[test]
fn reader_rejects_v2_archive_with_unaligned_file_size() {
    let entries = vec![V2EntrySpec {
        name: "bad-v2-file-size.bin".to_string(),
        payload: b"payload".to_vec(),
        uncompressed_size: 7,
        compression_method: CM_STORE,
        crc32: p4k_crc32(b"payload"),
        encrypted: false,
    }];
    let bytes = build_v2_archive(&entries, 1);

    let tmp = tempfile_path("svarog_p4k_bad_v2_unaligned_size.p4k");
    fs::write(&tmp, &bytes).unwrap();

    let err = P4kArchive::open(&tmp).unwrap_err();
    assert!(
        err.to_string().contains("file size") && err.to_string().contains("not aligned"),
        "expected unaligned v2 file-size rejection, got {err}"
    );

    let _ = fs::remove_file(&tmp);
}

#[test]
fn multiple_entries_preserve_order_and_data() {
    let entries = vec![
        V2EntrySpec {
            name: "a/first.dat".to_string(),
            payload: b"AAAAA".to_vec(),
            uncompressed_size: 5,
            compression_method: CM_STORE,
            crc32: p4k_crc32(b"AAAAA"),
            encrypted: false,
        },
        V2EntrySpec {
            name: "b/second.dat".to_string(),
            payload: b"BBBBBBBB".to_vec(),
            uncompressed_size: 8,
            compression_method: CM_STORE,
            crc32: p4k_crc32(b"BBBBBBBB"),
            encrypted: false,
        },
        V2EntrySpec {
            name: "c/third.dat".to_string(),
            payload: b"CCC".to_vec(),
            uncompressed_size: 3,
            compression_method: CM_STORE,
            crc32: p4k_crc32(b"CCC"),
            encrypted: false,
        },
    ];
    let bytes = build_v2_archive(&entries, 0);

    let tmp = tempfile_path("svarog_p4k_v2_multi.p4k");
    fs::write(&tmp, &bytes).unwrap();
    let archive = P4kArchive::open(&tmp).unwrap();
    assert_eq!(archive.entry_count(), 3);

    // Names use the forward-slash form produced by cigNormalizePathInplace.
    let names: Vec<String> = archive.iter().map(|e| e.name.to_string()).collect();
    assert_eq!(
        names,
        vec![
            "a/first.dat".to_string(),
            "b/second.dat".to_string(),
            "c/third.dat".to_string(),
        ]
    );

    // Reads must yield each entry's distinct payload.
    assert_eq!(archive.read(&archive.get(0).unwrap()).unwrap(), b"AAAAA");
    assert_eq!(archive.read(&archive.get(1).unwrap()).unwrap(), b"BBBBBBBB");
    assert_eq!(archive.read(&archive.get(2).unwrap()).unwrap(), b"CCC");

    let _ = fs::remove_file(&tmp);
}

#[test]
fn zstd_compressed_entry_roundtrips() {
    let original: Vec<u8> = (0..512u16).map(|n| (n & 0xFF) as u8).collect();
    let compressed = zstd::stream::encode_all(&original[..], 3).unwrap();

    let entries = vec![V2EntrySpec {
        name: "blob.zst".to_string(),
        payload: compressed,
        uncompressed_size: original.len() as u64,
        compression_method: CM_ZSTD,
        crc32: p4k_crc32(&original),
        encrypted: false,
    }];
    let bytes = build_v2_archive(&entries, 0);

    let tmp = tempfile_path("svarog_p4k_v2_zstd.p4k");
    fs::write(&tmp, &bytes).unwrap();
    let archive = P4kArchive::open(&tmp).unwrap();
    let entry = archive.get(0).unwrap();
    assert_eq!(entry.uncompressed_size, original.len() as u64);
    let decoded = archive.read(&entry).unwrap();
    assert_eq!(decoded, original);

    let _ = fs::remove_file(&tmp);
}

#[test]
fn alternate_zstd_method_entry_roundtrips() {
    let original: Vec<u8> = (0..700u16).map(|n| ((n * 17) & 0xFF) as u8).collect();
    let compressed = zstd::stream::encode_all(&original[..], 3).unwrap();

    let entries = vec![V2EntrySpec {
        name: "blob-alt.zst".to_string(),
        payload: compressed,
        uncompressed_size: original.len() as u64,
        compression_method: CM_ZSTD_ALT,
        crc32: p4k_crc32(&original),
        encrypted: false,
    }];
    let bytes = build_v2_archive(&entries, 0);

    let tmp = tempfile_path("svarog_p4k_v2_zstd_alt.p4k");
    fs::write(&tmp, &bytes).unwrap();
    let archive = P4kArchive::open(&tmp).unwrap();
    let entry = archive.get(0).unwrap();
    assert_eq!(entry.compression_method, CompressionMethod::ZstdDeprecated);
    assert_eq!(archive.read(&entry).unwrap(), original);

    let _ = fs::remove_file(&tmp);
}

#[test]
fn zlib_wrapped_deflate_method_entry_roundtrips() {
    let original: Vec<u8> = (0..384u16).map(|n| ((n * 31) & 0xFF) as u8).collect();
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::best());
    encoder.write_all(&original).unwrap();
    let compressed = encoder.finish().unwrap();

    let entries = vec![V2EntrySpec {
        name: "xbox-deflate.bin".to_string(),
        payload: compressed,
        uncompressed_size: original.len() as u64,
        compression_method: CM_DEFLATE_ZLIB,
        crc32: p4k_crc32(&original),
        encrypted: false,
    }];
    let bytes = build_v2_archive(&entries, 0);

    let tmp = tempfile_path("svarog_p4k_v2_zlib_deflate.p4k");
    fs::write(&tmp, &bytes).unwrap();
    let archive = P4kArchive::open(&tmp).unwrap();
    let entry = archive.get(0).unwrap();
    assert_eq!(entry.compression_method, CompressionMethod::DeflateZlib);
    assert_eq!(archive.read(&entry).unwrap(), original);

    let _ = fs::remove_file(&tmp);
}

#[test]
fn compressed_entry_uncompressed_size_mismatch_is_rejected() {
    let original = b"metadata size must match decompressed bytes";
    let mut deflate_encoder =
        flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::best());
    deflate_encoder.write_all(original).unwrap();
    let deflate = deflate_encoder.finish().unwrap();

    let zstd = zstd::stream::encode_all(&original[..], 3).unwrap();

    let mut zlib_encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::best());
    zlib_encoder.write_all(original).unwrap();
    let zlib = zlib_encoder.finish().unwrap();

    for (name, method, payload) in [
        ("bad-deflate.bin", CM_DEFLATE, deflate),
        ("bad-zstd.bin", CM_ZSTD, zstd),
        ("bad-zlib.bin", CM_DEFLATE_ZLIB, zlib),
    ] {
        let tmp = tempfile_path(&format!("svarog_p4k_{name}.p4k"));
        let entries = vec![V2EntrySpec {
            name: name.to_string(),
            payload,
            uncompressed_size: original.len() as u64 + 1,
            compression_method: method,
            crc32: p4k_crc32(original),
            encrypted: false,
        }];
        fs::write(&tmp, build_v2_archive(&entries, 0)).unwrap();

        let archive = P4kArchive::open(&tmp).unwrap();
        let err = archive.read(&archive.get(0).unwrap()).unwrap_err();
        assert!(
            err.to_string().contains("decompressed size mismatch"),
            "expected size mismatch for method {method}, got {err}"
        );

        let _ = fs::remove_file(&tmp);
    }
}

#[test]
fn entry_crc_mismatch_is_rejected() {
    let tmp = tempfile_path("svarog_p4k_bad_crc.p4k");
    let entries = vec![V2EntrySpec {
        name: "bad-crc.bin".to_string(),
        payload: b"crc checked payload".to_vec(),
        uncompressed_size: 19,
        compression_method: CM_STORE,
        crc32: 0xDEAD_BEEF,
        encrypted: false,
    }];
    let mut bytes = build_v2_archive(&entries, 0);
    let eocdr_start = bytes.len() - EOCD_V2_SIZE;
    let eocdr =
        <Eocd2Record as zerocopy::FromBytes>::read_from_bytes(&bytes[eocdr_start..]).unwrap();
    let cdr_start = eocdr.central_directory_record_offset as usize;
    bytes[cdr_start + 0xAC..cdr_start + 0xCC]
        .copy_from_slice(&sha256_array(b"crc checked payload"));
    fs::write(&tmp, bytes).unwrap();

    let archive = P4kArchive::open(&tmp).unwrap();
    assert_eq!(
        archive.read(&archive.get(0).unwrap()).unwrap(),
        b"crc checked payload"
    );
    let err = archive.verify_integrity().unwrap_err();
    assert!(
        err.to_string().contains("CRC32 mismatch"),
        "expected CRC mismatch, got {err}"
    );

    let _ = fs::remove_file(&tmp);
}

#[test]
fn entry_sha256_mismatch_is_rejected_by_integrity_verifier() {
    let tmp = tempfile_path("svarog_p4k_bad_sha256.p4k");
    let data = b"sha checked payload";
    let entries = vec![V2EntrySpec {
        name: "bad-sha.bin".to_string(),
        payload: data.to_vec(),
        uncompressed_size: data.len() as u64,
        compression_method: CM_STORE,
        crc32: p4k_crc32(data),
        encrypted: false,
    }];
    fs::write(&tmp, build_v2_archive(&entries, 0)).unwrap();

    let archive = P4kArchive::open(&tmp).unwrap();
    let entry = archive.get(0).unwrap();
    assert_eq!(archive.read(&entry).unwrap(), data);
    let err = archive.verify_payload_sha256(&entry).unwrap_err();
    assert!(
        err.to_string().contains("SHA-256 mismatch"),
        "expected SHA-256 mismatch, got {err}"
    );

    let _ = fs::remove_file(&tmp);
}

#[test]
fn writer_builds_dump_verified_v2_layout() {
    let tmp = tempfile_path("svarog_p4k_writer_v2.p4k");
    let mut options = P4kWriterOptions::default();
    options.sector_size = SECTOR_SIZE;
    options.compression = CompressionMethod::Store;

    let mut builder = P4kBuilder::with_options(options);
    builder
        .add_bytes_with_method(
            "dir\\./sub//../file.txt   ",
            b"abc",
            CompressionMethod::Store,
        )
        .unwrap();
    let stats = builder.write_to_file(&tmp).unwrap();
    assert_eq!(stats.entry_count, 1);
    assert_eq!(stats.cdr_offset % 0x1_0000, 0);

    let bytes = fs::read(&tmp).unwrap();
    let cdr = &bytes[stats.cdr_offset as usize..][..CDR_V2_ENTRY_SIZE];
    assert_eq!(u16::from_le_bytes([cdr[0], cdr[1]]), CM_STORE as u16);
    assert_eq!(u16::from_le_bytes([cdr[2], cdr[3]]), 0);
    assert_eq!(u16::from_le_bytes([cdr[4], cdr[5]]), 0);
    assert_eq!(
        u32::from_le_bytes([cdr[6], cdr[7], cdr[8], cdr[9]]),
        p4k_crc32(b"abc")
    );
    assert_eq!(u64::from_le_bytes(cdr[0x0A..0x12].try_into().unwrap()), 3);
    assert_eq!(u64::from_le_bytes(cdr[0x1A..0x22].try_into().unwrap()), 0);
    let name_table = &bytes[stats.cdr_offset as usize + stats.cdr_size as usize..]
        [..stats.name_table_size as usize];
    assert_eq!(name_table, b"dir/file.txt\0");

    let eocdr_start = bytes.len() - EOCD_V2_SIZE;
    assert_eq!(stats.file_size as usize, bytes.len());
    assert_eq!(
        &bytes[bytes.len() - 4..],
        &EOCD_V2_MAGIC.to_le_bytes(),
        "official v2 EOF buffer ends with JiJi magic"
    );
    let name_table_end =
        stats.cdr_offset as usize + stats.cdr_size as usize + stats.name_table_size as usize;
    let eocdr = <Eocd2Record as zerocopy::FromBytes>::read_from_bytes(
        &bytes[eocdr_start..eocdr_start + EOCD_V2_SIZE],
    )
    .unwrap();
    assert_eq!(eocdr.trie_cache_offset as usize, name_table_end);
    assert!(eocdr.trie_cache_size >= 16 + 24 + 4 + 16);
    let trie_cache =
        &bytes[name_table_end..name_table_end + usize::try_from(eocdr.trie_cache_size).unwrap()];
    assert!(u32::from_le_bytes(trie_cache[0..4].try_into().unwrap()) >= 24);
    assert_eq!(u32::from_le_bytes(trie_cache[4..8].try_into().unwrap()), 1);
    assert_eq!(
        u32::from_le_bytes(trie_cache[8..12].try_into().unwrap()),
        16
    );
    assert_eq!(
        u32::from_le_bytes(trie_cache[12..16].try_into().unwrap()),
        0
    );
    let trie_cache_end = name_table_end + usize::try_from(eocdr.trie_cache_size).unwrap();
    assert!(bytes[trie_cache_end..eocdr_start]
        .iter()
        .all(|byte| *byte == 0));

    // The install block starts at the sector-aligned payload end and
    // stores bytes_already_written_to_disk for each entry.
    let end_of_payload_offset = eocdr.end_of_payload_offset as usize;
    assert_eq!(
        u64::from_le_bytes(
            bytes[end_of_payload_offset..end_of_payload_offset + 8]
                .try_into()
                .unwrap()
        ),
        3
    );

    let archive = P4kArchive::open(&tmp).unwrap();
    assert_eq!(archive.version(), P4kVersion::V2);
    let layout = archive.layout();
    assert_eq!(layout.file_size, bytes.len() as u64);
    assert_eq!(layout.actual_content_end, bytes.len() as u64);
    assert_eq!(layout.physical_sector_size, Some(SECTOR_SIZE));
    assert_eq!(layout.cdr_offset, stats.cdr_offset);
    assert_eq!(layout.cdr_size, stats.cdr_size);
    assert_eq!(
        layout.name_table_offset,
        Some(stats.cdr_offset + stats.cdr_size)
    );
    assert_eq!(layout.name_table_size, stats.name_table_size);
    assert_eq!(layout.end_of_payload, Some(eocdr.end_of_payload_offset));
    assert_eq!(
        layout.install_block_offset,
        Some(eocdr.end_of_payload_offset)
    );
    assert_eq!(
        layout.install_block_size,
        Some(stats.cdr_offset - eocdr.end_of_payload_offset)
    );
    assert_eq!(layout.eocd_offset, eocdr_start as u64);
    assert_eq!(layout.manifest_sha256, Some([0u8; 64]));
    assert!(archive.freelist_blocks().is_empty());
    let entry = archive.get(0).unwrap();
    assert_eq!(archive.find("dir\\file.txt").unwrap().name, "dir/file.txt");
    assert_eq!(
        archive.find("DIR\\./SUB//../FILE.TXT   ").unwrap().name,
        "dir/file.txt"
    );
    assert_eq!(entry.sha256, sha256_array(b"abc"));
    archive.verify_payload_sha256(&entry).unwrap();
    archive.verify_entry_integrity(&entry).unwrap();
    archive.verify_integrity().unwrap();
    assert_eq!(archive.read(&entry).unwrap(), b"abc");

    let _ = fs::remove_file(&tmp);
}

#[cfg(feature = "parallel")]
#[test]
fn parallel_integrity_verifier_reports_progress_for_each_entry() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let tmp = tempfile_path("svarog_p4k_parallel_verify_v2.p4k");
    let mut options = P4kWriterOptions::default();
    options.sector_size = SECTOR_SIZE;
    options.compression = CompressionMethod::Store;

    let mut builder = P4kBuilder::with_options(options);
    builder.add_bytes("a.txt", b"alpha").unwrap();
    builder.add_bytes("b.txt", b"beta").unwrap();
    builder.write_to_file(&tmp).unwrap();

    let archive = P4kArchive::open(&tmp).unwrap();
    let progress = AtomicUsize::new(0);
    archive
        .verify_integrity_parallel_with_progress(|_, _| {
            progress.fetch_add(1, Ordering::Relaxed);
        })
        .unwrap();
    assert_eq!(progress.load(Ordering::Relaxed), archive.entry_count());

    progress.store(0, Ordering::Relaxed);
    archive
        .verify_payloads_sha256_parallel_with_progress(|_, _| {
            progress.fetch_add(1, Ordering::Relaxed);
        })
        .unwrap();
    assert_eq!(progress.load(Ordering::Relaxed), archive.entry_count());

    progress.store(0, Ordering::Relaxed);
    archive
        .verify_payloads_sha256_physical_order_with_progress(|_, _| {
            progress.fetch_add(1, Ordering::Relaxed);
        })
        .unwrap();
    assert_eq!(progress.load(Ordering::Relaxed), archive.entry_count());

    progress.store(0, Ordering::Relaxed);
    archive
        .verify_payloads_sha256_physical_order_parallel_with_progress(|_, _| {
            progress.fetch_add(1, Ordering::Relaxed);
        })
        .unwrap();
    assert_eq!(progress.load(Ordering::Relaxed), archive.entry_count());

    let _ = fs::remove_file(&tmp);
}

#[test]
fn writer_orders_v2_new_entries_by_compressed_size_then_filename_handle() {
    let tmp = tempfile_path("svarog_p4k_writer_v2_order.p4k");
    let mut options = P4kWriterOptions::default();
    options.sector_size = SECTOR_SIZE;
    options.compression = CompressionMethod::Store;

    let mut builder = P4kBuilder::with_options(options);
    builder.add_bytes("zeta.bin", b"1111").unwrap();
    builder.add_bytes("alpha.bin", b"2222").unwrap();
    builder.add_bytes("large.bin", b"aaaaaaaa").unwrap();
    builder.write_to_file(&tmp).unwrap();

    let archive = P4kArchive::open(&tmp).unwrap();
    let names: Vec<_> = archive.iter().map(|entry| entry.name.to_string()).collect();
    assert_eq!(names, ["large.bin", "alpha.bin", "zeta.bin"]);
    assert_eq!(
        archive.read(&archive.find("zeta.bin").unwrap()).unwrap(),
        b"1111"
    );
    assert_eq!(
        archive.read(&archive.find("large.bin").unwrap()).unwrap(),
        b"aaaaaaaa"
    );

    let _ = fs::remove_file(&tmp);
}

#[test]
fn writer_places_v2_entries_on_sector_boundaries() {
    let tmp = tempfile_path("svarog_p4k_writer_v2_alignment.p4k");
    let mut options = P4kWriterOptions::default();
    options.sector_size = SECTOR_SIZE;
    options.compression = CompressionMethod::Store;

    let mut builder = P4kBuilder::with_options(options);
    builder.add_bytes("small.bin", b"aa").unwrap();
    builder.add_bytes("large.bin", b"aaaaaaaa").unwrap();
    let stats = builder.write_to_file(&tmp).unwrap();

    let bytes = fs::read(&tmp).unwrap();
    let first_cdr = &bytes[stats.cdr_offset as usize..][..CDR_V2_ENTRY_SIZE];
    let second_cdr = &bytes[stats.cdr_offset as usize + CDR_V2_ENTRY_SIZE..][..CDR_V2_ENTRY_SIZE];
    assert_eq!(
        u64::from_le_bytes(first_cdr[0x1A..0x22].try_into().unwrap()),
        0
    );
    assert_eq!(
        u64::from_le_bytes(second_cdr[0x1A..0x22].try_into().unwrap()),
        SECTOR_SIZE
    );

    let archive = P4kArchive::open(&tmp).unwrap();
    assert_eq!(archive.get(0).unwrap().name, "large.bin");
    assert_eq!(archive.get(1).unwrap().name, "small.bin");
    assert_eq!(
        archive.read(&archive.find("large.bin").unwrap()).unwrap(),
        b"aaaaaaaa"
    );
    assert_eq!(
        archive.read(&archive.find("small.bin").unwrap()).unwrap(),
        b"aa"
    );

    let _ = fs::remove_file(&tmp);
}

#[test]
fn writer_add_file_preserves_filesystem_dos_timestamp() {
    let input = tempfile_path("svarog_p4k_timestamp_input.txt");
    let archive_path = tempfile_path("svarog_p4k_timestamp_writer_v2.p4k");
    fs::write(&input, b"timestamped").unwrap();

    let mut options = P4kWriterOptions::default();
    options.sector_size = SECTOR_SIZE;
    options.compression = CompressionMethod::Store;
    let mut builder = P4kBuilder::with_options(options);
    builder.add_file(&input, "timestamp.txt").unwrap();
    let stats = builder.write_to_file(&archive_path).unwrap();

    let bytes = fs::read(&archive_path).unwrap();
    let cdr = &bytes[stats.cdr_offset as usize..][..CDR_V2_ENTRY_SIZE];
    let dos_time = u16::from_le_bytes(cdr[0x02..0x04].try_into().unwrap());
    let dos_date = u16::from_le_bytes(cdr[0x04..0x06].try_into().unwrap());
    // Midnight encodes to DOS time 0, so the date field catches metadata fallback.
    assert_ne!((dos_time, dos_date), (0, 0));
    assert_ne!(dos_date, 0);

    let _ = fs::remove_file(&input);
    let _ = fs::remove_file(&archive_path);
}

#[test]
fn writer_stores_manifest_digests_in_v2_eocdr() {
    let tmp = tempfile_path("svarog_p4k_manifest_writer_v2.p4k");
    let mut options = P4kWriterOptions::default();
    options.sector_size = SECTOR_SIZE;
    options.compression = CompressionMethod::Store;
    options.manifest_sha256 = std::array::from_fn(|i| (i as u8).wrapping_mul(3));

    let mut builder = P4kBuilder::with_options(options.clone());
    builder.add_bytes("manifest.bin", b"manifest").unwrap();
    let stats = builder.write_to_file(&tmp).unwrap();

    let bytes = fs::read(&tmp).unwrap();
    assert_eq!(stats.file_size as usize, bytes.len());
    let eocdr_start = bytes.len() - EOCD_V2_SIZE;
    assert_eq!(
        &bytes[eocdr_start + 0x69..eocdr_start + 0xA9],
        &options.manifest_sha256
    );

    let _ = fs::remove_file(&tmp);
}

#[test]
fn writer_v2_eocdr_fields_match_dump_offsets() {
    let tmp = tempfile_path("svarog_p4k_writer_v2_eocdr_fields.p4k");
    let mut options = P4kWriterOptions::default();
    options.sector_size = SECTOR_SIZE;
    options.compression = CompressionMethod::Store;
    options.manifest_sha256 = std::array::from_fn(|i| 0xA5u8 ^ i as u8);

    let mut builder = P4kBuilder::with_options(options.clone());
    builder.add_bytes("first.bin", b"11111").unwrap();
    builder.add_bytes("second.bin", b"22").unwrap();
    let stats = builder.write_to_file(&tmp).unwrap();

    let bytes = fs::read(&tmp).unwrap();
    let eocdr_start = bytes.len() - EOCD_V2_SIZE;
    let eocdr = &bytes[eocdr_start..eocdr_start + EOCD_V2_SIZE];
    let field_u64 =
        |offset: usize| u64::from_le_bytes(eocdr[offset..offset + 8].try_into().unwrap());

    assert_eq!(field_u64(0x00), 2);
    assert_eq!(field_u64(0x08), 0);
    assert_eq!(field_u64(0x10), stats.cdr_offset);
    assert_eq!(field_u64(0x18), 2 * CDR_V2_ENTRY_SIZE as u64);
    assert_eq!(field_u64(0x20), 0);
    assert_eq!(field_u64(0x28), stats.cdr_offset + stats.cdr_size);
    assert_eq!(field_u64(0x30), stats.name_table_size);
    assert_eq!(field_u64(0x38), 0);
    assert_eq!(field_u64(0x40), stats.payload_end);
    assert_eq!(field_u64(0x48), 0);
    assert_eq!(
        field_u64(0x50),
        stats.cdr_offset + stats.cdr_size + stats.name_table_size
    );
    assert!(field_u64(0x58) >= 16 + 24 + 2 * 4 + 16);
    assert_eq!(field_u64(0x60), options.sector_size);
    assert_eq!(eocdr[0x68], 0);
    assert_eq!(&eocdr[0x69..0xA9], &options.manifest_sha256);
    assert_eq!(
        u16::from_le_bytes(eocdr[0xA9..0xAB].try_into().unwrap()),
        EOCD_V2_VERSION
    );
    assert_eq!(
        u32::from_le_bytes(eocdr[0xAB..0xAF].try_into().unwrap()),
        EOCD_V2_MAGIC
    );

    assert_eq!(
        &bytes[eocdr_start + 0xA9..eocdr_start + 0xAB],
        &2u16.to_le_bytes()
    );
    assert_eq!(
        &bytes[eocdr_start + 0xAB..eocdr_start + 0xAF],
        &0x696A_694Au32.to_le_bytes()
    );
    assert_eq!(stats.file_size as usize, bytes.len());

    let _ = fs::remove_file(&tmp);
}

#[test]
fn writer_builds_empty_v1_initial_layout_like_initialize_pak_file_v1() {
    let tmp = tempfile_path("svarog_p4k_empty_init_v1.p4k");
    let mut options = P4kWriterOptions::default();
    options.sector_size = SECTOR_SIZE;

    let builder = P4kBuilder::with_options(options);
    let stats = builder.write_v1_to_file(&tmp).unwrap();
    let bytes = fs::read(&tmp).unwrap();

    assert_eq!(stats.entry_count, 0);
    assert_eq!(stats.payload_end, 0);
    assert_eq!(stats.cdr_offset, 0);
    assert_eq!(stats.cdr_size, 0);
    assert_eq!(stats.file_size, SECTOR_SIZE);
    assert_eq!(bytes.len(), SECTOR_SIZE as usize);

    assert_eq!(
        u32::from_le_bytes(bytes[0x00..0x04].try_into().unwrap()),
        0x0606_4b50
    );
    assert_eq!(
        u64::from_le_bytes(bytes[0x04..0x0C].try_into().unwrap()),
        0x2C
    );
    assert_eq!(
        u16::from_le_bytes(bytes[0x0C..0x0E].try_into().unwrap()),
        46
    );
    assert_eq!(
        u16::from_le_bytes(bytes[0x0E..0x10].try_into().unwrap()),
        45
    );
    assert_eq!(u32::from_le_bytes(bytes[0x10..0x14].try_into().unwrap()), 0);
    assert_eq!(u32::from_le_bytes(bytes[0x14..0x18].try_into().unwrap()), 0);
    assert_eq!(u64::from_le_bytes(bytes[0x18..0x20].try_into().unwrap()), 0);
    assert_eq!(u64::from_le_bytes(bytes[0x20..0x28].try_into().unwrap()), 0);
    assert_eq!(u64::from_le_bytes(bytes[0x28..0x30].try_into().unwrap()), 0);
    assert_eq!(u64::from_le_bytes(bytes[0x30..0x38].try_into().unwrap()), 0);

    assert_eq!(
        u32::from_le_bytes(bytes[0x38..0x3C].try_into().unwrap()),
        0x0706_4b50
    );
    assert_eq!(u32::from_le_bytes(bytes[0x3C..0x40].try_into().unwrap()), 0);
    assert_eq!(u64::from_le_bytes(bytes[0x40..0x48].try_into().unwrap()), 0);
    assert_eq!(u32::from_le_bytes(bytes[0x48..0x4C].try_into().unwrap()), 1);

    assert_eq!(
        u32::from_le_bytes(bytes[0x4C..0x50].try_into().unwrap()),
        0x0605_4b50
    );
    assert_eq!(&bytes[0x50..0x60], &[0xFF; 16]);
    assert_eq!(
        u16::from_le_bytes(bytes[0x60..0x62].try_into().unwrap()),
        16
    );
    assert_eq!(&bytes[0x62..0x64], b"CI");
    assert_eq!(
        u32::from_le_bytes(bytes[0x64..0x68].try_into().unwrap()),
        0x0001_0047
    );
    assert_eq!(
        u16::from_le_bytes(bytes[0x68..0x6A].try_into().unwrap()),
        SECTOR_SIZE as u16
    );
    assert_eq!(u64::from_le_bytes(bytes[0x6A..0x72].try_into().unwrap()), 0);
    assert!(bytes[0x72..].iter().all(|byte| *byte == 0));

    let archive = P4kArchive::open(&tmp).unwrap();
    assert_eq!(archive.version(), P4kVersion::V1);
    assert_eq!(archive.entry_count(), 0);
    assert!(archive.freelist_blocks().is_empty());

    let _ = fs::remove_file(&tmp);
}

#[test]
fn writer_builds_empty_v2_initial_layout_like_initialize_pak_file_v2() {
    let tmp = tempfile_path("svarog_p4k_empty_init_v2.p4k");
    let mut options = P4kWriterOptions::default();
    options.sector_size = SECTOR_SIZE;

    let builder = P4kBuilder::with_options(options.clone());
    let stats = builder.write_to_file(&tmp).unwrap();
    let bytes = fs::read(&tmp).unwrap();

    assert_eq!(stats.entry_count, 0);
    assert_eq!(stats.payload_end, 0);
    assert_eq!(stats.cdr_offset, 0);
    assert_eq!(stats.cdr_size, 0);
    assert_eq!(stats.name_table_size, 0);
    assert_eq!(stats.file_size, SECTOR_SIZE);
    assert_eq!(bytes.len(), SECTOR_SIZE as usize);

    let eocdr_start = SECTOR_SIZE as usize - EOCD_V2_SIZE;
    let eocdr = &bytes[eocdr_start..];
    let field_u64 =
        |offset: usize| u64::from_le_bytes(eocdr[offset..offset + 8].try_into().unwrap());
    let trie_cache_size = 16 + 24 + 16;
    assert_eq!(
        &bytes[0..16],
        &[24, 0, 0, 0, 0, 0, 0, 0, 16, 0, 0, 0, 0, 0, 0, 0]
    );
    assert!(bytes[trie_cache_size..eocdr_start]
        .iter()
        .all(|byte| *byte == 0));
    assert_eq!(field_u64(0x00), 0);
    assert_eq!(field_u64(0x08), 0);
    assert_eq!(field_u64(0x10), 0);
    assert_eq!(field_u64(0x18), 0);
    assert_eq!(field_u64(0x20), 0);
    assert_eq!(field_u64(0x28), 0);
    assert_eq!(field_u64(0x30), 0);
    assert_eq!(field_u64(0x38), 0);
    assert_eq!(field_u64(0x40), 0);
    assert_eq!(field_u64(0x48), 0);
    assert_eq!(field_u64(0x50), 0);
    assert_eq!(field_u64(0x58), trie_cache_size as u64);
    assert_eq!(field_u64(0x60), SECTOR_SIZE);
    assert_eq!(eocdr[0x68], 0);
    assert_eq!(&eocdr[0x69..0xA9], &options.manifest_sha256);
    assert_eq!(
        u16::from_le_bytes(eocdr[0xA9..0xAB].try_into().unwrap()),
        EOCD_V2_VERSION
    );
    assert_eq!(
        u32::from_le_bytes(eocdr[0xAB..0xAF].try_into().unwrap()),
        EOCD_V2_MAGIC
    );

    let archive = P4kArchive::open(&tmp).unwrap();
    assert_eq!(archive.version(), P4kVersion::V2);
    assert_eq!(archive.entry_count(), 0);
    assert!(archive.freelist_blocks().is_empty());

    let _ = fs::remove_file(&tmp);
}

#[test]
fn writer_builds_readable_v1_layout() {
    let tmp = tempfile_path("svarog_p4k_writer_v1.p4k");
    let mut options = P4kWriterOptions::default();
    options.sector_size = SECTOR_SIZE;
    options.compression = CompressionMethod::Store;

    let mut builder = P4kBuilder::with_options(options);
    builder
        .add_bytes_with_method("v1/file.txt", b"legacy", CompressionMethod::Store)
        .unwrap();
    let stats = builder.write_v1_to_file(&tmp).unwrap();
    assert_eq!(stats.entry_count, 1);
    assert_eq!(
        stats.payload_end,
        (local_v1_record_size("v1/file.txt", SECTOR_SIZE) + 6 + SECTOR_SIZE - 1)
            & !(SECTOR_SIZE - 1)
    );

    let bytes = fs::read(&tmp).unwrap();
    assert_eq!(
        u32::from_le_bytes(bytes[0..4].try_into().unwrap()),
        0x0403_4b50
    );
    let local_name = b"v1/file.txt";
    let local_extra_len = u16::from_le_bytes(bytes[28..30].try_into().unwrap()) as usize;
    assert_eq!(
        local_extra_len,
        SECTOR_SIZE as usize - 0x1E - local_name.len()
    );
    let local_extra = 0x1E + local_name.len();
    assert_eq!(
        u16::from_le_bytes(bytes[local_extra..local_extra + 2].try_into().unwrap()),
        0x0001
    );
    assert_eq!(
        u16::from_le_bytes(bytes[local_extra + 2..local_extra + 4].try_into().unwrap()),
        0x20
    );
    assert_eq!(
        u64::from_le_bytes(bytes[local_extra + 4..local_extra + 12].try_into().unwrap()),
        6
    );
    assert_eq!(
        u64::from_le_bytes(
            bytes[local_extra + 12..local_extra + 20]
                .try_into()
                .unwrap()
        ),
        6
    );
    assert_eq!(
        u64::from_le_bytes(
            bytes[local_extra + 20..local_extra + 28]
                .try_into()
                .unwrap()
        ),
        0
    );
    let dummy = local_extra + 0x20;
    assert_eq!(
        u16::from_le_bytes(bytes[dummy..dummy + 2].try_into().unwrap()),
        0x0666
    );
    assert_eq!(
        u16::from_le_bytes(bytes[dummy + 2..dummy + 4].try_into().unwrap()),
        (local_extra_len - 0x20) as u16
    );
    let payload_offset = local_v1_record_size("v1/file.txt", SECTOR_SIZE) as usize;
    assert_eq!(&bytes[payload_offset..payload_offset + 6], b"legacy");
    assert_eq!(
        u64::from_le_bytes(
            bytes[stats.payload_end as usize..stats.payload_end as usize + 8]
                .try_into()
                .unwrap()
        ),
        6
    );
    let ci_comment = find_v1_ci_comment(&bytes);
    assert_eq!(
        u16::from_le_bytes(bytes[ci_comment - 2..ci_comment].try_into().unwrap()),
        16
    );
    assert_eq!(&bytes[ci_comment..ci_comment + 6], b"CIG\0\x01\0");
    assert_eq!(
        u32::from_le_bytes(bytes[ci_comment + 2..ci_comment + 6].try_into().unwrap()),
        0x0001_0047
    );
    assert_eq!(
        u16::from_le_bytes(bytes[ci_comment + 6..ci_comment + 8].try_into().unwrap()),
        SECTOR_SIZE as u16
    );
    assert_eq!(
        u64::from_le_bytes(bytes[ci_comment + 8..ci_comment + 16].try_into().unwrap()),
        0
    );

    let archive = P4kArchive::open(&tmp).unwrap();
    assert_eq!(archive.version(), P4kVersion::V1);
    let layout = archive.layout();
    assert_eq!(layout.file_size, bytes.len() as u64);
    assert_eq!(layout.actual_content_end, ci_comment as u64 + 8);
    assert_eq!(layout.physical_sector_size, Some(SECTOR_SIZE));
    assert_eq!(layout.cdr_offset, stats.cdr_offset);
    assert_eq!(layout.cdr_size, stats.cdr_size);
    assert_eq!(layout.name_table_offset, None);
    assert_eq!(layout.name_table_size, 0);
    assert_eq!(layout.end_of_payload, Some(stats.payload_end));
    assert_eq!(layout.install_block_offset, Some(stats.payload_end));
    assert_eq!(layout.install_block_size, Some(12));
    assert_eq!(
        layout.eocd_offset,
        (ci_comment - 4 - std::mem::size_of::<svarog_p4k::zip::EocdRecord>()) as u64
    );
    assert_eq!(layout.manifest_sha256, None);
    assert!(archive.freelist_blocks().is_empty());
    assert_eq!(archive.read(&archive.get(0).unwrap()).unwrap(), b"legacy");

    let _ = fs::remove_file(&tmp);
}

#[test]
fn writer_places_v1_entries_on_sector_boundaries() {
    let tmp = tempfile_path("svarog_p4k_writer_v1_alignment.p4k");
    let mut options = P4kWriterOptions::default();
    options.sector_size = SECTOR_SIZE;
    options.compression = CompressionMethod::Store;

    let mut builder = P4kBuilder::with_options(options);
    builder.add_bytes("a.bin", b"aaa").unwrap();
    builder.add_bytes("b.bin", b"bbbbb").unwrap();
    builder.write_v1_to_file(&tmp).unwrap();

    let archive = P4kArchive::open(&tmp).unwrap();
    assert_eq!(archive.entry_count(), 2);
    assert_eq!(archive.get(0).unwrap().name, "b.bin");
    assert_eq!(archive.get(1).unwrap().name, "a.bin");
    assert_eq!(archive.get(0).unwrap().local_header_offset, 0);
    assert_eq!(
        archive.get(1).unwrap().local_header_offset,
        (local_v1_record_size("b.bin", SECTOR_SIZE) + 5 + SECTOR_SIZE - 1) & !(SECTOR_SIZE - 1)
    );
    assert_eq!(
        archive.read(&archive.find("a.bin").unwrap()).unwrap(),
        b"aaa"
    );
    assert_eq!(
        archive.read(&archive.find("b.bin").unwrap()).unwrap(),
        b"bbbbb"
    );

    let _ = fs::remove_file(&tmp);
}

#[test]
fn reader_rejects_v1_ci_comment_with_bad_marker() {
    let tmp = tempfile_path("svarog_p4k_bad_v1_ci_marker.p4k");
    let mut bytes = build_zip64_v1_archive_with_p4k_metadata_install_freelist(
        "bad-marker.bin",
        b"payload",
        [0; 128],
        sha256_array(b"payload"),
        0,
        0,
        0,
        7,
        &[],
    );
    let ci_comment = find_v1_ci_comment(&bytes);
    bytes[ci_comment + 2..ci_comment + 6].copy_from_slice(&0x0001_0048u32.to_le_bytes());
    fs::write(&tmp, bytes).unwrap();

    let err = P4kArchive::open(&tmp).unwrap_err();
    assert!(err.to_string().contains("P4K CI EOCD marker"));

    let _ = fs::remove_file(&tmp);
}

#[test]
fn reader_rejects_v1_ci_comment_with_bad_sector_size() {
    let tmp = tempfile_path("svarog_p4k_bad_v1_ci_sector.p4k");
    let mut bytes = build_zip64_v1_archive_with_p4k_metadata_install_freelist(
        "bad-sector.bin",
        b"payload",
        [0; 128],
        sha256_array(b"payload"),
        0,
        0,
        0,
        7,
        &[],
    );
    let ci_comment = find_v1_ci_comment(&bytes);
    bytes[ci_comment + 6..ci_comment + 8].copy_from_slice(&3u16.to_le_bytes());
    fs::write(&tmp, bytes).unwrap();

    let err = P4kArchive::open(&tmp).unwrap_err();
    assert!(err.to_string().contains("P4K CI EOCD sector size"));

    let _ = fs::remove_file(&tmp);
}

#[test]
fn reader_rejects_v1_ci_archive_with_unaligned_file_size() {
    let tmp = tempfile_path("svarog_p4k_bad_v1_unaligned_size.p4k");
    let mut options = P4kWriterOptions::default();
    options.sector_size = SECTOR_SIZE;
    options.compression = CompressionMethod::Store;

    let mut builder = P4kBuilder::with_options(options);
    builder.add_bytes("unaligned.bin", b"payload").unwrap();
    builder.write_v1_to_file(&tmp).unwrap();

    let mut bytes = fs::read(&tmp).unwrap();
    assert_eq!(bytes.len() as u64 % SECTOR_SIZE, 0);
    assert_eq!(bytes.pop(), Some(0));
    fs::write(&tmp, bytes).unwrap();

    let err = P4kArchive::open(&tmp).unwrap_err();
    assert!(
        err.to_string().contains("not aligned"),
        "expected unaligned-size rejection, got {err}"
    );

    let _ = fs::remove_file(&tmp);
}

#[test]
fn reader_rejects_v1_ci_archive_with_nonzero_sector_padding() {
    let tmp = tempfile_path("svarog_p4k_bad_v1_ci_sector_padding.p4k");
    let mut bytes = build_zip64_v1_archive_with_p4k_metadata_install_freelist(
        "bad-padding.bin",
        b"payload",
        [0; 128],
        sha256_array(b"payload"),
        0,
        0,
        0,
        7,
        &[],
    );
    let ci_comment = find_v1_ci_comment(&bytes);
    let padding_start = ci_comment + 16;
    assert_eq!(bytes[padding_start], 0);
    bytes[padding_start] = 0xA5;
    fs::write(&tmp, bytes).unwrap();

    let err = P4kArchive::open(&tmp).unwrap_err();
    assert!(
        err.to_string().contains("sector padding"),
        "expected v1 sector-padding rejection, got {err}"
    );

    let _ = fs::remove_file(&tmp);
}

#[test]
fn reader_rejects_v1_ci_entry_with_unaligned_local_offset() {
    let tmp = tempfile_path("svarog_p4k_bad_v1_unaligned_entry.p4k");
    let mut bytes = build_zip64_v1_archive_with_p4k_metadata_install_freelist(
        "bad-offset.bin",
        b"payload",
        [0; 128],
        sha256_array(b"payload"),
        0,
        0,
        0,
        7,
        &[],
    );
    let central_dir = bytes
        .windows(4)
        .position(|window| window == 0x0201_4b50u32.to_le_bytes())
        .expect("central directory record");
    let extra = central_dir + 0x2E + "bad-offset.bin".len();
    let zip64_local_header_offset = extra + 4 + 8 + 8;
    bytes[zip64_local_header_offset..zip64_local_header_offset + 8]
        .copy_from_slice(&1u64.to_le_bytes());
    fs::write(&tmp, bytes).unwrap();

    let err = P4kArchive::open(&tmp).unwrap_err();
    assert!(
        err.to_string().contains("not aligned"),
        "expected unaligned v1 entry offset rejection, got {err}"
    );

    let _ = fs::remove_file(&tmp);
}

#[test]
fn v1_loader_removes_duplicate_names_like_dump() {
    let v1 = tempfile_path("svarog_p4k_duplicate_v1.p4k");
    let v2 = tempfile_path("svarog_p4k_duplicate_v1_converted.p4k");
    let mut options = P4kWriterOptions::default();
    options.sector_size = SECTOR_SIZE;
    options.compression = CompressionMethod::Store;

    let mut builder = P4kBuilder::with_options(options.clone());
    builder.add_bytes("Case/Dup.bin", b"first-long").unwrap();
    builder.add_bytes("case\\dup.bin", b"second").unwrap();
    builder.add_bytes("tail.bin", b"tail").unwrap();
    builder.write_v1_to_file(&v1).unwrap();

    let archive = P4kArchive::open(&v1).unwrap();
    assert_eq!(archive.entry_count(), 2);
    let dup = archive.find("CASE/DUP.BIN").unwrap();
    assert_eq!(archive.read(&dup).unwrap(), b"first-long");
    assert!(archive.find("tail.bin").is_some());
    let duplicate_local_offset = (local_v1_record_size("Case/Dup.bin", SECTOR_SIZE)
        + b"first-long".len() as u64
        + SECTOR_SIZE
        - 1)
        & !(SECTOR_SIZE - 1);
    let duplicate_free_size =
        (local_v1_record_size("case/dup.bin", SECTOR_SIZE) + b"second".len() as u64 + SECTOR_SIZE
            - 1)
            & !(SECTOR_SIZE - 1);
    assert!(
        archive.freelist_blocks().iter().any(
            |block| block.offset == duplicate_local_offset && block.size == duplicate_free_size
        ),
        "duplicate entry free span was not recorded"
    );

    convert_v1_to_v2(&v1, &v2, options).unwrap();
    let converted = P4kArchive::open(&v2).unwrap();
    assert_eq!(converted.entry_count(), 2);
    assert_eq!(
        converted
            .read(&converted.find("case/dup.bin").unwrap())
            .unwrap(),
        b"first-long"
    );
    assert!(
        converted
            .freelist_blocks()
            .iter()
            .any(|block| block.offset <= duplicate_local_offset
                && block.offset + block.size >= duplicate_local_offset + duplicate_free_size),
        "converted archive did not preserve duplicate free span inside merged freelist"
    );

    let _ = fs::remove_file(&v1);
    let _ = fs::remove_file(&v2);
}

#[test]
fn writer_builds_readable_compression_method_variants() {
    let tmp = tempfile_path("svarog_p4k_writer_compression_variants.p4k");
    let mut options = P4kWriterOptions::default();
    options.sector_size = SECTOR_SIZE;

    let deflate_payload = b"raw deflate payload ".repeat(32);
    let zlib_payload = b"zlib wrapped deflate payload ".repeat(24);
    let zstd_payload = b"deprecated zstd method payload ".repeat(20);

    let mut builder = P4kBuilder::with_options(options);
    builder
        .add_bytes_with_method("deflate.raw", &deflate_payload, CompressionMethod::Deflate)
        .unwrap();
    builder
        .add_bytes_with_method(
            "deflate.zlib",
            &zlib_payload,
            CompressionMethod::DeflateZlib,
        )
        .unwrap();
    builder
        .add_bytes_with_method(
            "zstd.deprecated",
            &zstd_payload,
            CompressionMethod::ZstdDeprecated,
        )
        .unwrap();
    builder.write_to_file(&tmp).unwrap();

    let archive = P4kArchive::open(&tmp).unwrap();
    for (name, method, expected) in [
        (
            "deflate.raw",
            CompressionMethod::Deflate,
            deflate_payload.as_slice(),
        ),
        (
            "deflate.zlib",
            CompressionMethod::DeflateZlib,
            zlib_payload.as_slice(),
        ),
        (
            "zstd.deprecated",
            CompressionMethod::ZstdDeprecated,
            zstd_payload.as_slice(),
        ),
    ] {
        let entry = archive.find(name).unwrap();
        assert_eq!(entry.compression_method, method);
        assert_eq!(archive.read(&entry).unwrap(), expected);
    }

    let bytes = fs::read(&tmp).unwrap();
    let raw_deflate = archive.find("deflate.raw").unwrap();
    let raw_start = raw_deflate.local_header_offset as usize;
    let raw_end = raw_start + raw_deflate.compressed_size as usize;
    let mut raw_decoded = Vec::new();
    flate2::read::DeflateDecoder::new(&bytes[raw_start..raw_end])
        .read_to_end(&mut raw_decoded)
        .unwrap();
    assert_eq!(raw_decoded, deflate_payload);

    let zlib_deflate = archive.find("deflate.zlib").unwrap();
    let zlib_start = zlib_deflate.local_header_offset as usize;
    let zlib_end = zlib_start + zlib_deflate.compressed_size as usize;
    let mut zlib_decoded = Vec::new();
    flate2::read::ZlibDecoder::new(&bytes[zlib_start..zlib_end])
        .read_to_end(&mut zlib_decoded)
        .unwrap();
    assert_eq!(zlib_decoded, zlib_payload);

    let _ = fs::remove_file(&tmp);
}

#[test]
fn writer_builds_readable_encrypted_v2_layout() {
    let tmp = tempfile_path("svarog_p4k_writer_encrypted_v2.p4k");
    let mut options = P4kWriterOptions::default();
    options.sector_size = SECTOR_SIZE;
    options.compression = CompressionMethod::Store;

    let mut builder = P4kBuilder::with_options(options);
    builder
        .add_bytes_with_method_encrypted(
            "secret.bin",
            b"encrypted payload\0\0",
            CompressionMethod::Store,
        )
        .unwrap();
    let stats = builder.write_to_file(&tmp).unwrap();

    let bytes = fs::read(&tmp).unwrap();
    let cdr = &bytes[stats.cdr_offset as usize..][..CDR_V2_ENTRY_SIZE];
    assert_eq!(u16::from_le_bytes(cdr[0xAA..0xAC].try_into().unwrap()), 1);
    assert_eq!(
        u64::from_le_bytes(cdr[0x0A..0x12].try_into().unwrap()) % 16,
        0
    );

    let archive = P4kArchive::open(&tmp).unwrap();
    let entry = archive.get(0).unwrap();
    assert!(entry.is_encrypted);
    assert_eq!(archive.read(&entry).unwrap(), b"encrypted payload\0\0");

    let _ = fs::remove_file(&tmp);
}

#[test]
fn writer_v1_local_zip64_offset_tracks_local_record_offset() {
    let tmp = tempfile_path("svarog_p4k_writer_v1_local_zip64_offset.p4k");
    let mut options = P4kWriterOptions::default();
    options.sector_size = SECTOR_SIZE;
    options.compression = CompressionMethod::Store;

    let mut builder = P4kBuilder::with_options(options);
    builder.add_bytes("first.bin", b"aaaa").unwrap();
    builder.add_bytes("second.bin", b"bb").unwrap();
    let stats = builder.write_v1_to_file(&tmp).unwrap();
    assert_eq!(stats.entry_count, 2);

    let bytes = fs::read(&tmp).unwrap();
    let second_local_offset =
        ((local_v1_record_size("first.bin", SECTOR_SIZE) + b"aaaa".len() as u64 + SECTOR_SIZE - 1)
            & !(SECTOR_SIZE - 1)) as usize;
    assert_eq!(
        u32::from_le_bytes(
            bytes[second_local_offset..second_local_offset + 4]
                .try_into()
                .unwrap()
        ),
        0x0403_4b50
    );
    let second_name = b"second.bin";
    assert_eq!(
        &bytes[second_local_offset + 0x1E..second_local_offset + 0x1E + second_name.len()],
        second_name
    );
    let second_zip64_offset = second_local_offset + 0x1E + second_name.len();
    assert_eq!(
        u64::from_le_bytes(
            bytes[second_zip64_offset + 20..second_zip64_offset + 28]
                .try_into()
                .unwrap()
        ),
        second_local_offset as u64
    );

    let archive = P4kArchive::open(&tmp).unwrap();
    assert_eq!(
        archive.get(1).unwrap().local_header_offset,
        second_local_offset as u64
    );

    let _ = fs::remove_file(&tmp);
}

#[test]
fn writer_builds_readable_encrypted_v1_layout() {
    let tmp = tempfile_path("svarog_p4k_writer_encrypted_v1.p4k");
    let mut options = P4kWriterOptions::default();
    options.sector_size = SECTOR_SIZE;
    options.compression = CompressionMethod::Store;

    let mut builder = P4kBuilder::with_options(options);
    builder
        .add_bytes_encrypted("secret.bin", b"legacy secret\0\0")
        .unwrap();
    builder.write_v1_to_file(&tmp).unwrap();

    let archive = P4kArchive::open(&tmp).unwrap();
    assert_eq!(archive.version(), P4kVersion::V1);
    let entry = archive.get(0).unwrap();
    assert!(entry.is_encrypted);
    assert_eq!(archive.read(&entry).unwrap(), b"legacy secret\0\0");

    let _ = fs::remove_file(&tmp);
}

#[test]
fn writer_builds_readable_encrypted_compressed_entries() {
    let v2 = tempfile_path("svarog_p4k_writer_encrypted_compressed_v2.p4k");
    let v1 = tempfile_path("svarog_p4k_writer_encrypted_compressed_v1.p4k");
    let mut options = P4kWriterOptions::default();
    options.sector_size = SECTOR_SIZE;
    options.compression = CompressionMethod::Zstd;

    let payload = b"compressed secret ".repeat(64);
    let mut builder = P4kBuilder::with_options(options);
    builder
        .add_bytes_with_method_encrypted("secret.bin", &payload, CompressionMethod::Zstd)
        .unwrap();

    let stats = builder.write_to_file(&v2).unwrap();
    let bytes = fs::read(&v2).unwrap();
    let cdr = &bytes[stats.cdr_offset as usize..][..CDR_V2_ENTRY_SIZE];
    assert_eq!(
        u16::from_le_bytes(cdr[0x00..0x02].try_into().unwrap()),
        CM_ZSTD as u16
    );
    assert_eq!(u16::from_le_bytes(cdr[0xAA..0xAC].try_into().unwrap()), 1);
    assert_eq!(
        u64::from_le_bytes(cdr[0x0A..0x12].try_into().unwrap()) % 16,
        0
    );

    let archive = P4kArchive::open(&v2).unwrap();
    let entry = archive.get(0).unwrap();
    assert!(entry.is_encrypted);
    assert_eq!(entry.compression_method, CompressionMethod::Zstd);
    let payload_start = entry.local_header_offset as usize;
    let payload_end = payload_start + entry.compressed_size as usize;
    let stored_payload = &bytes[payload_start..payload_end];
    assert_eq!(sha256_array(stored_payload), entry.sha256);
    assert_ne!(
        &stored_payload[..4],
        &[0x28, 0xB5, 0x2F, 0xFD],
        "encrypted zstd payload must store ciphertext, not plaintext zstd bytes"
    );
    archive.verify_entry_integrity(&entry).unwrap();
    assert_eq!(archive.read(&entry).unwrap(), payload);

    builder.write_v1_to_file(&v1).unwrap();
    let archive = P4kArchive::open(&v1).unwrap();
    assert_eq!(archive.version(), P4kVersion::V1);
    let entry = archive.get(0).unwrap();
    assert!(entry.is_encrypted);
    assert_eq!(entry.compression_method, CompressionMethod::Zstd);
    archive.verify_entry_integrity(&entry).unwrap();
    assert_eq!(archive.read(&entry).unwrap(), payload);

    let _ = fs::remove_file(&v2);
    let _ = fs::remove_file(&v1);
}

#[test]
fn writer_empty_entries_match_official_empty_file_metadata() {
    let empty_file = tempfile_path("svarog_p4k_empty_input.bin");
    let v2 = tempfile_path("svarog_p4k_empty_writer_v2.p4k");
    let v1 = tempfile_path("svarog_p4k_empty_writer_v1.p4k");
    fs::write(&empty_file, b"").unwrap();

    let mut options = P4kWriterOptions::default();
    options.sector_size = SECTOR_SIZE;
    options.compression = CompressionMethod::Zstd;
    let mut builder = P4kBuilder::with_options(options);
    builder
        .add_bytes_with_method_encrypted("empty-bytes.bin", b"", CompressionMethod::Zstd)
        .unwrap()
        .add_file_encrypted(&empty_file, "empty-file.bin")
        .unwrap();

    let stats = builder.write_to_file(&v2).unwrap();
    let bytes = fs::read(&v2).unwrap();
    let empty_sha256 = sha256_array(b"");
    let empty_crc32 = p4k_crc32(b"");
    for index in 0..2 {
        let start = stats.cdr_offset as usize + index * CDR_V2_ENTRY_SIZE;
        let cdr = &bytes[start..start + CDR_V2_ENTRY_SIZE];
        assert_eq!(
            u16::from_le_bytes(cdr[0x00..0x02].try_into().unwrap()),
            CM_STORE as u16
        );
        assert_eq!(
            u32::from_le_bytes(cdr[0x06..0x0A].try_into().unwrap()),
            empty_crc32
        );
        assert_eq!(u64::from_le_bytes(cdr[0x0A..0x12].try_into().unwrap()), 0);
        assert_eq!(u64::from_le_bytes(cdr[0x12..0x1A].try_into().unwrap()), 0);
        assert_eq!(u64::from_le_bytes(cdr[0x1A..0x22].try_into().unwrap()), 0);
        assert_eq!(u16::from_le_bytes(cdr[0xAA..0xAC].try_into().unwrap()), 0);
        assert_eq!(&cdr[0xAC..0xCC], empty_sha256.as_slice());
    }

    for path in [&v2, &v1] {
        if path == &v1 {
            builder.write_v1_to_file(path).unwrap();
        }
        let archive = P4kArchive::open(path).unwrap();
        assert_eq!(archive.entry_count(), 2);
        for name in ["empty-bytes.bin", "empty-file.bin"] {
            let entry = archive.find(name).unwrap();
            assert_eq!(entry.compression_method, CompressionMethod::Store);
            assert!(!entry.is_encrypted);
            assert_eq!(entry.compressed_size, 0);
            assert_eq!(entry.uncompressed_size, 0);
            assert_eq!(entry.crc32, empty_crc32);
            assert_eq!(entry.sha256, empty_sha256);
            assert_eq!(archive.known_payload_offset(&entry), Some(0));
            assert!(archive.read(&entry).unwrap().is_empty());
            archive.verify_entry_integrity(&entry).unwrap();
        }
    }

    let _ = fs::remove_file(&empty_file);
    let _ = fs::remove_file(&v2);
    let _ = fs::remove_file(&v1);
}

#[test]
fn writer_empty_v2_entries_use_zero_data_offset_after_payloads() {
    let empty_file = tempfile_path("svarog_p4k_empty_after_payload_input.bin");
    let v2 = tempfile_path("svarog_p4k_empty_after_payload_writer_v2.p4k");
    fs::write(&empty_file, b"").unwrap();

    let mut options = P4kWriterOptions::default();
    options.sector_size = SECTOR_SIZE;
    options.compression = CompressionMethod::Store;
    let mut builder = P4kBuilder::with_options(options);
    builder.add_bytes("payload.bin", b"payload").unwrap();
    builder
        .add_file(&empty_file, "empty-after-payload.bin")
        .unwrap();

    let stats = builder.write_to_file(&v2).unwrap();
    let bytes = fs::read(&v2).unwrap();
    let archive = P4kArchive::open(&v2).unwrap();
    assert_eq!(archive.get(0).unwrap().name, "payload.bin");
    assert_eq!(archive.get(1).unwrap().name, "empty-after-payload.bin");

    let first_cdr = &bytes[stats.cdr_offset as usize..][..CDR_V2_ENTRY_SIZE];
    let second_cdr = &bytes[stats.cdr_offset as usize + CDR_V2_ENTRY_SIZE..][..CDR_V2_ENTRY_SIZE];
    assert_eq!(
        u64::from_le_bytes(first_cdr[0x1A..0x22].try_into().unwrap()),
        0
    );
    assert_eq!(
        u64::from_le_bytes(second_cdr[0x1A..0x22].try_into().unwrap()),
        0
    );
    assert!(archive
        .read(&archive.find("empty-after-payload.bin").unwrap())
        .unwrap()
        .is_empty());

    let _ = fs::remove_file(&empty_file);
    let _ = fs::remove_file(&v2);
}

#[test]
fn writer_rejects_precompressed_stored_size_mismatch() {
    let mut builder = P4kBuilder::new();
    let err = builder
        .add_precompressed(
            "bad-store.bin",
            b"not empty".to_vec(),
            0,
            CompressionMethod::Store,
            p4k_crc32(b""),
            false,
            [0; 128],
        )
        .unwrap_err();
    assert!(err
        .to_string()
        .contains("stored payload size must match uncompressed_size"));

    let err = builder
        .add_precompressed(
            "bad-encrypted-store.bin",
            vec![0u8; 15],
            1,
            CompressionMethod::Store,
            p4k_crc32(b"\0"),
            true,
            [0; 128],
        )
        .unwrap_err();
    assert!(err
        .to_string()
        .contains("encrypted payload must be block-aligned"));
}

#[test]
fn writer_rejects_precompressed_encrypted_compressed_unaligned_payload() {
    let mut builder = P4kBuilder::new();
    let err = builder
        .add_precompressed(
            "bad-encrypted-zstd.bin",
            vec![0u8; 15],
            1024,
            CompressionMethod::Zstd,
            p4k_crc32(&vec![0u8; 1024]),
            true,
            [0; 128],
        )
        .unwrap_err();
    assert!(err
        .to_string()
        .contains("encrypted payload must be block-aligned"));
}

#[test]
fn reader_rejects_stored_payload_with_zero_uncompressed_size() {
    let tmp = tempfile_path("svarog_p4k_bad_v2_zero_uncompressed_store.p4k");
    let entries = vec![V2EntrySpec {
        name: "bad-v2-zero-uncompressed-store.bin".to_string(),
        payload: b"hidden".to_vec(),
        uncompressed_size: 0,
        compression_method: CM_STORE,
        crc32: p4k_crc32(b""),
        encrypted: false,
    }];
    fs::write(&tmp, build_v2_archive(&entries, 0)).unwrap();

    let archive = P4kArchive::open(&tmp).unwrap();
    let err = archive.read(&archive.get(0).unwrap()).unwrap_err();
    assert!(
        err.to_string().contains("stored entry size mismatch"),
        "expected stored size mismatch, got {err}"
    );

    let _ = fs::remove_file(&tmp);
}

#[test]
fn dump_rejects_stored_payload_size_mismatch_on_fast_path() {
    let tmp = tempfile_path("svarog_p4k_bad_v2_dump_store_size.p4k");
    let out_dir = tempfile_path("svarog_p4k_bad_v2_dump_store_size_out");
    let _ = fs::remove_dir_all(&out_dir);
    let entries = vec![V2EntrySpec {
        name: "bad-v2-dump-store-size.bin".to_string(),
        payload: b"hidden".to_vec(),
        uncompressed_size: 0,
        compression_method: CM_STORE,
        crc32: p4k_crc32(b""),
        encrypted: false,
    }];
    fs::write(&tmp, build_v2_archive(&entries, 0)).unwrap();

    let archive = P4kArchive::open(&tmp).unwrap();
    let err = archive.dump_to_dir(&out_dir).unwrap_err();
    assert!(
        err.to_string().contains("stored entry size mismatch"),
        "expected stored size mismatch, got {err}"
    );

    let _ = fs::remove_file(&tmp);
    let _ = fs::remove_dir_all(&out_dir);
}

#[test]
fn dump_archive_to_directory_writes_normalized_paths() {
    let tmp = tempfile_path("svarog_p4k_dump_source.p4k");
    let out_dir = tempfile_path("svarog_p4k_dump_out");
    let _ = fs::remove_dir_all(&out_dir);

    let mut options = P4kWriterOptions::default();
    options.sector_size = SECTOR_SIZE;
    options.compression = CompressionMethod::Store;
    let mut builder = P4kBuilder::with_options(options);
    builder.add_bytes("a/b/c.txt", b"dumped").unwrap();
    builder.write_to_file(&tmp).unwrap();

    let archive = P4kArchive::open(&tmp).unwrap();
    archive.dump_to_dir(&out_dir).unwrap();
    assert_eq!(fs::read(out_dir.join("a/b/c.txt")).unwrap(), b"dumped");

    let _ = fs::remove_file(&tmp);
    let _ = fs::remove_dir_all(&out_dir);
}

#[test]
fn dump_archive_to_directory_preserves_order_for_duplicate_output_paths() {
    let tmp = tempfile_path("svarog_p4k_dump_duplicate_source.p4k");
    let out_dir = tempfile_path("svarog_p4k_dump_duplicate_out");
    let _ = fs::remove_dir_all(&out_dir);

    let entries = vec![
        V2EntrySpec {
            name: "dup.bin".to_string(),
            payload: b"first".to_vec(),
            uncompressed_size: 5,
            compression_method: CM_STORE,
            crc32: p4k_crc32(b"first"),
            encrypted: false,
        },
        V2EntrySpec {
            name: "dup.bin".to_string(),
            payload: b"second".to_vec(),
            uncompressed_size: 6,
            compression_method: CM_STORE,
            crc32: p4k_crc32(b"second"),
            encrypted: false,
        },
    ];
    fs::write(&tmp, build_v2_archive(&entries, 0)).unwrap();

    let archive = P4kArchive::open(&tmp).unwrap();
    archive.dump_to_dir(&out_dir).unwrap();
    assert_eq!(fs::read(out_dir.join("dup.bin")).unwrap(), b"second");

    let _ = fs::remove_file(&tmp);
    let _ = fs::remove_dir_all(&out_dir);
}

#[test]
fn extract_to_writer_streams_archive_entries() {
    let tmp = tempfile_path("svarog_p4k_extract_to_writer.p4k");
    let stored = b"streamed stored payload";
    let deflate = b"streamed raw deflate payload ".repeat(128);
    let zlib = b"streamed zlib payload ".repeat(128);
    let zstd = b"streamed zstd payload ".repeat(128);
    let encrypted_store = b"streamed encrypted store payload with real zeroes\0\0".repeat(128);
    let encrypted_deflate = b"streamed encrypted deflate payload ".repeat(128);
    let encrypted = b"streamed encrypted zstd payload ".repeat(128);

    let mut options = P4kWriterOptions::default();
    options.sector_size = SECTOR_SIZE;
    let mut builder = P4kBuilder::with_options(options);
    builder
        .add_bytes_with_method("stored.bin", stored, CompressionMethod::Store)
        .unwrap()
        .add_bytes_with_method("deflate.bin", &deflate, CompressionMethod::Deflate)
        .unwrap()
        .add_bytes_with_method("zlib.bin", &zlib, CompressionMethod::DeflateZlib)
        .unwrap()
        .add_bytes_with_method("zstd.bin", &zstd, CompressionMethod::Zstd)
        .unwrap()
        .add_bytes_with_method_encrypted(
            "encrypted-store.bin",
            &encrypted_store,
            CompressionMethod::Store,
        )
        .unwrap()
        .add_bytes_with_method_encrypted(
            "encrypted-deflate.bin",
            &encrypted_deflate,
            CompressionMethod::Deflate,
        )
        .unwrap()
        .add_bytes_with_method_encrypted("encrypted.bin", &encrypted, CompressionMethod::Zstd)
        .unwrap();
    builder.write_to_file(&tmp).unwrap();

    let archive = P4kArchive::open(&tmp).unwrap();
    for (name, expected) in [
        ("stored.bin", stored.as_slice()),
        ("deflate.bin", deflate.as_slice()),
        ("zlib.bin", zlib.as_slice()),
        ("zstd.bin", zstd.as_slice()),
        ("encrypted-store.bin", encrypted_store.as_slice()),
        ("encrypted-deflate.bin", encrypted_deflate.as_slice()),
        ("encrypted.bin", encrypted.as_slice()),
    ] {
        let entry = archive.find(name).unwrap();
        let mut output = Vec::new();
        archive.extract_to_writer(&entry, &mut output).unwrap();
        assert_eq!(output, expected);
    }
    let entry = archive.find("encrypted.bin").unwrap();
    let mut raw = Vec::new();
    archive
        .extract_raw_payload_to_writer(&entry, &mut raw)
        .unwrap();
    assert_eq!(raw.len(), entry.compressed_size as usize);
    assert_ne!(raw, encrypted);
    assert_eq!(sha256_array(&raw), entry.sha256);

    let _ = fs::remove_file(&tmp);
}

#[test]
fn convert_v1_to_v2_preserves_stored_payload() {
    let v1 = tempfile_path("svarog_p4k_convert_source_v1.p4k");
    let v2 = tempfile_path("svarog_p4k_convert_dest_v2.p4k");

    let mut options = P4kWriterOptions::default();
    options.sector_size = SECTOR_SIZE;
    options.compression = CompressionMethod::Store;
    let mut builder = P4kBuilder::with_options(options.clone());
    builder.add_bytes("plain.txt", b"payload").unwrap();
    builder.write_v1_to_file(&v1).unwrap();
    let stats = convert_v1_to_v2(&v1, &v2, options).unwrap();
    assert_eq!(stats.entry_count, 1);

    let source = fs::read(&v1).unwrap();
    let payload_offset = source
        .windows(b"payload".len())
        .position(|window| window == b"payload")
        .unwrap();
    let converted = fs::read(&v2).unwrap();
    let cdr = &converted[stats.cdr_offset as usize..][..CDR_V2_ENTRY_SIZE];
    assert_eq!(
        u64::from_le_bytes(cdr[0x1A..0x22].try_into().unwrap()),
        payload_offset as u64
    );

    let eocdr_start = converted.len() - EOCD_V2_SIZE;
    let eocdr = &converted[eocdr_start..eocdr_start + EOCD_V2_SIZE];
    assert_eq!(u64::from_le_bytes(eocdr[0x48..0x50].try_into().unwrap()), 1);
    let install_block = u64::from_le_bytes(eocdr[0x40..0x48].try_into().unwrap()) as usize;
    assert_eq!(
        u64::from_le_bytes(
            converted[install_block + 8..install_block + 16]
                .try_into()
                .unwrap()
        ),
        0
    );
    assert_eq!(
        u64::from_le_bytes(
            converted[install_block + 16..install_block + 24]
                .try_into()
                .unwrap()
        ),
        payload_offset as u64
    );

    let archive = P4kArchive::open(&v2).unwrap();
    assert_eq!(archive.version(), P4kVersion::V2);
    let entry = archive.get(0).unwrap();
    assert_eq!(entry.name, "plain.txt");
    assert_eq!(archive.read(&entry).unwrap(), b"payload");

    let _ = fs::remove_file(&v1);
    let _ = fs::remove_file(&v2);
}

#[test]
fn convert_v1_to_v2_carries_p4k_subarchive_zst_as_opaque_payload() {
    let v1 = tempfile_path("svarog_p4k_convert_source_v1_p4k_subarchive.p4k");
    let v2 = tempfile_path("svarog_p4k_convert_dest_v2_p4k_subarchive.p4k");
    let name = "Data/SubArchives/example.p4k_subarchive.zst";
    let payload = b"not a valid p4k_subarchive, but conversion must not parse it";

    let mut options = P4kWriterOptions::default();
    options.sector_size = SECTOR_SIZE;
    options.compression = CompressionMethod::Store;
    let mut builder = P4kBuilder::with_options(options.clone());
    builder.add_bytes(name, payload).unwrap();
    builder.write_v1_to_file(&v1).unwrap();

    let stats = convert_v1_to_v2(&v1, &v2, options).unwrap();
    assert_eq!(stats.entry_count, 1);

    let converted = P4kArchive::open(&v2).unwrap();
    assert_eq!(converted.version(), P4kVersion::V2);
    let entry = converted.find(name).unwrap();
    assert_eq!(entry.compression_method, CompressionMethod::Store);
    assert!(!entry.is_encrypted);
    assert_eq!(entry.compressed_size, payload.len() as u64);
    assert_eq!(entry.uncompressed_size, payload.len() as u64);
    assert_eq!(converted.read(&entry).unwrap(), payload);

    let source = fs::read(&v1).unwrap();
    let source_payload_offset = source
        .windows(payload.len())
        .position(|window| window == payload)
        .unwrap() as u64;
    let converted_bytes = fs::read(&v2).unwrap();
    let cdr = &converted_bytes[stats.cdr_offset as usize..][..CDR_V2_ENTRY_SIZE];
    assert_eq!(
        u64::from_le_bytes(cdr[0x1A..0x22].try_into().unwrap()),
        source_payload_offset
    );

    let _ = fs::remove_file(&v1);
    let _ = fs::remove_file(&v2);
}

#[test]
fn convert_v1_to_v2_preserves_encrypted_compressed_payload() {
    let v1 = tempfile_path("svarog_p4k_convert_source_v1_encrypted_zstd.p4k");
    let v2 = tempfile_path("svarog_p4k_convert_dest_v2_encrypted_zstd.p4k");

    let mut options = P4kWriterOptions::default();
    options.sector_size = SECTOR_SIZE;
    options.compression = CompressionMethod::Zstd;
    let plaintext = b"encrypted compressed conversion payload ".repeat(48);

    let mut builder = P4kBuilder::with_options(options.clone());
    builder
        .add_bytes_with_method_encrypted("secret.zstd", &plaintext, CompressionMethod::Zstd)
        .unwrap();
    builder.write_v1_to_file(&v1).unwrap();

    let source_archive = P4kArchive::open(&v1).unwrap();
    let source_entry = source_archive.get(0).unwrap();
    assert!(source_entry.is_encrypted);
    assert_eq!(source_entry.compression_method, CompressionMethod::Zstd);
    assert_eq!(source_archive.read(&source_entry).unwrap(), plaintext);

    let stats = convert_v1_to_v2(&v1, &v2, options).unwrap();
    let converted = fs::read(&v2).unwrap();
    let cdr = &converted[stats.cdr_offset as usize..][..CDR_V2_ENTRY_SIZE];
    let compressed_size = u64::from_le_bytes(cdr[0x0A..0x12].try_into().unwrap()) as usize;
    let payload_offset = u64::from_le_bytes(cdr[0x1A..0x22].try_into().unwrap()) as usize;
    assert_eq!(
        u16::from_le_bytes(cdr[0x00..0x02].try_into().unwrap()),
        CM_ZSTD as u16
    );
    assert_eq!(u16::from_le_bytes(cdr[0xAA..0xAC].try_into().unwrap()), 1);
    assert_eq!(&cdr[0xAC..0xCC], &source_entry.sha256);
    assert_eq!(compressed_size as u64, source_entry.compressed_size);
    assert_eq!(
        payload_offset as u64,
        source_entry.local_header_offset + local_v1_record_size("secret.zstd", SECTOR_SIZE)
    );

    let source = fs::read(&v1).unwrap();
    assert_eq!(
        &converted[payload_offset..payload_offset + compressed_size],
        &source[payload_offset..payload_offset + compressed_size]
    );

    let converted_archive = P4kArchive::open(&v2).unwrap();
    let converted_entry = converted_archive.get(0).unwrap();
    assert!(converted_entry.is_encrypted);
    assert_eq!(converted_entry.compression_method, CompressionMethod::Zstd);
    assert_eq!(converted_archive.read(&converted_entry).unwrap(), plaintext);

    let _ = fs::remove_file(&v1);
    let _ = fs::remove_file(&v2);
}

#[test]
fn convert_v1_to_v2_rejects_incompatible_destination_sector_size() {
    let v1 = tempfile_path("svarog_p4k_convert_bad_sector_source_v1.p4k");
    let v2 = tempfile_path("svarog_p4k_convert_bad_sector_dest_v2.p4k");

    let mut source_options = P4kWriterOptions::default();
    source_options.sector_size = SECTOR_SIZE;
    source_options.compression = CompressionMethod::Store;
    let mut builder = P4kBuilder::with_options(source_options);
    builder.add_bytes("plain.txt", b"payload").unwrap();
    builder.write_v1_to_file(&v1).unwrap();

    let mut dest_options = P4kWriterOptions::default();
    dest_options.sector_size = 4096;
    dest_options.compression = CompressionMethod::Store;
    let err = convert_v1_to_v2(&v1, &v2, dest_options).unwrap_err();
    assert!(
        err.to_string()
            .contains("incompatible with source payload offset"),
        "expected incompatible sector-size rejection, got {err}"
    );

    let _ = fs::remove_file(&v1);
    let _ = fs::remove_file(&v2);
}

#[test]
fn convert_v1_to_v2_rejects_same_source_and_destination() {
    let v1 = tempfile_path("svarog_p4k_convert_same_path_v1.p4k");

    let mut options = P4kWriterOptions::default();
    options.sector_size = SECTOR_SIZE;
    options.compression = CompressionMethod::Store;
    let mut builder = P4kBuilder::with_options(options.clone());
    builder.add_bytes("plain.txt", b"payload").unwrap();
    builder.write_v1_to_file(&v1).unwrap();

    let original = fs::read(&v1).unwrap();
    let err = convert_v1_to_v2(&v1, &v1, options).unwrap_err();
    assert!(
        err.to_string().contains("same file"),
        "expected same-path rejection, got {err}"
    );
    assert_eq!(fs::read(&v1).unwrap(), original);

    let _ = fs::remove_file(&v1);
}

#[test]
fn convert_v1_to_v2_cleans_temporary_output_when_publish_fails() {
    let v1 = tempfile_path("svarog_p4k_convert_publish_fail_v1.p4k");
    let root = tempfile_path("svarog_p4k_convert_publish_fail_root");
    let output = root.join("dest.p4k");

    fs::create_dir_all(&output).unwrap();

    let mut options = P4kWriterOptions::default();
    options.sector_size = SECTOR_SIZE;
    options.compression = CompressionMethod::Store;
    let mut builder = P4kBuilder::with_options(options.clone());
    builder.add_bytes("plain.txt", b"payload").unwrap();
    builder.write_v1_to_file(&v1).unwrap();

    let err = convert_v1_to_v2(&v1, &output, options).unwrap_err();
    assert!(
        err.to_string().contains("I/O error"),
        "expected final publish failure, got {err}"
    );
    assert!(
        output.is_dir(),
        "failed conversion must leave existing output directory intact"
    );

    let leaked_temp = fs::read_dir(&root)
        .unwrap()
        .filter_map(|entry| entry.ok())
        .any(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .contains(".dest.p4k.svarog-p4k-output-")
        });
    assert!(
        !leaked_temp,
        "temporary conversion output should be removed after publish failure"
    );

    let _ = fs::remove_file(&v1);
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn convert_v1_to_v2_handles_empty_entries_like_official_converter() {
    let v1 = tempfile_path("svarog_p4k_convert_empty_source_v1.p4k");
    let v2 = tempfile_path("svarog_p4k_convert_empty_dest_v2.p4k");

    let mut options = P4kWriterOptions::default();
    options.sector_size = SECTOR_SIZE;
    options.compression = CompressionMethod::Store;

    let mut builder = P4kBuilder::with_options(options.clone());
    builder.add_bytes("empty1.bin", b"").unwrap();
    builder.add_bytes("empty2.bin", b"").unwrap();
    builder.write_v1_to_file(&v1).unwrap();

    let stats = convert_v1_to_v2(&v1, &v2, options).unwrap();
    let converted = fs::read(&v2).unwrap();
    let cdr = &converted[stats.cdr_offset as usize..][..2 * CDR_V2_ENTRY_SIZE];
    assert_eq!(u64::from_le_bytes(cdr[0x0A..0x12].try_into().unwrap()), 0);
    assert_eq!(u64::from_le_bytes(cdr[0x1A..0x22].try_into().unwrap()), 0);
    assert_eq!(
        u64::from_le_bytes(
            cdr[CDR_V2_ENTRY_SIZE + 0x0A..CDR_V2_ENTRY_SIZE + 0x12]
                .try_into()
                .unwrap()
        ),
        0
    );
    assert_eq!(
        u64::from_le_bytes(
            cdr[CDR_V2_ENTRY_SIZE + 0x1A..CDR_V2_ENTRY_SIZE + 0x22]
                .try_into()
                .unwrap()
        ),
        0
    );

    let eocdr_start = converted.len() - EOCD_V2_SIZE;
    let eocdr = &converted[eocdr_start..eocdr_start + EOCD_V2_SIZE];
    assert_eq!(u64::from_le_bytes(eocdr[0x40..0x48].try_into().unwrap()), 0);
    assert_eq!(u64::from_le_bytes(eocdr[0x48..0x50].try_into().unwrap()), 1);
    assert_eq!(u64::from_le_bytes(converted[16..24].try_into().unwrap()), 0);
    assert_eq!(
        u64::from_le_bytes(converted[24..32].try_into().unwrap()),
        2 * local_v1_record_size("empty1.bin", SECTOR_SIZE)
    );

    let archive = P4kArchive::open(&v2).unwrap();
    assert_eq!(archive.version(), P4kVersion::V2);
    assert_eq!(archive.read(&archive.get(0).unwrap()).unwrap(), b"");
    assert_eq!(archive.read(&archive.get(1).unwrap()).unwrap(), b"");

    let _ = fs::remove_file(&v1);
    let _ = fs::remove_file(&v2);
}

#[test]
fn convert_v1_to_v2_preserves_empty_local_record_freelist_after_payload_end() {
    let v1 = tempfile_path("svarog_p4k_convert_mixed_empty_source_v1.p4k");
    let v2 = tempfile_path("svarog_p4k_convert_mixed_empty_dest_v2.p4k");

    let mut options = P4kWriterOptions::default();
    options.sector_size = SECTOR_SIZE;
    options.compression = CompressionMethod::Store;

    let mut builder = P4kBuilder::with_options(options.clone());
    builder
        .add_bytes("payload.bin", b"payload")
        .unwrap()
        .add_bytes("empty.bin", b"")
        .unwrap();
    builder.write_v1_to_file(&v1).unwrap();

    let non_empty_local_record = local_v1_record_size("payload.bin", SECTOR_SIZE);
    let empty_local_record = local_v1_record_size("empty.bin", SECTOR_SIZE);
    let expected_payload_end =
        (non_empty_local_record + b"payload".len() as u64 + SECTOR_SIZE - 1) & !(SECTOR_SIZE - 1);
    let stats = convert_v1_to_v2(&v1, &v2, options).unwrap();
    let converted = fs::read(&v2).unwrap();

    assert_eq!(stats.payload_end, expected_payload_end);
    let eocdr_start = converted.len() - EOCD_V2_SIZE;
    let eocdr = &converted[eocdr_start..eocdr_start + EOCD_V2_SIZE];
    assert_eq!(
        u64::from_le_bytes(eocdr[0x40..0x48].try_into().unwrap()),
        expected_payload_end
    );
    assert_eq!(u64::from_le_bytes(eocdr[0x48..0x50].try_into().unwrap()), 2);

    let install_block = stats.payload_end as usize;
    assert_eq!(
        u64::from_le_bytes(
            converted[install_block..install_block + 8]
                .try_into()
                .unwrap()
        ),
        b"payload".len() as u64
    );
    assert_eq!(
        u64::from_le_bytes(
            converted[install_block + 8..install_block + 16]
                .try_into()
                .unwrap()
        ),
        0
    );
    assert_eq!(
        u64::from_le_bytes(
            converted[install_block + 16..install_block + 24]
                .try_into()
                .unwrap()
        ),
        0
    );
    assert_eq!(
        u64::from_le_bytes(
            converted[install_block + 24..install_block + 32]
                .try_into()
                .unwrap()
        ),
        non_empty_local_record
    );
    assert_eq!(
        u64::from_le_bytes(
            converted[install_block + 32..install_block + 40]
                .try_into()
                .unwrap()
        ),
        expected_payload_end
    );
    assert_eq!(
        u64::from_le_bytes(
            converted[install_block + 40..install_block + 48]
                .try_into()
                .unwrap()
        ),
        empty_local_record
    );

    let cdr = &converted[stats.cdr_offset as usize..][..2 * CDR_V2_ENTRY_SIZE];
    assert_eq!(
        u64::from_le_bytes(cdr[0x1A..0x22].try_into().unwrap()),
        non_empty_local_record
    );
    assert_eq!(
        u64::from_le_bytes(
            cdr[CDR_V2_ENTRY_SIZE + 0x1A..CDR_V2_ENTRY_SIZE + 0x22]
                .try_into()
                .unwrap()
        ),
        0
    );

    let archive = P4kArchive::open(&v2).unwrap();
    assert_eq!(archive.version(), P4kVersion::V2);
    assert_eq!(
        archive.read(&archive.find("payload.bin").unwrap()).unwrap(),
        b"payload"
    );
    assert_eq!(
        archive.read(&archive.find("empty.bin").unwrap()).unwrap(),
        b""
    );

    let _ = fs::remove_file(&v1);
    let _ = fs::remove_file(&v2);
}

#[test]
fn convert_v1_to_v2_preserves_p4k_custom_metadata() {
    let v1 = tempfile_path("svarog_p4k_convert_source_v1_meta.p4k");
    let v2 = tempfile_path("svarog_p4k_convert_dest_v2_meta.p4k");
    let signature = std::array::from_fn(|i| (i ^ 0x5A) as u8);
    let sha256 = sha256_array(b"payload");
    let dos_time = 0x4A21;
    let dos_date = 0x5B3C;
    fs::write(
        &v1,
        build_zip64_v1_archive_with_p4k_metadata(
            "meta.bin", b"payload", signature, sha256, dos_time, dos_date, 1,
        ),
    )
    .unwrap();

    let mut options = P4kWriterOptions::default();
    options.sector_size = SECTOR_SIZE;
    options.compression = CompressionMethod::Store;
    let stats = convert_v1_to_v2(&v1, &v2, options).unwrap();

    let bytes = fs::read(&v2).unwrap();
    let cdr = &bytes[stats.cdr_offset as usize..][..CDR_V2_ENTRY_SIZE];
    assert_eq!(
        u16::from_le_bytes(cdr[0x02..0x04].try_into().unwrap()),
        dos_time
    );
    assert_eq!(
        u16::from_le_bytes(cdr[0x04..0x06].try_into().unwrap()),
        dos_date
    );
    assert_eq!(&cdr[0x2A..0xAA], &signature);
    assert_eq!(u16::from_le_bytes(cdr[0xAA..0xAC].try_into().unwrap()), 1);
    assert_eq!(&cdr[0xAC..0xCC], &sha256);

    let archive = P4kArchive::open(&v2).unwrap();
    assert_eq!(archive.version(), P4kVersion::V2);
    assert!(archive.get(0).unwrap().is_encrypted);
    assert!(archive.entries()[0].last_modified().is_some());

    let _ = fs::remove_file(&v1);
    let _ = fs::remove_file(&v2);
}

#[test]
fn convert_v1_to_v2_reads_v1_zip_local_payload_selected_by_sha256() {
    let v1 = tempfile_path("svarog_p4k_convert_source_v1_zip_local_payload.p4k");
    let v2 = tempfile_path("svarog_p4k_convert_dest_v2_zip_local_payload.p4k");
    let name = "retail-local.bin";
    let data = b"retail v1 payload at the ZIP local data offset";
    let signature = [0u8; 128];
    let sha256 = sha256_array(data);
    let mut bytes =
        build_zip64_v1_archive_with_p4k_metadata(name, data, signature, sha256, 0, 0, 0);
    move_v1_payload_to_zip_local_span(&mut bytes, name, data);
    fs::write(&v1, bytes).unwrap();

    let archive = P4kArchive::open(&v1).unwrap();
    assert_eq!(archive.read(&archive.get(0).unwrap()).unwrap(), data);

    let mut options = P4kWriterOptions::default();
    options.sector_size = SECTOR_SIZE;
    options.compression = CompressionMethod::Store;
    convert_v1_to_v2(&v1, &v2, options).unwrap();

    let converted = P4kArchive::open(&v2).unwrap();
    assert_eq!(converted.version(), P4kVersion::V2);
    assert_eq!(converted.read(&converted.get(0).unwrap()).unwrap(), data);

    let _ = fs::remove_file(&v1);
    let _ = fs::remove_file(&v2);
}

#[test]
fn convert_v1_to_v2_preserves_partial_install_progress() {
    let v1 = tempfile_path("svarog_p4k_convert_source_v1_partial_install.p4k");
    let v2 = tempfile_path("svarog_p4k_convert_dest_v2_partial_install.p4k");
    let signature = [0u8; 128];
    let sha256 = sha256_array(b"payload");
    fs::write(
        &v1,
        build_zip64_v1_archive_with_p4k_metadata_and_install(
            "partial.bin",
            b"payload",
            signature,
            sha256,
            0,
            0,
            0,
            3,
        ),
    )
    .unwrap();

    let mut options = P4kWriterOptions::default();
    options.sector_size = SECTOR_SIZE;
    options.compression = CompressionMethod::Store;
    let stats = convert_v1_to_v2(&v1, &v2, options).unwrap();

    let converted = fs::read(&v2).unwrap();
    assert_eq!(stats.file_size as usize, converted.len());
    let eocdr_start = converted.len() - EOCD_V2_SIZE;
    let eocdr = &converted[eocdr_start..eocdr_start + EOCD_V2_SIZE];
    assert_eq!(u64::from_le_bytes(eocdr[0x48..0x50].try_into().unwrap()), 1);
    let install_block = u64::from_le_bytes(eocdr[0x40..0x48].try_into().unwrap()) as usize;
    assert_eq!(
        u64::from_le_bytes(
            converted[install_block..install_block + 8]
                .try_into()
                .unwrap()
        ),
        3
    );
    let archive = P4kArchive::open(&v2).unwrap();
    let layout = archive.layout();
    assert_eq!(layout.file_size, converted.len() as u64);
    assert_eq!(layout.actual_content_end, converted.len() as u64);
    assert_eq!(layout.physical_sector_size, Some(SECTOR_SIZE));
    assert_eq!(layout.cdr_offset, stats.cdr_offset);
    assert_eq!(layout.cdr_size, stats.cdr_size);
    assert_eq!(layout.end_of_payload, Some(stats.payload_end));
    assert_eq!(layout.install_block_offset, Some(stats.payload_end));
    assert_eq!(
        layout.install_block_size,
        Some(stats.cdr_offset - stats.payload_end)
    );
    assert_eq!(layout.eocd_offset, eocdr_start as u64);
    let freelist = archive.freelist_blocks();
    assert_eq!(freelist.len(), 1);
    assert_eq!(freelist[0].offset, 0);
    assert_eq!(
        freelist[0].size,
        archive.get(0).unwrap().local_header_offset
    );
    assert_eq!(archive.read(&archive.get(0).unwrap()).unwrap(), b"payload");

    let _ = fs::remove_file(&v1);
    let _ = fs::remove_file(&v2);
}

#[test]
fn convert_v1_to_v2_reads_v1_install_entries_before_freelist_blocks() {
    let v1 = tempfile_path("svarog_p4k_convert_source_v1_with_freelist.p4k");
    let v2 = tempfile_path("svarog_p4k_convert_dest_v2_with_freelist.p4k");
    let signature = [0u8; 128];
    let sha256 = sha256_array(b"payload");
    fs::write(
        &v1,
        build_zip64_v1_archive_with_p4k_metadata_install_freelist(
            "freelist.bin",
            b"payload",
            signature,
            sha256,
            0,
            0,
            0,
            3,
            &[(0x1122_3344_5566_7788, 0x20)],
        ),
    )
    .unwrap();

    let mut options = P4kWriterOptions::default();
    options.sector_size = SECTOR_SIZE;
    options.compression = CompressionMethod::Store;
    let stats = convert_v1_to_v2(&v1, &v2, options).unwrap();

    let converted = fs::read(&v2).unwrap();
    assert_eq!(stats.file_size as usize, converted.len());
    let eocdr_start = converted.len() - EOCD_V2_SIZE;
    let eocdr = &converted[eocdr_start..eocdr_start + EOCD_V2_SIZE];
    assert_eq!(u64::from_le_bytes(eocdr[0x48..0x50].try_into().unwrap()), 2);
    let install_block = u64::from_le_bytes(eocdr[0x40..0x48].try_into().unwrap()) as usize;
    assert_eq!(
        u64::from_le_bytes(
            converted[install_block..install_block + 8]
                .try_into()
                .unwrap()
        ),
        3
    );
    let source = fs::read(&v1).unwrap();
    let payload_offset = source
        .windows(b"payload".len())
        .position(|window| window == b"payload")
        .unwrap() as u64;
    assert_eq!(
        u64::from_le_bytes(
            converted[install_block + 8..install_block + 16]
                .try_into()
                .unwrap()
        ),
        0
    );
    assert_eq!(
        u64::from_le_bytes(
            converted[install_block + 16..install_block + 24]
                .try_into()
                .unwrap()
        ),
        payload_offset
    );
    assert_eq!(
        u64::from_le_bytes(
            converted[install_block + 24..install_block + 32]
                .try_into()
                .unwrap()
        ),
        0x1122_3344_5566_7788
    );
    assert_eq!(
        u64::from_le_bytes(
            converted[install_block + 32..install_block + 40]
                .try_into()
                .unwrap()
        ),
        0x20
    );

    let archive = P4kArchive::open(&v2).unwrap();
    let layout = archive.layout();
    assert_eq!(layout.file_size, converted.len() as u64);
    assert_eq!(layout.actual_content_end, converted.len() as u64);
    assert_eq!(layout.physical_sector_size, Some(SECTOR_SIZE));
    assert_eq!(layout.cdr_offset, stats.cdr_offset);
    assert_eq!(layout.cdr_size, stats.cdr_size);
    assert_eq!(layout.end_of_payload, Some(stats.payload_end));
    assert_eq!(layout.install_block_offset, Some(stats.payload_end));
    assert_eq!(
        layout.install_block_size,
        Some(stats.cdr_offset - stats.payload_end)
    );
    assert_eq!(layout.eocd_offset, eocdr_start as u64);
    let freelist = archive.freelist_blocks();
    assert_eq!(freelist.len(), 2);
    assert_eq!(freelist[0].offset, 0);
    assert_eq!(freelist[0].size, payload_offset);
    assert_eq!(freelist[1].offset, 0x1122_3344_5566_7788);
    assert_eq!(freelist[1].size, 0x20);
    assert_eq!(archive.read(&archive.get(0).unwrap()).unwrap(), b"payload");

    let _ = fs::remove_file(&v1);
    let _ = fs::remove_file(&v2);
}

#[test]
fn convert_v1_to_v2_trims_existing_freelist_block_at_payload_end() {
    let v1 = tempfile_path("svarog_p4k_convert_source_v1_tail_freelist.p4k");
    let v2 = tempfile_path("svarog_p4k_convert_dest_v2_tail_freelist.p4k");
    let signature = [0u8; 128];
    let sha256 = sha256_array(b"payload");
    let base = build_zip64_v1_archive_with_p4k_metadata_install_freelist(
        "tail.bin",
        b"payload",
        signature,
        sha256,
        0,
        0,
        0,
        7,
        &[],
    );
    let payload_offset = base
        .windows(b"payload".len())
        .position(|window| window == b"payload")
        .unwrap() as u64;
    let payload_end = payload_offset + b"payload".len() as u64;
    let tail_end = (payload_end + SECTOR_SIZE - 1) & !(SECTOR_SIZE - 1);
    fs::write(
        &v1,
        build_zip64_v1_archive_with_p4k_metadata_install_freelist(
            "tail.bin",
            b"payload",
            signature,
            sha256,
            0,
            0,
            0,
            7,
            &[(payload_end, tail_end - payload_end)],
        ),
    )
    .unwrap();

    let mut options = P4kWriterOptions::default();
    options.sector_size = SECTOR_SIZE;
    options.compression = CompressionMethod::Store;
    let stats = convert_v1_to_v2(&v1, &v2, options).unwrap();

    let converted = fs::read(&v2).unwrap();
    let eocdr_start = converted.len() - EOCD_V2_SIZE;
    let eocdr = &converted[eocdr_start..eocdr_start + EOCD_V2_SIZE];
    assert_eq!(
        u64::from_le_bytes(eocdr[0x40..0x48].try_into().unwrap()),
        payload_end
    );
    assert_eq!(stats.payload_end, payload_end);
    assert_eq!(u64::from_le_bytes(eocdr[0x48..0x50].try_into().unwrap()), 1);

    let archive = P4kArchive::open(&v2).unwrap();
    assert_eq!(archive.read(&archive.get(0).unwrap()).unwrap(), b"payload");

    let _ = fs::remove_file(&v1);
    let _ = fs::remove_file(&v2);
}

#[test]
fn malformed_v1_p4k_extra_field_size_is_rejected() {
    let tmp = tempfile_path("svarog_p4k_bad_v1_extra_size.p4k");
    let signature = [0xA5; 128];
    let sha256 = sha256_array(b"payload");
    let mut bytes =
        build_zip64_v1_archive_with_p4k_metadata("bad.bin", b"payload", signature, sha256, 0, 0, 0);

    let marker = [0x00, 0x50, 0x84, 0x00];
    let field_pos = bytes
        .windows(marker.len())
        .position(|window| window == marker)
        .expect("0x5000 field marker");
    bytes[field_pos + 2..field_pos + 4].copy_from_slice(&0x83u16.to_le_bytes());
    fs::write(&tmp, bytes).unwrap();

    let err = P4kArchive::open(&tmp).unwrap_err();
    assert!(
        format!("{err}").contains("expected 0x0084"),
        "expected invalid 0x5000 size, got {err}"
    );

    let _ = fs::remove_file(&tmp);
}

#[test]
fn malformed_v1_cdr_zip64_disk_field_is_rejected() {
    let tmp = tempfile_path("svarog_p4k_bad_v1_cdr_zip64_disk.p4k");
    let signature = [0u8; 128];
    let sha256 = sha256_array(b"payload");
    let mut bytes = build_zip64_v1_archive_with_p4k_metadata(
        "bad-cdr-zip64.bin",
        b"payload",
        signature,
        sha256,
        0,
        0,
        0,
    );
    let cdr = bytes
        .windows(4)
        .position(|window| window == 0x0201_4b50u32.to_le_bytes())
        .expect("central directory record");
    let extra = cdr + 0x2E + "bad-cdr-zip64.bin".len();
    bytes[extra + 0x1C..extra + 0x20].copy_from_slice(&1u32.to_le_bytes());
    fs::write(&tmp, bytes).unwrap();

    let err = P4kArchive::open(&tmp).unwrap_err();
    assert!(
        err.to_string().contains("ZIP64 extra disk_number_start"),
        "expected CDR ZIP64 disk-field rejection, got {err}"
    );

    let _ = fs::remove_file(&tmp);
}

#[test]
fn malformed_v1_cdr_trailing_bytes_are_rejected() {
    let tmp = tempfile_path("svarog_p4k_bad_v1_cdr_trailing_bytes.p4k");
    let signature = [0u8; 128];
    let sha256 = sha256_array(b"payload");
    let mut bytes = build_zip64_v1_archive_with_p4k_metadata(
        "bad-cdr-trailing.bin",
        b"payload",
        signature,
        sha256,
        0,
        0,
        0,
    );

    let eocd64_offset = find_zip64_eocd(&bytes);
    bytes.insert(eocd64_offset, 0xEE);
    let new_eocd64_offset = eocd64_offset + 1;
    let cdr_size_offset = new_eocd64_offset + 40;
    let central_directory_record_size = u64::from_le_bytes(
        bytes[cdr_size_offset..cdr_size_offset + 8]
            .try_into()
            .unwrap(),
    );
    bytes[cdr_size_offset..cdr_size_offset + 8]
        .copy_from_slice(&(central_directory_record_size + 1).to_le_bytes());

    let locator_offset = find_zip64_locator(&bytes);
    bytes[locator_offset + 8..locator_offset + 16]
        .copy_from_slice(&(new_eocd64_offset as u64).to_le_bytes());
    bytes.resize(
        (bytes.len() + SECTOR_SIZE as usize - 1) & !(SECTOR_SIZE as usize - 1),
        0,
    );
    fs::write(&tmp, bytes).unwrap();

    let err = P4kArchive::open(&tmp).unwrap_err();
    assert!(
        err.to_string()
            .contains("central directory parser consumed"),
        "expected v1 CDR trailing-byte rejection, got {err}"
    );

    let _ = fs::remove_file(&tmp);
}

#[test]
fn malformed_v1_local_zip64_metadata_is_rejected() {
    let tmp = tempfile_path("svarog_p4k_bad_v1_local_zip64_metadata.p4k");
    let signature = [0u8; 128];
    let sha256 = sha256_array(b"payload");
    let mut bytes = build_zip64_v1_archive_with_p4k_metadata(
        "bad-local-zip64.bin",
        b"payload",
        signature,
        sha256,
        0,
        0,
        0,
    );
    let local_extra = 0x1E + "bad-local-zip64.bin".len();
    bytes[local_extra + 0x0C..local_extra + 0x14].copy_from_slice(&8u64.to_le_bytes());
    fs::write(&tmp, bytes).unwrap();

    let archive = P4kArchive::open(&tmp).unwrap();
    let err = archive.read(&archive.get(0).unwrap()).unwrap_err();
    assert!(
        err.to_string().contains("local ZIP64 metadata"),
        "expected local ZIP64 metadata rejection, got {err}"
    );

    let _ = fs::remove_file(&tmp);
}

#[test]
fn malformed_v1_local_dummy_extra_field_is_rejected() {
    let tmp = tempfile_path("svarog_p4k_bad_v1_local_dummy_extra.p4k");
    let signature = [0u8; 128];
    let sha256 = sha256_array(b"payload");
    let mut bytes = build_zip64_v1_archive_with_p4k_metadata(
        "bad-local-dummy.bin",
        b"payload",
        signature,
        sha256,
        0,
        0,
        0,
    );
    let local_extra = 0x1E + "bad-local-dummy.bin".len();
    bytes[local_extra + 0x20..local_extra + 0x22].copy_from_slice(&0x0667u16.to_le_bytes());
    fs::write(&tmp, bytes).unwrap();

    let archive = P4kArchive::open(&tmp).unwrap();
    let err = archive.read(&archive.get(0).unwrap()).unwrap_err();
    assert!(
        err.to_string().contains("local dummy extra id"),
        "expected local dummy-field rejection, got {err}"
    );

    let _ = fs::remove_file(&tmp);
}

#[test]
fn v1_local_dummy_extra_padding_may_be_nonzero() {
    let tmp = tempfile_path("svarog_p4k_v1_local_dummy_extra_nonzero_padding.p4k");
    let signature = [0u8; 128];
    let sha256 = sha256_array(b"payload");
    let mut bytes = build_zip64_v1_archive_with_p4k_metadata(
        "dummy-padding.bin",
        b"payload",
        signature,
        sha256,
        0,
        0,
        0,
    );
    let local_extra = 0x1E + "dummy-padding.bin".len();
    bytes[local_extra + 0x24] = 0xA5;
    bytes[local_extra + 0x25] = 0x5A;
    fs::write(&tmp, bytes).unwrap();

    let archive = P4kArchive::open(&tmp).unwrap();
    assert_eq!(archive.read(&archive.get(0).unwrap()).unwrap(), b"payload");

    let _ = fs::remove_file(&tmp);
}

#[test]
fn malformed_v1_cdr_fixed_field_is_rejected() {
    let tmp = tempfile_path("svarog_p4k_bad_v1_cdr_fixed.p4k");
    let signature = [0u8; 128];
    let sha256 = sha256_array(b"payload");
    let mut bytes = build_zip64_v1_archive_with_p4k_metadata(
        "bad-cdr.bin",
        b"payload",
        signature,
        sha256,
        0,
        0,
        0,
    );
    let cdr = bytes
        .windows(4)
        .position(|window| window == 0x0201_4b50u32.to_le_bytes())
        .expect("central directory record");
    bytes[cdr + 4..cdr + 6].copy_from_slice(&45u16.to_le_bytes());
    fs::write(&tmp, bytes).unwrap();

    let err = P4kArchive::open(&tmp).unwrap_err();
    assert!(
        err.to_string()
            .contains("central directory version_made_by"),
        "expected v1 CDR fixed-field rejection, got {err}"
    );

    let _ = fs::remove_file(&tmp);
}

#[test]
fn malformed_v1_zip64_eocd_fixed_field_is_rejected() {
    let tmp = tempfile_path("svarog_p4k_bad_v1_zip64_eocd.p4k");
    let signature = [0u8; 128];
    let sha256 = sha256_array(b"payload");
    let mut bytes = build_zip64_v1_archive_with_p4k_metadata(
        "bad-zip64-eocd.bin",
        b"payload",
        signature,
        sha256,
        0,
        0,
        0,
    );
    let zip64_eocd = find_zip64_eocd(&bytes);
    bytes[zip64_eocd + 4..zip64_eocd + 12].copy_from_slice(&43u64.to_le_bytes());
    fs::write(&tmp, bytes).unwrap();

    let err = P4kArchive::open(&tmp).unwrap_err();
    assert!(
        err.to_string().contains("ZIP64 EOCD record_size"),
        "expected v1 ZIP64 EOCD fixed-field rejection, got {err}"
    );

    let _ = fs::remove_file(&tmp);
}

#[test]
fn malformed_v1_zip64_eocd_entry_count_must_fit_official_loader() {
    let tmp = tempfile_path("svarog_p4k_bad_v1_zip64_count.p4k");
    let signature = [0u8; 128];
    let sha256 = sha256_array(b"payload");
    let mut bytes = build_zip64_v1_archive_with_p4k_metadata(
        "bad-zip64-count.bin",
        b"payload",
        signature,
        sha256,
        0,
        0,
        0,
    );
    let zip64_eocd = find_zip64_eocd(&bytes);
    let count = u32::MAX as u64 + 1;
    bytes[zip64_eocd + 0x18..zip64_eocd + 0x20].copy_from_slice(&count.to_le_bytes());
    bytes[zip64_eocd + 0x20..zip64_eocd + 0x28].copy_from_slice(&count.to_le_bytes());
    fs::write(&tmp, bytes).unwrap();

    let err = P4kArchive::open(&tmp).unwrap_err();
    assert!(
        err.to_string().contains("official v1 loader u32 limit"),
        "expected v1 ZIP64 entry-count rejection, got {err}"
    );

    let _ = fs::remove_file(&tmp);
}

#[test]
fn malformed_v1_zip64_locator_fixed_field_is_rejected() {
    let tmp = tempfile_path("svarog_p4k_bad_v1_zip64_locator.p4k");
    let signature = [0u8; 128];
    let sha256 = sha256_array(b"payload");
    let mut bytes = build_zip64_v1_archive_with_p4k_metadata(
        "bad-zip64-locator.bin",
        b"payload",
        signature,
        sha256,
        0,
        0,
        0,
    );
    let locator = find_zip64_locator(&bytes);
    bytes[locator + 16..locator + 20].copy_from_slice(&2u32.to_le_bytes());
    fs::write(&tmp, bytes).unwrap();

    let err = P4kArchive::open(&tmp).unwrap_err();
    assert!(
        err.to_string().contains("ZIP64 locator disk fields"),
        "expected v1 ZIP64 locator fixed-field rejection, got {err}"
    );

    let _ = fs::remove_file(&tmp);
}

#[test]
fn malformed_v1_install_progress_is_rejected() {
    let tmp = tempfile_path("svarog_p4k_bad_v1_install_progress.p4k");
    let signature = [0u8; 128];
    let sha256 = sha256_array(b"payload");
    fs::write(
        &tmp,
        build_zip64_v1_archive_with_p4k_metadata_and_install(
            "bad-install.bin",
            b"payload",
            signature,
            sha256,
            0,
            0,
            0,
            8,
        ),
    )
    .unwrap();

    let err = P4kArchive::open(&tmp).unwrap_err();
    assert!(
        format!("{err}").contains("bytes_already_written"),
        "expected invalid install progress, got {err}"
    );

    let _ = fs::remove_file(&tmp);
}

#[test]
fn malformed_v1_install_reserved_word_is_rejected() {
    let tmp = tempfile_path("svarog_p4k_bad_v1_install_reserved.p4k");
    let signature = [0u8; 128];
    let sha256 = sha256_array(b"payload");
    let mut bytes = build_zip64_v1_archive_with_p4k_metadata_and_install(
        "bad-install-reserved.bin",
        b"payload",
        signature,
        sha256,
        0,
        0,
        0,
        7,
    );
    let cdr = bytes
        .windows(4)
        .position(|window| window == 0x0201_4b50u32.to_le_bytes())
        .expect("central directory record");
    let install_reserved = cdr - 4;
    bytes[install_reserved..install_reserved + 4].copy_from_slice(&1u32.to_le_bytes());
    fs::write(&tmp, bytes).unwrap();

    let err = P4kArchive::open(&tmp).unwrap_err();
    assert!(
        err.to_string().contains("install reserved word"),
        "expected install reserved-word rejection, got {err}"
    );

    let _ = fs::remove_file(&tmp);
}

#[test]
fn malformed_v1_zero_freelist_block_is_rejected() {
    let tmp = tempfile_path("svarog_p4k_bad_v1_zero_freelist.p4k");
    let signature = [0u8; 128];
    let sha256 = sha256_array(b"payload");
    fs::write(
        &tmp,
        build_zip64_v1_archive_with_p4k_metadata_install_freelist(
            "bad-v1-zero-freelist.bin",
            b"payload",
            signature,
            sha256,
            0,
            0,
            0,
            7,
            &[(0x100, 0)],
        ),
    )
    .unwrap();

    let err = P4kArchive::open(&tmp).unwrap_err();
    assert!(
        err.to_string().contains("zero size"),
        "expected v1 zero-sized freelist rejection, got {err}"
    );

    let _ = fs::remove_file(&tmp);
}

#[test]
fn malformed_v1_overflowing_freelist_block_is_rejected() {
    let tmp = tempfile_path("svarog_p4k_bad_v1_overflow_freelist.p4k");
    let signature = [0u8; 128];
    let sha256 = sha256_array(b"payload");
    fs::write(
        &tmp,
        build_zip64_v1_archive_with_p4k_metadata_install_freelist(
            "bad-v1-overflow-freelist.bin",
            b"payload",
            signature,
            sha256,
            0,
            0,
            0,
            7,
            &[(u64::MAX, 2)],
        ),
    )
    .unwrap();

    let err = P4kArchive::open(&tmp).unwrap_err();
    assert!(
        err.to_string().contains("overflows u64"),
        "expected v1 overflowing freelist rejection, got {err}"
    );

    let _ = fs::remove_file(&tmp);
}

#[test]
fn reader_accepts_v1_local_header_redundant_method_mismatch() {
    let tmp = tempfile_path("svarog_p4k_v1_local_header_method_mismatch.p4k");
    let signature = [0u8; 128];
    let sha256 = sha256_array(b"payload");
    let mut bytes = build_zip64_v1_archive_with_p4k_metadata(
        "local-header-method.bin",
        b"payload",
        signature,
        sha256,
        0,
        0,
        0,
    );
    bytes[8..10].copy_from_slice(&(CM_DEFLATE as u16).to_le_bytes());
    fs::write(&tmp, bytes).unwrap();

    let archive = P4kArchive::open(&tmp).unwrap();
    let entry = archive.get(0).unwrap();
    assert_eq!(entry.compression_method, CompressionMethod::Store);
    assert_eq!(archive.read(&entry).unwrap(), b"payload");
    archive.verify_entry_integrity(&entry).unwrap();

    let _ = fs::remove_file(&tmp);
}

#[test]
fn reader_accepts_v1_local_header_redundant_timestamp_mismatch() {
    let tmp = tempfile_path("svarog_p4k_v1_local_header_timestamp_mismatch.p4k");
    let signature = [0u8; 128];
    let sha256 = sha256_array(b"payload");
    let mut bytes = build_zip64_v1_archive_with_p4k_metadata(
        "local-header-time.bin",
        b"payload",
        signature,
        sha256,
        0x1234,
        0x5678,
        0,
    );
    bytes[10..12].copy_from_slice(&0x4321u16.to_le_bytes());
    fs::write(&tmp, bytes).unwrap();

    let archive = P4kArchive::open(&tmp).unwrap();
    let entry = archive.get(0).unwrap();
    assert_eq!(entry.last_mod_file_time, 0x1234);
    assert_eq!(entry.last_mod_file_date, 0x5678);
    assert_eq!(archive.read(&entry).unwrap(), b"payload");
    archive.verify_entry_integrity(&entry).unwrap();

    let _ = fs::remove_file(&tmp);
}

#[test]
fn reader_rejects_v1_local_header_name_length_mismatch() {
    let tmp = tempfile_path("svarog_p4k_bad_v1_local_header_name_length.p4k");
    let signature = [0u8; 128];
    let sha256 = sha256_array(b"payload");
    let mut bytes = build_zip64_v1_archive_with_p4k_metadata(
        "bad-local-name.bin",
        b"payload",
        signature,
        sha256,
        0,
        0,
        0,
    );
    bytes[26..28].copy_from_slice(&1u16.to_le_bytes());
    fs::write(&tmp, bytes).unwrap();

    let archive = P4kArchive::open(&tmp).unwrap();
    let err = archive.read(&archive.get(0).unwrap()).unwrap_err();
    assert!(
        err.to_string().contains("local header span") || err.to_string().contains("file name"),
        "expected v1 local-header file-name length mismatch, got {err}"
    );

    let _ = fs::remove_file(&tmp);
}

#[test]
fn reader_accepts_v1_local_header_name_with_unnormalized_separators() {
    let tmp = tempfile_path("svarog_p4k_v1_local_header_unnormalized_name.p4k");
    let signature = [0u8; 128];
    let sha256 = sha256_array(b"payload");
    let mut bytes = build_zip64_v1_archive_with_p4k_metadata(
        "Data/local-name.bin",
        b"payload",
        signature,
        sha256,
        0,
        0,
        0,
    );
    let local_name_start = 0x1E;
    bytes[local_name_start + 4] = b'\\';
    fs::write(&tmp, bytes).unwrap();

    let archive = P4kArchive::open(&tmp).unwrap();
    let entry = archive.find("Data/local-name.bin").unwrap();
    assert_eq!(archive.read(&entry).unwrap(), b"payload");
    archive.verify_entry_integrity(&entry).unwrap();

    let _ = fs::remove_file(&tmp);
}

#[test]
fn reader_accepts_v1_local_header_name_with_different_case() {
    let tmp = tempfile_path("svarog_p4k_v1_local_header_case_name.p4k");
    let signature = [0u8; 128];
    let sha256 = sha256_array(b"payload");
    let mut bytes = build_zip64_v1_archive_with_p4k_metadata(
        "Data/Objects/Ships/Cleaver/file.bin",
        b"payload",
        signature,
        sha256,
        0,
        0,
        0,
    );
    let local_name_start = 0x1E;
    let local_name = b"Data\\Objects\\Ships\\cleaver\\file.bin";
    bytes[local_name_start..local_name_start + local_name.len()].copy_from_slice(local_name);
    fs::write(&tmp, bytes).unwrap();

    let archive = P4kArchive::open(&tmp).unwrap();
    let entry = archive.find("Data/Objects/Ships/Cleaver/file.bin").unwrap();
    assert_eq!(entry.name, "Data/Objects/Ships/Cleaver/file.bin");
    assert_eq!(archive.read(&entry).unwrap(), b"payload");
    archive.verify_entry_integrity(&entry).unwrap();

    let _ = fs::remove_file(&tmp);
}

#[test]
fn reader_rejects_v1_local_header_name_bytes_mismatch() {
    let tmp = tempfile_path("svarog_p4k_bad_v1_local_header_name_bytes.p4k");
    let signature = [0u8; 128];
    let sha256 = sha256_array(b"payload");
    let mut bytes = build_zip64_v1_archive_with_p4k_metadata(
        "bad-local-name.bin",
        b"payload",
        signature,
        sha256,
        0,
        0,
        0,
    );
    bytes[0x1E] = b'x';
    fs::write(&tmp, bytes).unwrap();

    let archive = P4kArchive::open(&tmp).unwrap();
    let err = archive.read(&archive.get(0).unwrap()).unwrap_err();
    assert!(
        err.to_string().contains("file name"),
        "expected v1 local-header file-name byte mismatch, got {err}"
    );

    let _ = fs::remove_file(&tmp);
}

#[test]
fn reader_rejects_v1_local_header_sector_span_mismatch() {
    let tmp = tempfile_path("svarog_p4k_bad_v1_local_header_sector_span.p4k");
    let signature = [0u8; 128];
    let sha256 = sha256_array(b"payload");
    let mut bytes = build_zip64_v1_archive_with_p4k_metadata(
        "bad-local-span.bin",
        b"payload",
        signature,
        sha256,
        0,
        0,
        0,
    );
    let extra_len = u16::from_le_bytes(bytes[28..30].try_into().unwrap());
    bytes[28..30].copy_from_slice(&(extra_len - 1).to_le_bytes());
    fs::write(&tmp, bytes).unwrap();

    let archive = P4kArchive::open(&tmp).unwrap();
    let err = archive.read(&archive.get(0).unwrap()).unwrap_err();
    assert!(
        err.to_string().contains("local header span"),
        "expected v1 local-header sector span mismatch, got {err}"
    );

    let _ = fs::remove_file(&tmp);
}

#[test]
fn reader_rejects_v1_non_boolean_encryption_field() {
    let tmp = tempfile_path("svarog_p4k_bad_v1_encryption_flag.p4k");
    let signature = [0u8; 128];
    let sha256 = sha256_array(b"payload");
    let mut bytes = build_zip64_v1_archive_with_p4k_metadata(
        "bad-v1-encryption.bin",
        b"payload",
        signature,
        sha256,
        0,
        0,
        0,
    );
    let marker = [0x02, 0x50, 0x06, 0x00];
    let field_pos = bytes
        .windows(marker.len())
        .position(|window| window == marker)
        .expect("0x5002 field marker");
    bytes[field_pos + 4..field_pos + 6].copy_from_slice(&2u16.to_le_bytes());
    fs::write(&tmp, bytes).unwrap();

    let err = P4kArchive::open(&tmp).unwrap_err();
    assert!(
        err.to_string().contains("P4K encryption field"),
        "expected v1 encryption-field rejection, got {err}"
    );

    let _ = fs::remove_file(&tmp);
}

#[test]
fn malformed_v2_install_progress_is_rejected() {
    let entries = vec![V2EntrySpec {
        name: "bad-v2-install.bin".to_string(),
        payload: b"payload".to_vec(),
        uncompressed_size: 7,
        compression_method: CM_STORE,
        crc32: 0,
        encrypted: false,
    }];
    let mut bytes = build_v2_archive(&entries, 0);
    let eocdr_start = bytes.len() - EOCD_V2_SIZE;
    let eocdr =
        <Eocd2Record as zerocopy::FromBytes>::read_from_bytes(&bytes[eocdr_start..]).unwrap();
    let install_start = eocdr.end_of_payload_offset as usize;
    bytes[install_start..install_start + 8].copy_from_slice(&8u64.to_le_bytes());

    let tmp = tempfile_path("svarog_p4k_bad_v2_install_progress.p4k");
    fs::write(&tmp, bytes).unwrap();

    let err = P4kArchive::open(&tmp).unwrap_err();
    assert!(
        format!("{err}").contains("bytes_already_written"),
        "expected invalid v2 install progress, got {err}"
    );

    let _ = fs::remove_file(&tmp);
}

#[test]
fn malformed_v2_freelist_install_span_is_rejected() {
    let entries = vec![V2EntrySpec {
        name: "bad-v2-freelist.bin".to_string(),
        payload: b"payload".to_vec(),
        uncompressed_size: 7,
        compression_method: CM_STORE,
        crc32: 0,
        encrypted: false,
    }];
    let mut bytes = build_v2_archive(&entries, 0);
    let eocdr_start = bytes.len() - EOCD_V2_SIZE;
    let eocdr =
        <Eocd2Record as zerocopy::FromBytes>::read_from_bytes(&bytes[eocdr_start..]).unwrap();
    let cdr_start = eocdr.central_directory_record_offset;
    bytes[eocdr_start + 0x40..eocdr_start + 0x48].copy_from_slice(&(cdr_start - 8).to_le_bytes());
    bytes[eocdr_start + 0x48..eocdr_start + 0x50].copy_from_slice(&1u64.to_le_bytes());

    let tmp = tempfile_path("svarog_p4k_bad_v2_freelist_install_span.p4k");
    fs::write(&tmp, bytes).unwrap();

    let err = P4kArchive::open(&tmp).unwrap_err();
    assert!(
        format!("{err}").contains("install block has"),
        "expected short install block rejection, got {err}"
    );

    let _ = fs::remove_file(&tmp);
}

#[test]
fn malformed_v2_zero_freelist_block_is_rejected() {
    let entries = vec![V2EntrySpec {
        name: "bad-v2-zero-freelist.bin".to_string(),
        payload: b"payload".to_vec(),
        uncompressed_size: 7,
        compression_method: CM_STORE,
        crc32: p4k_crc32(b"payload"),
        encrypted: false,
    }];
    let mut bytes = build_v2_archive(&entries, 0);
    let eocdr_start = bytes.len() - EOCD_V2_SIZE;
    let eocdr =
        <Eocd2Record as zerocopy::FromBytes>::read_from_bytes(&bytes[eocdr_start..]).unwrap();
    let install_start = eocdr.end_of_payload_offset as usize;
    bytes[eocdr_start + 0x48..eocdr_start + 0x50].copy_from_slice(&1u64.to_le_bytes());
    bytes[install_start + 8..install_start + 16].copy_from_slice(&0x100u64.to_le_bytes());
    bytes[install_start + 16..install_start + 24].copy_from_slice(&0u64.to_le_bytes());

    let tmp = tempfile_path("svarog_p4k_bad_v2_zero_freelist.p4k");
    fs::write(&tmp, bytes).unwrap();

    let err = P4kArchive::open(&tmp).unwrap_err();
    assert!(
        err.to_string().contains("zero size"),
        "expected v2 zero-sized freelist rejection, got {err}"
    );

    let _ = fs::remove_file(&tmp);
}

#[test]
fn malformed_v2_overflowing_freelist_block_is_rejected() {
    let entries = vec![V2EntrySpec {
        name: "bad-v2-overflow-freelist.bin".to_string(),
        payload: b"payload".to_vec(),
        uncompressed_size: 7,
        compression_method: CM_STORE,
        crc32: p4k_crc32(b"payload"),
        encrypted: false,
    }];
    let mut bytes = build_v2_archive(&entries, 0);
    let eocdr_start = bytes.len() - EOCD_V2_SIZE;
    let eocdr =
        <Eocd2Record as zerocopy::FromBytes>::read_from_bytes(&bytes[eocdr_start..]).unwrap();
    let install_start = eocdr.end_of_payload_offset as usize;
    bytes[eocdr_start + 0x48..eocdr_start + 0x50].copy_from_slice(&1u64.to_le_bytes());
    bytes[install_start + 8..install_start + 16].copy_from_slice(&u64::MAX.to_le_bytes());
    bytes[install_start + 16..install_start + 24].copy_from_slice(&2u64.to_le_bytes());

    let tmp = tempfile_path("svarog_p4k_bad_v2_overflow_freelist.p4k");
    fs::write(&tmp, bytes).unwrap();

    let err = P4kArchive::open(&tmp).unwrap_err();
    assert!(
        err.to_string().contains("overflows u64"),
        "expected v2 overflowing freelist rejection, got {err}"
    );

    let _ = fs::remove_file(&tmp);
}

#[test]
fn malformed_v2_trie_cache_offset_is_rejected() {
    let entries = vec![V2EntrySpec {
        name: "bad-v2-trie-cache.bin".to_string(),
        payload: b"payload".to_vec(),
        uncompressed_size: 7,
        compression_method: CM_STORE,
        crc32: p4k_crc32(b"payload"),
        encrypted: false,
    }];
    let mut bytes = build_v2_archive(&entries, 0);
    let eocdr_start = bytes.len() - EOCD_V2_SIZE;
    bytes[eocdr_start + 0x50..eocdr_start + 0x58].copy_from_slice(&1u64.to_le_bytes());
    bytes[eocdr_start + 0x58..eocdr_start + 0x60].copy_from_slice(&16u64.to_le_bytes());

    let tmp = tempfile_path("svarog_p4k_bad_v2_trie_cache_offset.p4k");
    fs::write(&tmp, bytes).unwrap();

    let err = P4kArchive::open(&tmp).unwrap_err();
    assert!(
        err.to_string().contains("trie cache offset"),
        "expected trie-cache-offset rejection, got {err}"
    );

    let _ = fs::remove_file(&tmp);
}

#[test]
fn malformed_v2_eocdr_incomplete_flag_is_rejected() {
    let entries = vec![V2EntrySpec {
        name: "bad-v2-flag.bin".to_string(),
        payload: b"payload".to_vec(),
        uncompressed_size: 7,
        compression_method: CM_STORE,
        crc32: p4k_crc32(b"payload"),
        encrypted: false,
    }];
    let mut bytes = build_v2_archive(&entries, 0);
    let eocdr_start = bytes.len() - EOCD_V2_SIZE;
    bytes[eocdr_start + 0x68] = 1;

    let tmp = tempfile_path("svarog_p4k_bad_v2_eocdr_incomplete_flag.p4k");
    fs::write(&tmp, bytes).unwrap();

    let err = P4kArchive::open(&tmp).unwrap_err();
    assert!(
        err.to_string().contains("incomplete bit"),
        "expected incomplete-flag rejection, got {err}"
    );

    let _ = fs::remove_file(&tmp);
}

#[test]
fn malformed_v2_non_boolean_encryption_flag_is_rejected() {
    let entries = vec![V2EntrySpec {
        name: "bad-v2-encryption.bin".to_string(),
        payload: b"payload".to_vec(),
        uncompressed_size: 7,
        compression_method: CM_STORE,
        crc32: p4k_crc32(b"payload"),
        encrypted: false,
    }];
    let mut bytes = build_v2_archive(&entries, 0);
    let eocdr_start = bytes.len() - EOCD_V2_SIZE;
    let eocdr =
        <Eocd2Record as zerocopy::FromBytes>::read_from_bytes(&bytes[eocdr_start..]).unwrap();
    let cdr_start = eocdr.central_directory_record_offset as usize;
    bytes[cdr_start + 0xAA..cdr_start + 0xAC].copy_from_slice(&2u16.to_le_bytes());

    let tmp = tempfile_path("svarog_p4k_bad_v2_encryption_flag.p4k");
    fs::write(&tmp, bytes).unwrap();

    let err = P4kArchive::open(&tmp).unwrap_err();
    assert!(
        err.to_string().contains("encryption flag"),
        "expected v2 encryption-flag rejection, got {err}"
    );

    let _ = fs::remove_file(&tmp);
}

#[test]
fn malformed_v2_cdr_alignment_is_rejected() {
    let entries = vec![V2EntrySpec {
        name: "bad-v2-cdr-align.bin".to_string(),
        payload: b"payload".to_vec(),
        uncompressed_size: 7,
        compression_method: CM_STORE,
        crc32: p4k_crc32(b"payload"),
        encrypted: false,
    }];
    let mut bytes = build_v2_archive(&entries, 0);
    let eocdr_start = bytes.len() - EOCD_V2_SIZE;
    bytes[eocdr_start + 0x10..eocdr_start + 0x18].copy_from_slice(&0x1234u64.to_le_bytes());

    let tmp = tempfile_path("svarog_p4k_bad_v2_cdr_alignment.p4k");
    fs::write(&tmp, bytes).unwrap();

    let err = P4kArchive::open(&tmp).unwrap_err();
    assert!(
        err.to_string().contains("CDR offset"),
        "expected CDR-alignment rejection, got {err}"
    );

    let _ = fs::remove_file(&tmp);
}

#[test]
fn malformed_v2_oversized_cdr_size_is_rejected_before_slicing() {
    let entries = vec![V2EntrySpec {
        name: "bad-v2-cdr-size.bin".to_string(),
        payload: b"payload".to_vec(),
        uncompressed_size: 7,
        compression_method: CM_STORE,
        crc32: p4k_crc32(b"payload"),
        encrypted: false,
    }];
    let mut bytes = build_v2_archive(&entries, 0);
    let eocdr_start = bytes.len() - EOCD_V2_SIZE;
    bytes[eocdr_start + 0x18..eocdr_start + 0x20].copy_from_slice(&u64::MAX.to_le_bytes());

    let tmp = tempfile_path("svarog_p4k_bad_v2_cdr_size.p4k");
    fs::write(&tmp, bytes).unwrap();

    let err = P4kArchive::open(&tmp).unwrap_err();
    assert!(
        err.to_string().contains("CDR size") || err.to_string().contains("cdr_size"),
        "expected oversized CDR-size rejection, got {err}"
    );

    let _ = fs::remove_file(&tmp);
}

#[test]
fn malformed_v2_data_offset_alignment_is_rejected() {
    let entries = vec![V2EntrySpec {
        name: "bad-v2-data-align.bin".to_string(),
        payload: b"payload".to_vec(),
        uncompressed_size: 7,
        compression_method: CM_STORE,
        crc32: p4k_crc32(b"payload"),
        encrypted: false,
    }];
    let mut bytes = build_v2_archive(&entries, 0);
    let eocdr_start = bytes.len() - EOCD_V2_SIZE;
    let eocdr =
        <Eocd2Record as zerocopy::FromBytes>::read_from_bytes(&bytes[eocdr_start..]).unwrap();
    let cdr_start = eocdr.central_directory_record_offset as usize;
    bytes[cdr_start + 0x1A..cdr_start + 0x22].copy_from_slice(&1u64.to_le_bytes());

    let tmp = tempfile_path("svarog_p4k_bad_v2_data_alignment.p4k");
    fs::write(&tmp, bytes).unwrap();

    let err = P4kArchive::open(&tmp).unwrap_err();
    assert!(
        err.to_string().contains("not aligned"),
        "expected data-offset alignment rejection, got {err}"
    );

    let _ = fs::remove_file(&tmp);
}

#[test]
fn malformed_v2_oversized_name_offset_is_rejected_before_slicing() {
    let entries = vec![V2EntrySpec {
        name: "bad-v2-name-offset.bin".to_string(),
        payload: b"payload".to_vec(),
        uncompressed_size: 7,
        compression_method: CM_STORE,
        crc32: p4k_crc32(b"payload"),
        encrypted: false,
    }];
    let mut bytes = build_v2_archive(&entries, 0);
    let eocdr_start = bytes.len() - EOCD_V2_SIZE;
    let eocdr =
        <Eocd2Record as zerocopy::FromBytes>::read_from_bytes(&bytes[eocdr_start..]).unwrap();
    let cdr_start = eocdr.central_directory_record_offset as usize;
    bytes[cdr_start + 0x22..cdr_start + 0x2A].copy_from_slice(&u64::MAX.to_le_bytes());

    let tmp = tempfile_path("svarog_p4k_bad_v2_huge_name_offset.p4k");
    fs::write(&tmp, bytes).unwrap();

    let err = P4kArchive::open(&tmp).unwrap_err();
    assert!(
        err.to_string().contains("name offset"),
        "expected oversized name-offset rejection, got {err}"
    );

    let _ = fs::remove_file(&tmp);
}

#[test]
fn v2_name_offsets_may_skip_official_writer_slack() {
    let entries = vec![
        V2EntrySpec {
            name: "first.bin".to_string(),
            payload: b"first".to_vec(),
            uncompressed_size: 5,
            compression_method: CM_STORE,
            crc32: p4k_crc32(b"first"),
            encrypted: false,
        },
        V2EntrySpec {
            name: "second.bin".to_string(),
            payload: b"second".to_vec(),
            uncompressed_size: 6,
            compression_method: CM_STORE,
            crc32: p4k_crc32(b"second"),
            encrypted: false,
        },
    ];
    let bytes = insert_v2_name_table_slack_before_entry(
        build_v2_archive(&entries, 0),
        1,
        b"ignored-stale-bytes",
    );

    let tmp = tempfile_path("svarog_p4k_v2_name_offset_slack.p4k");
    fs::write(&tmp, bytes).unwrap();

    let archive = P4kArchive::open(&tmp).unwrap();
    let entries = archive.entries();
    let names: Vec<_> = entries.iter().map(|entry| entry.name()).collect();
    assert_eq!(names, vec!["first.bin", "second.bin"]);

    let _ = fs::remove_file(&tmp);
}

#[test]
fn malformed_v2_name_offsets_must_not_overlap_previous_name() {
    let entries = vec![
        V2EntrySpec {
            name: "first.bin".to_string(),
            payload: b"first".to_vec(),
            uncompressed_size: 5,
            compression_method: CM_STORE,
            crc32: p4k_crc32(b"first"),
            encrypted: false,
        },
        V2EntrySpec {
            name: "second.bin".to_string(),
            payload: b"second".to_vec(),
            uncompressed_size: 6,
            compression_method: CM_STORE,
            crc32: p4k_crc32(b"second"),
            encrypted: false,
        },
    ];
    let mut bytes = build_v2_archive(&entries, 0);
    let eocdr_start = bytes.len() - EOCD_V2_SIZE;
    let eocdr =
        <Eocd2Record as zerocopy::FromBytes>::read_from_bytes(&bytes[eocdr_start..]).unwrap();
    let cdr_start = eocdr.central_directory_record_offset as usize;
    let second_entry = cdr_start + CDR_V2_ENTRY_SIZE;
    bytes[second_entry + 0x22..second_entry + 0x2A].copy_from_slice(&0u64.to_le_bytes());

    let tmp = tempfile_path("svarog_p4k_bad_v2_name_offset.p4k");
    fs::write(&tmp, bytes).unwrap();

    let err = P4kArchive::open(&tmp).unwrap_err();
    assert!(
        err.to_string().contains("before previous name end"),
        "expected v2 overlapping name-offset rejection, got {err}"
    );

    let _ = fs::remove_file(&tmp);
}

#[test]
fn malformed_v2_name_table_entry_must_be_normalized() {
    let entries = vec![V2EntrySpec {
        name: "bad\\path.bin".to_string(),
        payload: b"payload".to_vec(),
        uncompressed_size: 7,
        compression_method: CM_STORE,
        crc32: p4k_crc32(b"payload"),
        encrypted: false,
    }];
    let bytes = build_v2_archive(&entries, 0);

    let tmp = tempfile_path("svarog_p4k_bad_v2_name_normalization.p4k");
    fs::write(&tmp, bytes).unwrap();

    let err = P4kArchive::open(&tmp).unwrap_err();
    assert!(
        err.to_string().contains("not normalized"),
        "expected v2 name normalization rejection, got {err}"
    );

    let _ = fs::remove_file(&tmp);
}

#[test]
fn malformed_v2_payload_overlap_install_block_is_rejected() {
    let entries = vec![V2EntrySpec {
        name: "bad-v2-payload-span.bin".to_string(),
        payload: b"payload".to_vec(),
        uncompressed_size: 7,
        compression_method: CM_STORE,
        crc32: p4k_crc32(b"payload"),
        encrypted: false,
    }];
    let mut bytes = build_v2_archive(&entries, 0);
    let eocdr_start = bytes.len() - EOCD_V2_SIZE;
    let eocdr =
        <Eocd2Record as zerocopy::FromBytes>::read_from_bytes(&bytes[eocdr_start..]).unwrap();
    let cdr_start = eocdr.central_directory_record_offset as usize;
    let overlapping_size = eocdr.end_of_payload_offset + 1;
    bytes[cdr_start + 0x0A..cdr_start + 0x12].copy_from_slice(&overlapping_size.to_le_bytes());

    let tmp = tempfile_path("svarog_p4k_bad_v2_payload_span.p4k");
    fs::write(&tmp, bytes).unwrap();

    let err = P4kArchive::open(&tmp).unwrap_err();
    assert!(
        err.to_string().contains("end_of_payload"),
        "expected payload/end_of_payload rejection, got {err}"
    );

    let _ = fs::remove_file(&tmp);
}

#[test]
fn malformed_v2_overlapping_payload_ranges_are_rejected() {
    let entries = vec![
        V2EntrySpec {
            name: "first-overlap.bin".to_string(),
            payload: b"first".to_vec(),
            uncompressed_size: 5,
            compression_method: CM_STORE,
            crc32: p4k_crc32(b"first"),
            encrypted: false,
        },
        V2EntrySpec {
            name: "second-overlap.bin".to_string(),
            payload: b"second".to_vec(),
            uncompressed_size: 6,
            compression_method: CM_STORE,
            crc32: p4k_crc32(b"second"),
            encrypted: false,
        },
    ];
    let mut bytes = build_v2_archive(&entries, 0);
    let eocdr_start = bytes.len() - EOCD_V2_SIZE;
    let eocdr =
        <Eocd2Record as zerocopy::FromBytes>::read_from_bytes(&bytes[eocdr_start..]).unwrap();
    let cdr_start = eocdr.central_directory_record_offset as usize;
    let second_entry = cdr_start + CDR_V2_ENTRY_SIZE;
    bytes[second_entry + 0x1A..second_entry + 0x22].copy_from_slice(&0u64.to_le_bytes());

    let tmp = tempfile_path("svarog_p4k_bad_v2_payload_overlap.p4k");
    fs::write(&tmp, bytes).unwrap();

    let err = P4kArchive::open(&tmp).unwrap_err();
    assert!(
        err.to_string().contains("overlaps entry"),
        "expected v2 payload overlap rejection, got {err}"
    );

    let _ = fs::remove_file(&tmp);
}

#[test]
fn malformed_v2_install_padding_is_rejected() {
    let entries = vec![V2EntrySpec {
        name: "bad-v2-install-padding.bin".to_string(),
        payload: b"payload".to_vec(),
        uncompressed_size: 7,
        compression_method: CM_STORE,
        crc32: p4k_crc32(b"payload"),
        encrypted: false,
    }];
    let mut bytes = build_v2_archive(&entries, 0);
    let eocdr_start = bytes.len() - EOCD_V2_SIZE;
    let eocdr =
        <Eocd2Record as zerocopy::FromBytes>::read_from_bytes(&bytes[eocdr_start..]).unwrap();
    let padding_offset = eocdr.end_of_payload_offset as usize + 8;
    bytes[padding_offset] = 0xA5;

    let tmp = tempfile_path("svarog_p4k_bad_v2_install_padding.p4k");
    fs::write(&tmp, bytes).unwrap();

    let err = P4kArchive::open(&tmp).unwrap_err();
    assert!(
        err.to_string().contains("install padding"),
        "expected install-padding rejection, got {err}"
    );

    let _ = fs::remove_file(&tmp);
}

#[test]
fn malformed_v2_sector_size_is_rejected() {
    let entries = vec![V2EntrySpec {
        name: "bad-v2-sector.bin".to_string(),
        payload: b"payload".to_vec(),
        uncompressed_size: 7,
        compression_method: CM_STORE,
        crc32: p4k_crc32(b"payload"),
        encrypted: false,
    }];
    let mut bytes = build_v2_archive(&entries, 0);
    let eocdr_start = bytes.len() - EOCD_V2_SIZE;
    bytes[eocdr_start + 0x60..eocdr_start + 0x68].copy_from_slice(&3u64.to_le_bytes());

    let tmp = tempfile_path("svarog_p4k_bad_v2_sector.p4k");
    fs::write(&tmp, bytes).unwrap();

    let err = P4kArchive::open(&tmp).unwrap_err();
    assert!(
        err.to_string().contains("physical sector size"),
        "expected bad sector-size rejection, got {err}"
    );

    let _ = fs::remove_file(&tmp);
}

#[test]
fn malformed_v2_eof_buffer_placement_is_rejected() {
    let entries = vec![V2EntrySpec {
        name: "bad-v2-eof.bin".to_string(),
        payload: b"payload".to_vec(),
        uncompressed_size: 7,
        compression_method: CM_STORE,
        crc32: p4k_crc32(b"payload"),
        encrypted: false,
    }];
    let mut bytes = build_v2_archive(&entries, 0);
    let eocdr_start = bytes.len() - EOCD_V2_SIZE;
    let removed = bytes.remove(eocdr_start - 1);
    assert_eq!(removed, 0);
    bytes.push(0);

    let tmp = tempfile_path("svarog_p4k_bad_v2_eof_buffer.p4k");
    fs::write(&tmp, bytes).unwrap();

    let err = P4kArchive::open(&tmp).unwrap_err();
    assert!(
        err.to_string().contains("EOF buffer ends"),
        "expected EOF-buffer placement rejection, got {err}"
    );

    let _ = fs::remove_file(&tmp);
}

#[test]
fn v1_files_still_parse_as_v1() {
    // A file that is too short to even hold a v2 trailer should
    // gracefully fall through to the v1 parser (which will report
    // EocdNotFound for an empty file).
    let tmp = tempfile_path("svarog_p4k_v2_not_v2.p4k");
    fs::write(&tmp, b"this is definitely not a p4k").unwrap();
    let err = P4kArchive::open(&tmp).unwrap_err();
    let msg = format!("{}", err);
    assert!(
        msg.contains("end of central directory"),
        "expected v1 EOCD error, got: {}",
        msg
    );

    let _ = fs::remove_file(&tmp);
}

#[test]
fn truncated_data_offset_rejected_at_open() {
    use zerocopy::IntoBytes;

    let entries = vec![V2EntrySpec {
        name: "broken.bin".to_string(),
        payload: vec![0u8; 16],
        uncompressed_size: 16,
        compression_method: CM_STORE,
        crc32: 0,
        encrypted: false,
    }];
    let mut bytes = build_v2_archive(&entries, 0);

    // Patch the entry's offset_to_file_data to point past end-of-file
    // so the v2 parser's bounds check fires at open() rather than
    // during read().
    let eocdr_start = bytes.len() - EOCD_V2_SIZE;
    let eocdr =
        <Eocd2Record as zerocopy::FromBytes>::read_from_bytes(&bytes[eocdr_start..]).unwrap();
    let cdr_start = eocdr.central_directory_record_offset as usize;
    let mut entry = <CentralDirectoryHeaderV2 as zerocopy::FromBytes>::read_from_bytes(
        &bytes[cdr_start..cdr_start + CDR_V2_ENTRY_SIZE],
    )
    .unwrap();
    entry.offset_to_file_data = (bytes.len() as u64).saturating_add(1024);
    bytes[cdr_start..cdr_start + CDR_V2_ENTRY_SIZE].copy_from_slice(entry.as_bytes());

    let tmp = tempfile_path("svarog_p4k_v2_truncated.p4k");
    fs::write(&tmp, &bytes).unwrap();
    let err = P4kArchive::open(&tmp).unwrap_err();
    let msg = format!("{}", err);
    assert!(
        msg.contains("v2 CDR entry") || msg.contains("MalformedV2Entry"),
        "expected MalformedV2Entry, got: {}",
        msg
    );

    let _ = fs::remove_file(&tmp);
}

fn tempfile_path(name: &str) -> std::path::PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!("{}-{}", std::process::id(), name));
    p
}

fn build_zip64_v1_archive_with_p4k_metadata(
    name: &str,
    data: &[u8],
    signature: [u8; 128],
    sha256: [u8; 32],
    dos_time: u16,
    dos_date: u16,
    encryption: u16,
) -> Vec<u8> {
    build_zip64_v1_archive_with_p4k_metadata_and_install(
        name,
        data,
        signature,
        sha256,
        dos_time,
        dos_date,
        encryption,
        data.len() as u64,
    )
}

fn build_zip64_v1_archive_with_p4k_metadata_and_install(
    name: &str,
    data: &[u8],
    signature: [u8; 128],
    sha256: [u8; 32],
    dos_time: u16,
    dos_date: u16,
    encryption: u16,
    bytes_already_written: u64,
) -> Vec<u8> {
    build_zip64_v1_archive_with_p4k_metadata_install_freelist(
        name,
        data,
        signature,
        sha256,
        dos_time,
        dos_date,
        encryption,
        bytes_already_written,
        &[],
    )
}

fn build_zip64_v1_archive_with_p4k_metadata_install_freelist(
    name: &str,
    data: &[u8],
    signature: [u8; 128],
    sha256: [u8; 32],
    dos_time: u16,
    dos_date: u16,
    encryption: u16,
    bytes_already_written: u64,
    freelist_blocks: &[(u64, u64)],
) -> Vec<u8> {
    let crc = p4k_crc32(data);
    let name_bytes = name.as_bytes();
    let mut bytes = Vec::new();

    let local_header_offset = bytes.len() as u64;
    bytes.extend_from_slice(&0x1403_4b50u32.to_le_bytes());
    bytes.extend_from_slice(&45u16.to_le_bytes());
    bytes.extend_from_slice(&0u16.to_le_bytes());
    bytes.extend_from_slice(&(CM_STORE as u16).to_le_bytes());
    bytes.extend_from_slice(&dos_time.to_le_bytes());
    bytes.extend_from_slice(&dos_date.to_le_bytes());
    bytes.extend_from_slice(&crc.to_le_bytes());
    bytes.extend_from_slice(&u32::MAX.to_le_bytes());
    bytes.extend_from_slice(&u32::MAX.to_le_bytes());
    bytes.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
    let local_extra_len = SECTOR_SIZE as usize - 0x1E - name_bytes.len();
    bytes.extend_from_slice(&(local_extra_len as u16).to_le_bytes());
    bytes.extend_from_slice(name_bytes);
    bytes.extend_from_slice(&0x0001u16.to_le_bytes());
    bytes.extend_from_slice(&0x20u16.to_le_bytes());
    bytes.extend_from_slice(&(data.len() as u64).to_le_bytes());
    bytes.extend_from_slice(&(data.len() as u64).to_le_bytes());
    bytes.extend_from_slice(&local_header_offset.to_le_bytes());
    bytes.extend_from_slice(&0u32.to_le_bytes());
    let dummy_total_size = local_extra_len - 0x20;
    bytes.extend_from_slice(&0x0666u16.to_le_bytes());
    bytes.extend_from_slice(&(dummy_total_size as u16).to_le_bytes());
    bytes.resize(
        local_header_offset as usize + local_v1_record_size(name, SECTOR_SIZE) as usize,
        0,
    );
    bytes.extend_from_slice(data);

    bytes.extend_from_slice(&bytes_already_written.to_le_bytes());
    bytes.extend_from_slice(&0u32.to_le_bytes());
    for (offset, size) in freelist_blocks {
        bytes.extend_from_slice(&offset.to_le_bytes());
        bytes.extend_from_slice(&size.to_le_bytes());
    }

    let central_dir_offset = bytes.len() as u64;
    bytes.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
    bytes.extend_from_slice(&46u16.to_le_bytes());
    bytes.extend_from_slice(&45u16.to_le_bytes());
    bytes.extend_from_slice(&0u16.to_le_bytes());
    bytes.extend_from_slice(&(CM_STORE as u16).to_le_bytes());
    bytes.extend_from_slice(&dos_time.to_le_bytes());
    bytes.extend_from_slice(&dos_date.to_le_bytes());
    bytes.extend_from_slice(&crc.to_le_bytes());
    bytes.extend_from_slice(&u32::MAX.to_le_bytes());
    bytes.extend_from_slice(&u32::MAX.to_le_bytes());
    bytes.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
    bytes.extend_from_slice(&0xCEu16.to_le_bytes());
    bytes.extend_from_slice(&0u16.to_le_bytes());
    bytes.extend_from_slice(&u16::MAX.to_le_bytes());
    bytes.extend_from_slice(&0u16.to_le_bytes());
    bytes.extend_from_slice(&0u32.to_le_bytes());
    bytes.extend_from_slice(&u32::MAX.to_le_bytes());
    bytes.extend_from_slice(name_bytes);

    bytes.extend_from_slice(&0x0001u16.to_le_bytes());
    bytes.extend_from_slice(&0x20u16.to_le_bytes());
    bytes.extend_from_slice(&(data.len() as u64).to_le_bytes());
    bytes.extend_from_slice(&(data.len() as u64).to_le_bytes());
    bytes.extend_from_slice(&local_header_offset.to_le_bytes());
    bytes.extend_from_slice(&0u32.to_le_bytes());

    bytes.extend_from_slice(&0x5000u16.to_le_bytes());
    bytes.extend_from_slice(&0x84u16.to_le_bytes());
    bytes.extend_from_slice(&signature);

    bytes.extend_from_slice(&0x5002u16.to_le_bytes());
    bytes.extend_from_slice(&0x06u16.to_le_bytes());
    bytes.extend_from_slice(&encryption.to_le_bytes());

    bytes.extend_from_slice(&0x5003u16.to_le_bytes());
    bytes.extend_from_slice(&0x24u16.to_le_bytes());
    bytes.extend_from_slice(&sha256);

    let central_dir_size = bytes.len() as u64 - central_dir_offset;
    let zip64_eocd_offset = bytes.len() as u64;
    bytes.extend_from_slice(&0x0606_4b50u32.to_le_bytes());
    bytes.extend_from_slice(&44u64.to_le_bytes());
    bytes.extend_from_slice(&46u16.to_le_bytes());
    bytes.extend_from_slice(&45u16.to_le_bytes());
    bytes.extend_from_slice(&0u32.to_le_bytes());
    bytes.extend_from_slice(&0u32.to_le_bytes());
    bytes.extend_from_slice(&1u64.to_le_bytes());
    bytes.extend_from_slice(&1u64.to_le_bytes());
    bytes.extend_from_slice(&central_dir_size.to_le_bytes());
    bytes.extend_from_slice(&central_dir_offset.to_le_bytes());

    bytes.extend_from_slice(&0x0706_4b50u32.to_le_bytes());
    bytes.extend_from_slice(&0u32.to_le_bytes());
    bytes.extend_from_slice(&zip64_eocd_offset.to_le_bytes());
    bytes.extend_from_slice(&1u32.to_le_bytes());

    bytes.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    bytes.extend_from_slice(&u16::MAX.to_le_bytes());
    bytes.extend_from_slice(&u16::MAX.to_le_bytes());
    bytes.extend_from_slice(&u16::MAX.to_le_bytes());
    bytes.extend_from_slice(&u16::MAX.to_le_bytes());
    bytes.extend_from_slice(&u32::MAX.to_le_bytes());
    bytes.extend_from_slice(&u32::MAX.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"CI");
    bytes.extend_from_slice(&0x0001_0047u32.to_le_bytes());
    bytes.extend_from_slice(&(SECTOR_SIZE as u16).to_le_bytes());
    bytes.extend_from_slice(&(freelist_blocks.len() as u64).to_le_bytes());
    bytes.resize(
        (bytes.len() + SECTOR_SIZE as usize - 1) & !(SECTOR_SIZE as usize - 1),
        0,
    );

    bytes
}

fn sha256_array(data: &[u8]) -> [u8; 32] {
    let digest = Sha256::digest(data);
    let mut out = [0; 32];
    out.copy_from_slice(&digest);
    out
}

fn find_v1_ci_comment(bytes: &[u8]) -> usize {
    bytes
        .windows(6)
        .rposition(|window| window == b"CIG\0\x01\0")
        .expect("v1 EOCD CIG marker comment")
}

fn find_zip64_eocd(bytes: &[u8]) -> usize {
    bytes
        .windows(4)
        .position(|window| window == 0x0606_4b50u32.to_le_bytes())
        .expect("ZIP64 EOCD")
}

fn find_zip64_locator(bytes: &[u8]) -> usize {
    bytes
        .windows(4)
        .position(|window| window == 0x0706_4b50u32.to_le_bytes())
        .expect("ZIP64 locator")
}

fn local_v1_record_size(name: &str, sector_size: u64) -> u64 {
    let value = name.len() as u64 + sector_size + 0x3D;
    (value + sector_size - 1) & !(sector_size - 1)
}

fn move_v1_payload_to_zip_local_span(bytes: &mut [u8], name: &str, data: &[u8]) {
    let zip_payload_offset = SECTOR_SIZE as usize;
    let aligned_payload_offset = local_v1_record_size(name, SECTOR_SIZE) as usize;
    assert_ne!(zip_payload_offset, aligned_payload_offset);
    bytes[zip_payload_offset..zip_payload_offset + data.len()].copy_from_slice(data);
    bytes[aligned_payload_offset..aligned_payload_offset + data.len()].fill(0);
}

/// Opt-in test: set `SVAROG_P4K_TEST_FILE` to point at a real P4K
/// archive to verify the dispatcher still classifies it correctly.
/// Set `SVAROG_P4K_TEST_VERIFY=1` to also hash/decode every entry.
/// Skipped silently when the env var is unset.
#[test]
fn real_world_p4k_parses() {
    let Ok(path) = std::env::var("SVAROG_P4K_TEST_FILE") else {
        return;
    };
    if !Path::new(&path).exists() {
        eprintln!("skipping: SVAROG_P4K_TEST_FILE points at missing path");
        return;
    }
    open_real_archive(Path::new(&path));
}

/// Opt-in corpus test: set `SVAROG_P4K_TEST_DIR` to a Star Citizen
/// archive directory or corpus root. Every `.p4k` file below it is
/// opened; set `SVAROG_P4K_TEST_VERIFY=1` to also hash/decode all entries.
#[test]
fn real_world_p4k_corpus_parses() {
    let Ok(root) = std::env::var("SVAROG_P4K_TEST_DIR") else {
        return;
    };
    let root = Path::new(&root);
    if !root.exists() {
        eprintln!("skipping: SVAROG_P4K_TEST_DIR points at missing path");
        return;
    }

    let mut paths = Vec::new();
    collect_p4k_files(root, &mut paths).expect("walk p4k corpus");
    paths.sort();
    assert!(
        !paths.is_empty(),
        "SVAROG_P4K_TEST_DIR did not contain any .p4k files"
    );

    for path in paths {
        open_real_archive(&path);
    }
}

fn open_real_archive(path: &Path) {
    let archive = P4kArchive::open(path).expect("open p4k");
    eprintln!(
        "opened {:?}: version={:?} entries={}",
        path.file_name(),
        archive.version(),
        archive.entry_count()
    );
    assert!(archive.entry_count() > 0);

    if verify_real_archive_integrity() {
        verify_real_archive(&archive);
    } else if verify_real_archive_payload_sha256() {
        verify_real_archive_payload_sha256_only(&archive);
    }
}

#[cfg(feature = "parallel")]
fn verify_real_archive(archive: &P4kArchive) {
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

    let compressed_sizes: Vec<_> = archive.iter().map(|entry| entry.compressed_size).collect();
    let checked_entries = AtomicUsize::new(0);
    let checked_bytes = AtomicU64::new(0);
    let started = Instant::now();
    archive
        .verify_integrity_parallel_with_progress(|index, _| {
            let entries = checked_entries.fetch_add(1, Ordering::Relaxed) + 1;
            let bytes = checked_bytes.fetch_add(compressed_sizes[index], Ordering::Relaxed)
                + compressed_sizes[index];
            report_real_archive_progress("verify", entries, archive.entry_count(), bytes, started);
        })
        .expect("verify p4k integrity");
}

#[cfg(not(feature = "parallel"))]
fn verify_real_archive(archive: &P4kArchive) {
    let compressed_sizes: Vec<_> = archive.iter().map(|entry| entry.compressed_size).collect();
    let mut checked_entries = 0usize;
    let mut checked_bytes = 0u64;
    let started = Instant::now();
    archive
        .verify_integrity_with_progress(|index, _| {
            checked_entries += 1;
            checked_bytes += compressed_sizes[index];
            report_real_archive_progress(
                "verify",
                checked_entries,
                archive.entry_count(),
                checked_bytes,
                started,
            );
        })
        .expect("verify p4k integrity");
}

#[cfg(feature = "parallel")]
fn verify_real_archive_payload_sha256_only(archive: &P4kArchive) {
    verify_real_archive_payload_sha256_physical_order_parallel(archive);
}

#[cfg(not(feature = "parallel"))]
fn verify_real_archive_payload_sha256_only(archive: &P4kArchive) {
    verify_real_archive_payload_sha256_physical_order(archive);
}

#[cfg(not(feature = "parallel"))]
fn verify_real_archive_payload_sha256_physical_order(archive: &P4kArchive) {
    let compressed_sizes: Vec<_> = archive.iter().map(|entry| entry.compressed_size).collect();
    let mut checked_entries = 0usize;
    let mut checked_bytes = 0u64;
    let started = Instant::now();
    archive
        .verify_payloads_sha256_physical_order_with_progress(|index, _| {
            checked_entries += 1;
            checked_bytes += compressed_sizes[index];
            report_real_archive_progress(
                "sha256",
                checked_entries,
                archive.entry_count(),
                checked_bytes,
                started,
            );
        })
        .expect("verify p4k payload sha256");
}

#[cfg(feature = "parallel")]
fn verify_real_archive_payload_sha256_physical_order_parallel(archive: &P4kArchive) {
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

    let compressed_sizes: Vec<_> = archive.iter().map(|entry| entry.compressed_size).collect();
    let checked_entries = AtomicUsize::new(0);
    let checked_bytes = AtomicU64::new(0);
    let started = Instant::now();
    archive
        .verify_payloads_sha256_physical_order_parallel_with_progress(|index, _| {
            let entries = checked_entries.fetch_add(1, Ordering::Relaxed) + 1;
            let bytes = checked_bytes.fetch_add(compressed_sizes[index], Ordering::Relaxed)
                + compressed_sizes[index];
            report_real_archive_progress("sha256", entries, archive.entry_count(), bytes, started);
        })
        .expect("verify p4k payload sha256");
}

fn report_real_archive_progress(
    label: &str,
    entries: usize,
    total_entries: usize,
    bytes: u64,
    started: Instant,
) {
    if entries != total_entries && entries % 10_000 != 0 {
        return;
    }
    let elapsed = started.elapsed();
    if elapsed < Duration::from_secs(1) {
        return;
    }
    let mib = bytes as f64 / (1024.0 * 1024.0);
    let mib_per_sec = mib / elapsed.as_secs_f64();
    eprintln!("{label}: {entries}/{total_entries} entries, {mib:.1} MiB, {mib_per_sec:.1} MiB/s");
}

fn verify_real_archive_integrity() -> bool {
    matches!(
        std::env::var("SVAROG_P4K_TEST_VERIFY").as_deref(),
        Ok("1" | "true" | "TRUE" | "yes" | "YES")
    )
}

fn verify_real_archive_payload_sha256() -> bool {
    matches!(
        std::env::var("SVAROG_P4K_TEST_SHA256").as_deref(),
        Ok("1" | "true" | "TRUE" | "yes" | "YES")
    )
}

fn collect_p4k_files(path: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    if path.is_file() {
        if is_p4k_file(path) {
            out.push(path.to_path_buf());
        }
        return Ok(());
    }

    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            collect_p4k_files(&path, out)?;
        } else if file_type.is_file() && is_p4k_file(&path) {
            out.push(path);
        }
    }

    Ok(())
}

fn is_p4k_file(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("p4k"))
}
