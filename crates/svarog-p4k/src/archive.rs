//! P4K archive reader - Optimized for maximum performance.
//!
//! Key optimizations:
//! - SIMD-accelerated null padding detection
//! - Zero-copy entry storage with arena-allocated names
//! - Parallel central directory parsing
//! - Parallel extraction with worker pool
//! - Thread-local decompressors to avoid allocation

use std::borrow::Cow;
use std::collections::HashSet;
use std::fs::File;
use std::io::Write;
use std::path::Path;

use memmap2::{Advice, Mmap};
use sha2::{Digest, Sha256};
use svarog_common::BinaryReader;

use crate::crypto;
use crate::decompress;
use crate::simd;
use crate::zip::central_dir::extra_field;
#[cfg(any(feature = "parallel", test))]
use crate::zip::CDR_V2_OFFSET_TO_FILENAME_OFFSET;
use crate::zip::{
    CentralDirectoryHeader, CentralDirectoryHeaderV2, CompressionMethod, Eocd2Record,
    Eocd64Locator, Eocd64Record, EocdRecord, LocalFileHeader, P4kEncryptionExtraField,
    P4kSha256ExtraField, P4kSignatureExtraField, P4kZip64ExtraField, CDR_V2_ENTRY_SIZE,
    EOCD_V2_MAGIC, EOCD_V2_SIZE, EOCD_V2_VERSION,
};
use crate::{Error, Result};

type ParsedV2Archive = Option<(
    Vec<P4kEntryCompact>,
    Vec<P4kFreelistBlock>,
    P4kArchiveLayout,
)>;
type V2PayloadRange = (usize, usize, usize);
type ParsedV2Entries = (Vec<P4kEntryCompact>, Vec<V2PayloadRange>);

#[cfg(feature = "parallel")]
const V1_PARALLEL_CDR_ENTRY_THRESHOLD: usize = 4096;
#[cfg(feature = "parallel")]
const V2_PARALLEL_CDR_ENTRY_THRESHOLD: usize = 4096;

#[cfg(feature = "parallel")]
#[derive(Debug, Clone, Copy)]
struct CdrRecordSpan {
    start: usize,
    end: usize,
}

#[derive(Debug)]
struct ParsedV2Entry {
    entry: P4kEntryCompact,
    payload_range: Option<(usize, usize, usize)>,
}

/// On-disk P4K format version detected at open time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum P4kVersion {
    /// Legacy ZIP64-based format with per-entry local file headers.
    V1,
    /// "JiJi" trailer format. Entries point directly at compressed
    /// payload bytes with no local file header.
    V2,
}

/// A P4K entry with zero-copy name storage.
///
/// The name is stored as a reference into an arena allocator,
/// avoiding per-entry heap allocations.
#[derive(Debug, Clone, Copy)]
pub struct P4kEntryRef<'a> {
    /// File name/path within the archive (arena-allocated)
    pub name: &'a str,
    /// Compressed size in bytes
    pub compressed_size: u64,
    /// Uncompressed size in bytes
    pub uncompressed_size: u64,
    /// Compression method
    pub compression_method: CompressionMethod,
    /// Whether the entry is encrypted
    pub is_encrypted: bool,
    /// Entry offset from the central directory.
    ///
    /// For v1 archives this is the ZIP local-file-header offset. For v2
    /// archives this is `offset_to_file_data` and points directly at the
    /// compressed payload bytes because v2 has no per-entry local header.
    pub local_header_offset: u64,
    /// CIG CRC32C checksum
    pub crc32: u32,
    /// DOS last-modified time.
    pub last_mod_file_time: u16,
    /// DOS last-modified date.
    pub last_mod_file_date: u16,
    /// RSA-1024 signature stored in P4K custom metadata.
    pub signature: [u8; 128],
    /// SHA-256 of the raw stored payload bytes.
    pub sha256: [u8; 32],
    /// Bytes already written to disk from the install block.
    pub bytes_already_written: u64,
}

/// Archive-level layout metadata retained from the parsed CDR/EOCDR.
#[derive(Debug, Clone)]
pub struct P4kArchiveLayout {
    /// Total mapped file size, including trailing zero padding.
    pub file_size: u64,
    /// End of non-zero archive content after trimming trailing null padding.
    pub actual_content_end: u64,
    /// Physical sector size recorded by the P4K metadata, when present.
    pub physical_sector_size: Option<u64>,
    /// Absolute offset of the central directory block.
    pub cdr_offset: u64,
    /// Central directory byte size.
    pub cdr_size: u64,
    /// Absolute v2 name-table offset.
    pub name_table_offset: Option<u64>,
    /// v2 name-table byte size.
    pub name_table_size: u64,
    /// v2 payload end / install block start.
    pub end_of_payload: Option<u64>,
    /// Absolute install block offset.
    pub install_block_offset: Option<u64>,
    /// Install block span up to the CDR, including padding.
    pub install_block_size: Option<u64>,
    /// Offset of the v1 EOCD or v2 EOCDR.
    pub eocd_offset: u64,
    /// v2 EOCDR manifest digest bytes.
    pub manifest_sha256: Option<[u8; 64]>,
}

/// Optimized P4K archive reader.
///
/// Uses SIMD for parsing and optimized data structures.
pub struct P4kArchive {
    /// Memory-mapped file data
    mmap: Mmap,
    /// Archive file name
    name: String,
    /// Detected on-disk format version.
    version: P4kVersion,
    /// Entry metadata
    entries: Vec<P4kEntryCompact>,
    /// Free ranges persisted in the archive install block.
    freelist_blocks: Vec<P4kFreelistBlock>,
    /// Parsed archive-level layout metadata.
    layout: P4kArchiveLayout,
    /// Detected v1 payload placement convention, if this is a v1 archive.
    v1_payload_placement: V1PayloadPlacement,
}

/// Compact entry metadata (names stored separately)
#[derive(Debug, Clone)]
struct P4kEntryCompact {
    /// File name (owned string)
    name: String,
    /// Compressed size
    compressed_size: u64,
    /// Uncompressed size
    uncompressed_size: u64,
    /// Compression method (stored as u8)
    compression_method: u8,
    /// Flags: bit 0 = encrypted, bit 1 = v2 (no local header)
    flags: u8,
    /// For v1: offset of local file header.
    /// For v2: absolute offset of compressed payload bytes.
    local_header_offset: u64,
    /// CRC32
    crc32: u32,
    /// DOS last-modified time.
    last_mod_file_time: u16,
    /// DOS last-modified date.
    last_mod_file_date: u16,
    /// RSA-1024 signature stored in P4K custom metadata.
    signature: [u8; 128],
    /// SHA-256 stored in P4K custom metadata.
    sha256: [u8; 32],
    /// Bytes already written to disk from the install block.
    bytes_already_written: u64,
}

/// Raw compressed payload plus metadata for internal writer/converter paths.
pub(crate) struct P4kRawEntryRef<'a> {
    pub(crate) name: &'a str,
    pub(crate) payload_len: u64,
    pub(crate) local_header_offset: u64,
    pub(crate) payload_offset: u64,
    pub(crate) local_record_size: u64,
    pub(crate) uncompressed_size: u64,
    pub(crate) compression_method: CompressionMethod,
    pub(crate) is_encrypted: bool,
    pub(crate) crc32: u32,
    pub(crate) last_mod_file_time: u16,
    pub(crate) last_mod_file_date: u16,
    pub(crate) signature: [u8; 128],
    pub(crate) sha256: [u8; 32],
    pub(crate) bytes_already_written: u64,
}

/// Free byte range recorded in a P4K install block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct P4kFreelistBlock {
    /// Absolute byte offset of the free range.
    pub offset: u64,
    /// Byte length of the free range.
    pub size: u64,
}

/// Bit mask for the encrypted flag on [`P4kEntryCompact::flags`].
const FLAG_ENCRYPTED: u8 = 0b01;
/// Bit mask flagging an entry as coming from a v2 archive (payload
/// has no local file header preceding the compressed data).
const FLAG_V2_NO_LOCAL_HEADER: u8 = 0b10;
const V2_CDR_ALIGNMENT: usize = 0x1_0000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum V1PayloadPlacement {
    Unknown,
    /// Payload bytes follow the ZIP local header and the sampled
    /// archive proves that local header span is one physical sector.
    ZipLocalSector,
    /// Payload bytes follow the ZIP local header, but the local header
    /// span must be read per entry.
    ZipLocal,
    /// Payload bytes follow the dump's v1 local-record-size formula.
    AlignedRecord,
}

impl V1PayloadPlacement {
    fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::ZipLocalSector => "zip_local_sector",
            Self::ZipLocal => "zip_local",
            Self::AlignedRecord => "aligned_record",
        }
    }
}

impl P4kArchive {
    /// Open a P4K archive with maximum performance optimizations.
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path = path.as_ref();
        let file = File::open(path)?;
        let mmap = unsafe { Mmap::map(&file)? };
        let _ = mmap.advise(Advice::Sequential);

        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();

        let (version, entries, freelist_blocks, layout) = Self::parse_entries_dispatched(&mmap)?;
        let v1_payload_placement =
            detect_v1_payload_placement(&mmap, version, &entries, layout.physical_sector_size);

        Ok(Self {
            mmap,
            name,
            version,
            entries,
            freelist_blocks,
            layout,
            v1_payload_placement,
        })
    }

    /// Get the archive name.
    #[inline]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Detected P4K format version.
    #[inline]
    pub fn version(&self) -> P4kVersion {
        self.version
    }

    /// Get the number of entries.
    #[inline]
    pub fn entry_count(&self) -> usize {
        self.entries.len()
    }

    /// Parsed archive-level layout metadata.
    #[inline]
    pub fn layout(&self) -> &P4kArchiveLayout {
        &self.layout
    }

    /// Persisted freelist records from the install block.
    #[inline]
    pub fn freelist_blocks(&self) -> &[P4kFreelistBlock] {
        &self.freelist_blocks
    }

    /// Detected v1 payload placement convention, exposed for metadata dumps.
    ///
    /// Returns `None` for v2 archives. For v1 archives the value is:
    /// `unknown`, `zip_local_sector`, `zip_local`, or `aligned_record`.
    #[inline]
    pub fn v1_payload_placement_kind(&self) -> Option<&'static str> {
        match self.version {
            P4kVersion::V2 => None,
            P4kVersion::V1 => Some(self.v1_payload_placement.as_str()),
        }
    }

    /// Known compressed-payload offset for an entry, when archive-wide
    /// metadata is sufficient to compute it without per-entry SHA probing.
    #[inline]
    pub fn known_payload_offset(&self, entry: &P4kEntryRef<'_>) -> Option<u64> {
        if entry.compressed_size == 0 {
            return Some(0);
        }

        let offset = match self.version {
            P4kVersion::V2 => Some(entry.local_header_offset),
            P4kVersion::V1 => {
                self.v1_known_payload_offset_fields(entry.name, entry.local_header_offset)
            }
        }?;

        self.payload_range_fits(offset, entry.compressed_size)
            .then_some(offset)
    }

    /// Resolve the exact compressed-payload offset for an entry.
    ///
    /// For v2 this validates the direct `offset_to_file_data` range. For v1
    /// this follows the same local-header validation and placement selection
    /// path used by reads, including the SHA-256 fallback when archive-wide
    /// placement detection is inconclusive.
    pub fn payload_offset(&self, entry: &P4kEntryRef<'_>) -> Result<u64> {
        if entry.compressed_size == 0 {
            return Ok(0);
        }

        match self.version {
            P4kVersion::V2 => {
                self.payload_slice_by_data_offset(
                    entry.local_header_offset,
                    entry.compressed_size,
                )?;
                Ok(entry.local_header_offset)
            }
            P4kVersion::V1 => {
                let (offset, _, _) = self.payload_location_by_local_header(
                    entry.name,
                    entry.local_header_offset,
                    entry.compressed_size,
                    entry.uncompressed_size,
                    entry.compression_method,
                    entry.crc32,
                    entry.last_mod_file_time,
                    entry.last_mod_file_date,
                    entry.sha256,
                )?;
                Ok(offset)
            }
        }
    }

    /// Iterate over entries with zero-copy access.
    #[inline]
    pub fn iter(&self) -> impl Iterator<Item = P4kEntryRef<'_>> + '_ {
        self.entries.iter().map(|e| self.entry_ref(e))
    }

    /// Get entry by index.
    #[inline]
    pub fn get(&self, index: usize) -> Option<P4kEntryRef<'_>> {
        self.entries.get(index).map(|e| self.entry_ref(e))
    }

    /// Find an entry by name (case-insensitive).
    pub fn find(&self, name: &str) -> Option<P4kEntryRef<'_>> {
        let normalized = normalize_p4k_path(name, false);
        self.entries
            .iter()
            .find(|e| {
                let entry_name = self.get_name(e);
                entry_name.eq_ignore_ascii_case(&normalized)
            })
            .map(|e| self.entry_ref(e))
    }

    /// Read entry contents - handles decryption and decompression.
    pub fn read(&self, entry: &P4kEntryRef<'_>) -> Result<Vec<u8>> {
        match self.version {
            P4kVersion::V1 => self.read_by_offset(
                entry.name,
                entry.local_header_offset,
                entry.compressed_size,
                entry.uncompressed_size,
                entry.compression_method,
                entry.is_encrypted,
                entry.crc32,
                entry.last_mod_file_time,
                entry.last_mod_file_date,
                entry.sha256,
            ),
            P4kVersion::V2 => self.read_by_data_offset(
                entry.local_header_offset,
                entry.compressed_size,
                entry.uncompressed_size,
                entry.compression_method,
                entry.is_encrypted,
                entry.crc32,
            ),
        }
    }

    /// Extract entry contents to a writer.
    ///
    /// Stored, compressed, and encrypted payloads stream directly from
    /// the memory map through the required AES/decompression adapters
    /// without first allocating a full decoded `Vec`.
    pub fn extract_to_writer<W: Write>(
        &self,
        entry: &P4kEntryRef<'_>,
        writer: &mut W,
    ) -> Result<()> {
        if let Some(payload) = self.unencrypted_stored_payload_slice(entry)? {
            writer.write_all(payload)?;
            return Ok(());
        }

        let payload = match self.version {
            P4kVersion::V1 => {
                self.payload_slice_by_local_header(
                    entry.name,
                    entry.local_header_offset,
                    entry.compressed_size,
                    entry.uncompressed_size,
                    entry.compression_method,
                    entry.crc32,
                    entry.last_mod_file_time,
                    entry.last_mod_file_date,
                    entry.sha256,
                )?
                .2
            }
            P4kVersion::V2 => {
                self.payload_slice_by_data_offset(entry.local_header_offset, entry.compressed_size)?
            }
        };
        let expected_size = usize::try_from(entry.uncompressed_size).map_err(|_| {
            Error::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "uncompressed size overflows usize",
            ))
        })?;
        if entry.is_encrypted {
            if payload.len() % 16 != 0 {
                return Err(Error::Decryption(
                    "data length must be a multiple of 16 bytes".to_string(),
                ));
            }
            let reader = crypto::DecryptReader::new(payload);
            decompress::decode_reader_to_writer(
                reader,
                entry.compression_method,
                expected_size,
                writer,
            )?;
        } else {
            decompress::decode_to_writer(payload, entry.compression_method, expected_size, writer)?;
        }
        Ok(())
    }

    /// Write the raw stored payload bytes for an entry.
    ///
    /// The output is the exact compressed, post-encryption byte stream covered
    /// by the entry's SHA-256 metadata. No decompression or decryption is
    /// performed.
    pub fn extract_raw_payload_to_writer<W: Write>(
        &self,
        entry: &P4kEntryRef<'_>,
        writer: &mut W,
    ) -> Result<()> {
        let payload = self.raw_payload_slice(entry)?;
        writer.write_all(payload)?;
        Ok(())
    }

    /// Verify the raw stored payload SHA-256 for an entry.
    ///
    /// This matches the explicit verification path in the CigDataPatcher
    /// dump: hash the compressed, post-encryption payload bytes and compare
    /// them with the P4K metadata before decompression.
    pub fn verify_payload_sha256(&self, entry: &P4kEntryRef<'_>) -> Result<()> {
        let (payload, already_verified) = match self.version {
            P4kVersion::V1 => {
                let (_, _, payload, already_verified) = self.payload_slice_by_local_header(
                    entry.name,
                    entry.local_header_offset,
                    entry.compressed_size,
                    entry.uncompressed_size,
                    entry.compression_method,
                    entry.crc32,
                    entry.last_mod_file_time,
                    entry.last_mod_file_date,
                    entry.sha256,
                )?;
                (payload, already_verified)
            }
            P4kVersion::V2 => {
                let payload = self.payload_slice_by_data_offset(
                    entry.local_header_offset,
                    entry.compressed_size,
                )?;
                (payload, false)
            }
        };
        if already_verified {
            return Ok(());
        }
        validate_sha256(payload, entry.sha256)
    }

    /// Verify raw payload SHA-256 and decoded CIG CRC32C for an entry.
    pub fn verify_entry_integrity(&self, entry: &P4kEntryRef<'_>) -> Result<()> {
        self.verify_payload_sha256(entry)?;
        let actual = self.decoded_crc32(entry)?;
        validate_crc32_value(actual, entry.crc32)?;
        Ok(())
    }

    fn decoded_crc32(&self, entry: &P4kEntryRef<'_>) -> Result<u32> {
        if let Some(payload) = self.unencrypted_stored_payload_slice(entry)? {
            return Ok(svarog_common::crc::hash_bytes(payload));
        }

        let payload = match self.version {
            P4kVersion::V1 => {
                self.payload_slice_by_local_header(
                    entry.name,
                    entry.local_header_offset,
                    entry.compressed_size,
                    entry.uncompressed_size,
                    entry.compression_method,
                    entry.crc32,
                    entry.last_mod_file_time,
                    entry.last_mod_file_date,
                    entry.sha256,
                )?
                .2
            }
            P4kVersion::V2 => {
                self.payload_slice_by_data_offset(entry.local_header_offset, entry.compressed_size)?
            }
        };
        let expected_size = usize::try_from(entry.uncompressed_size).map_err(|_| {
            Error::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "uncompressed size overflows usize",
            ))
        })?;

        let mut sink = Crc32Sink::new();
        if entry.is_encrypted {
            if payload.len() % 16 != 0 {
                return Err(Error::Decryption(
                    "data length must be a multiple of 16 bytes".to_string(),
                ));
            }
            let reader = crypto::DecryptReader::new(payload);
            decompress::decode_reader_to_writer(
                reader,
                entry.compression_method,
                expected_size,
                &mut sink,
            )?;
        } else {
            decompress::decode_to_writer(
                payload,
                entry.compression_method,
                expected_size,
                &mut sink,
            )?;
        }

        Ok(sink.finish())
    }

    /// Verify every archive entry's raw payload SHA-256 and decoded CIG CRC32C.
    pub fn verify_integrity(&self) -> Result<()> {
        self.verify_integrity_with_progress(|_, _| {})
    }

    /// Verify every archive entry's raw stored payload SHA-256.
    pub fn verify_payloads_sha256(&self) -> Result<()> {
        self.verify_payloads_sha256_with_progress(|_, _| {})
    }

    /// Verify every raw stored payload SHA-256 in physical archive order.
    ///
    /// V1 archives can expose entries in filename-handle order, which is
    /// often different from on-disk payload order. Sorting before hashing
    /// keeps large retail archive verification mostly sequential.
    pub fn verify_payloads_sha256_physical_order(&self) -> Result<()> {
        self.verify_payloads_sha256_physical_order_with_progress(|_, _| {})
    }

    /// Verify every raw stored payload SHA-256 in physical archive order, calling
    /// `progress` after each successful entry.
    pub fn verify_payloads_sha256_physical_order_with_progress<F>(
        &self,
        mut progress: F,
    ) -> Result<()>
    where
        F: FnMut(usize, &str),
    {
        let mut entries: Vec<_> = self.entries.iter().enumerate().collect();
        entries.sort_unstable_by_key(|(_, entry)| self.payload_sort_offset(entry));
        for (index, entry) in entries {
            self.verify_payload_sha256_compact_fast(entry)?;
            progress(index, &entry.name);
        }
        Ok(())
    }

    /// Verify every archive entry's raw stored payload SHA-256, calling `progress`
    /// after each successful entry.
    pub fn verify_payloads_sha256_with_progress<F>(&self, mut progress: F) -> Result<()>
    where
        F: FnMut(usize, &str),
    {
        for (index, entry) in self.iter().enumerate() {
            self.verify_payload_sha256(&entry)?;
            progress(index, entry.name);
        }
        Ok(())
    }

    /// Verify every archive entry's raw stored payload SHA-256 in parallel.
    #[cfg(feature = "parallel")]
    pub fn verify_payloads_sha256_parallel_with_progress<F>(&self, progress: F) -> Result<()>
    where
        F: Fn(usize, &str) + Sync + Send,
    {
        use rayon::prelude::*;

        self.entries
            .par_iter()
            .enumerate()
            .try_for_each(|(index, entry)| {
                self.verify_payload_sha256_compact_fast(entry)?;
                progress(index, &entry.name);
                Ok(())
            })
    }

    /// Verify every raw stored payload SHA-256 in physical archive order in parallel.
    #[cfg(feature = "parallel")]
    pub fn verify_payloads_sha256_physical_order_parallel_with_progress<F>(
        &self,
        progress: F,
    ) -> Result<()>
    where
        F: Fn(usize, &str) + Sync + Send,
    {
        use rayon::prelude::*;

        let mut entries: Vec<_> = self.entries.iter().enumerate().collect();
        entries.sort_unstable_by_key(|(_, entry)| self.payload_sort_offset(entry));
        entries.par_iter().try_for_each(|(index, entry)| {
            self.verify_payload_sha256_compact_fast(entry)?;
            progress(*index, &entry.name);
            Ok(())
        })
    }

    /// Verify every archive entry, calling `progress` after each successful entry.
    pub fn verify_integrity_with_progress<F>(&self, mut progress: F) -> Result<()>
    where
        F: FnMut(usize, &str),
    {
        let mut entries: Vec<_> = self.entries.iter().enumerate().collect();
        entries.sort_unstable_by_key(|(_, entry)| self.payload_sort_offset(entry));
        for (index, entry) in entries {
            self.verify_entry_integrity_compact_fast(entry)?;
            progress(index, &entry.name);
        }
        Ok(())
    }

    /// Verify every archive entry in parallel, calling `progress` after each successful entry.
    #[cfg(feature = "parallel")]
    pub fn verify_integrity_parallel_with_progress<F>(&self, progress: F) -> Result<()>
    where
        F: Fn(usize, &str) + Sync + Send,
    {
        use rayon::prelude::*;

        let mut entries: Vec<_> = self.entries.iter().enumerate().collect();
        entries.sort_unstable_by_key(|(_, entry)| self.payload_sort_offset(entry));
        entries.par_iter().try_for_each(|(index, entry)| {
            self.verify_entry_integrity_compact_fast(entry)?;
            progress(*index, &entry.name);
            Ok(())
        })
    }

    /// Read entry by index.
    pub fn read_index(&self, index: usize) -> Result<Vec<u8>> {
        let entry = self.entries.get(index).ok_or_else(|| {
            Error::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "entry index out of bounds",
            ))
        })?;

        let v2 = entry.flags & FLAG_V2_NO_LOCAL_HEADER != 0;
        let method = CompressionMethod::try_from(entry.compression_method as u16)
            .map_err(Error::UnsupportedCompression)?;
        let encrypted = entry.flags & FLAG_ENCRYPTED != 0;

        if v2 {
            self.read_by_data_offset(
                entry.local_header_offset,
                entry.compressed_size,
                entry.uncompressed_size,
                method,
                encrypted,
                entry.crc32,
            )
        } else {
            self.read_by_offset(
                &entry.name,
                entry.local_header_offset,
                entry.compressed_size,
                entry.uncompressed_size,
                method,
                encrypted,
                entry.crc32,
                entry.last_mod_file_time,
                entry.last_mod_file_date,
                entry.sha256,
            )
        }
    }

    /// Extract entry contents by index to a writer.
    pub fn extract_index_to_writer<W: Write>(&self, index: usize, writer: &mut W) -> Result<()> {
        let entry = self.get(index).ok_or_else(|| {
            Error::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "entry index out of bounds",
            ))
        })?;
        self.extract_to_writer(&entry, writer)
    }

    /// Dump every archive entry into `output_dir`.
    pub fn dump_to_dir<P: AsRef<Path>>(&self, output_dir: P) -> Result<()> {
        crate::writer::dump_archive_to_dir(self, output_dir)
    }

    /// Dump every archive entry's raw stored payload into `output_dir`.
    pub fn dump_raw_payloads_to_dir<P: AsRef<Path>>(&self, output_dir: P) -> Result<()> {
        crate::writer::dump_archive_raw_payloads_to_dir(self, output_dir)
    }

    /// Parallel extraction of multiple entries.
    #[cfg(feature = "parallel")]
    pub fn read_parallel<'a>(&'a self, entries: &[P4kEntryRef<'a>]) -> Vec<Result<Vec<u8>>> {
        use rayon::prelude::*;

        entries.par_iter().map(|entry| self.read(entry)).collect()
    }

    /// Parallel extraction with callback for streaming.
    #[cfg(feature = "parallel")]
    pub fn extract_parallel<F>(&self, indices: &[usize], mut callback: F) -> Result<()>
    where
        F: FnMut(usize, &str, Result<Vec<u8>>) + Send,
    {
        use rayon::prelude::*;
        use std::sync::Mutex;

        let callback = Mutex::new(&mut callback);

        indices.par_iter().try_for_each(|&idx| {
            let entry = self.entries.get(idx).ok_or_else(|| {
                Error::Io(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "entry index out of bounds",
                ))
            })?;

            let name = self.get_name(entry);
            let v2 = entry.flags & FLAG_V2_NO_LOCAL_HEADER != 0;
            let method = CompressionMethod::try_from(entry.compression_method as u16)
                .map_err(Error::UnsupportedCompression)?;
            let encrypted = entry.flags & FLAG_ENCRYPTED != 0;
            let result = if v2 {
                self.read_by_data_offset(
                    entry.local_header_offset,
                    entry.compressed_size,
                    entry.uncompressed_size,
                    method,
                    encrypted,
                    entry.crc32,
                )
            } else {
                self.read_by_offset(
                    name,
                    entry.local_header_offset,
                    entry.compressed_size,
                    entry.uncompressed_size,
                    method,
                    encrypted,
                    entry.crc32,
                    entry.last_mod_file_time,
                    entry.last_mod_file_date,
                    entry.sha256,
                )
            };

            callback.lock().unwrap()(idx, name, result);
            Ok(())
        })
    }

    // Internal methods

    #[inline]
    fn entry_ref<'a>(&'a self, entry: &'a P4kEntryCompact) -> P4kEntryRef<'a> {
        P4kEntryRef {
            name: &entry.name,
            compressed_size: entry.compressed_size,
            uncompressed_size: entry.uncompressed_size,
            compression_method: CompressionMethod::try_from(entry.compression_method as u16)
                .unwrap_or(CompressionMethod::Store),
            is_encrypted: entry.flags & FLAG_ENCRYPTED != 0,
            local_header_offset: entry.local_header_offset,
            crc32: entry.crc32,
            last_mod_file_time: entry.last_mod_file_time,
            last_mod_file_date: entry.last_mod_file_date,
            signature: entry.signature,
            sha256: entry.sha256,
            bytes_already_written: entry.bytes_already_written,
        }
    }

    #[inline]
    fn get_name<'a>(&'a self, entry: &'a P4kEntryCompact) -> &'a str {
        &entry.name
    }

    #[inline]
    fn payload_sort_offset(&self, entry: &P4kEntryCompact) -> u64 {
        match self.version {
            P4kVersion::V1 => self
                .v1_known_payload_offset(entry)
                .unwrap_or(entry.local_header_offset),
            P4kVersion::V2 => entry.local_header_offset,
        }
    }

    fn verify_payload_sha256_compact_fast(&self, entry: &P4kEntryCompact) -> Result<()> {
        if entry.compressed_size == 0 {
            return validate_sha256(&[], entry.sha256);
        }

        match self.version {
            P4kVersion::V1 => {
                if let Some(payload) = self.v1_known_payload_slice(entry)? {
                    let actual: [u8; 32] = Sha256::digest(payload).into();
                    if actual == entry.sha256 {
                        return Ok(());
                    }
                }

                let method = CompressionMethod::try_from(entry.compression_method as u16)
                    .map_err(Error::UnsupportedCompression)?;
                let (_, _, payload, already_verified) = self.payload_slice_by_local_header(
                    &entry.name,
                    entry.local_header_offset,
                    entry.compressed_size,
                    entry.uncompressed_size,
                    method,
                    entry.crc32,
                    entry.last_mod_file_time,
                    entry.last_mod_file_date,
                    entry.sha256,
                )?;
                if already_verified {
                    Ok(())
                } else {
                    validate_sha256(payload, entry.sha256)
                }
            }
            P4kVersion::V2 => {
                let payload = self.payload_slice_by_data_offset(
                    entry.local_header_offset,
                    entry.compressed_size,
                )?;
                validate_sha256(payload, entry.sha256)
            }
        }
    }

    fn verify_entry_integrity_compact_fast(&self, entry: &P4kEntryCompact) -> Result<()> {
        self.verify_payload_sha256_compact_fast(entry)?;
        let entry_ref = self.entry_ref(entry);
        let actual = self.decoded_crc32(&entry_ref)?;
        validate_crc32_value(actual, entry.crc32)
    }

    fn v1_known_payload_slice(&self, entry: &P4kEntryCompact) -> Result<Option<&[u8]>> {
        let Some(payload_offset) = self.v1_known_payload_offset(entry) else {
            return Ok(None);
        };
        self.v1_payload_slice_at(payload_offset, entry.compressed_size)
    }

    fn v1_known_payload_offset(&self, entry: &P4kEntryCompact) -> Option<u64> {
        self.v1_known_payload_offset_fields(&entry.name, entry.local_header_offset)
    }

    fn v1_known_payload_offset_fields(&self, name: &str, local_header_offset: u64) -> Option<u64> {
        match self.v1_payload_placement {
            V1PayloadPlacement::Unknown => None,
            V1PayloadPlacement::ZipLocalSector => {
                local_header_offset.checked_add(self.layout.physical_sector_size?)
            }
            V1PayloadPlacement::ZipLocal => {
                self.v1_zip_payload_offset_from_local_header(local_header_offset)
            }
            V1PayloadPlacement::AlignedRecord => {
                self.v1_aligned_payload_offset_from_name(name, local_header_offset)
            }
        }
    }

    fn v1_zip_payload_offset_from_local_header(&self, local_header_offset: u64) -> Option<u64> {
        let local_header_offset = usize::try_from(local_header_offset).ok()?;
        let header_size = std::mem::size_of::<LocalFileHeader>();
        let signature_end = local_header_offset.checked_add(4)?;
        let header_end = signature_end.checked_add(header_size)?;
        if header_end > self.mmap.len() {
            return None;
        }

        let sig = u32::from_le_bytes(
            self.mmap[local_header_offset..signature_end]
                .try_into()
                .ok()?,
        );
        if sig != LocalFileHeader::SIGNATURE && sig != LocalFileHeader::SIGNATURE_EXTENDED {
            return None;
        }
        let local_header = <LocalFileHeader as zerocopy::FromBytes>::read_from_bytes(
            &self.mmap[signature_end..header_end],
        )
        .ok()?;
        zip_local_payload_offset(local_header_offset, header_size, &local_header)
            .ok()
            .and_then(|offset| u64::try_from(offset).ok())
    }

    fn v1_aligned_payload_offset_from_name(
        &self,
        name: &str,
        local_header_offset: u64,
    ) -> Option<u64> {
        let sector_size = self.layout.physical_sector_size?;
        let record_size = v1_local_record_size_from_name_len(name.len() as u64, sector_size)?;
        local_header_offset.checked_add(record_size)
    }

    fn v1_payload_slice_at(
        &self,
        payload_offset: u64,
        compressed_size: u64,
    ) -> Result<Option<&[u8]>> {
        let start = usize::try_from(payload_offset).map_err(|_| {
            Error::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "v1 payload offset overflows usize",
            ))
        })?;
        let len = usize::try_from(compressed_size).map_err(|_| {
            Error::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "compressed size overflows usize",
            ))
        })?;
        let Some(end) = start.checked_add(len) else {
            return Ok(None);
        };
        if end > self.mmap.len() {
            return Ok(None);
        }
        Ok(Some(&self.mmap[start..end]))
    }

    #[allow(clippy::too_many_arguments)]
    fn read_by_offset(
        &self,
        expected_name: &str,
        local_header_offset: u64,
        compressed_size: u64,
        uncompressed_size: u64,
        compression_method: CompressionMethod,
        is_encrypted: bool,
        crc32: u32,
        last_mod_file_time: u16,
        last_mod_file_date: u16,
        expected_sha256: [u8; 32],
    ) -> Result<Vec<u8>> {
        let offset = usize::try_from(local_header_offset).map_err(|_| {
            Error::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "local header offset overflows usize",
            ))
        })?;

        // Validate and read local header
        let Some(signature_end) = offset.checked_add(4) else {
            return Err(Error::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "local header signature range overflow",
            )));
        };
        if signature_end > self.mmap.len() {
            return Err(Error::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "local header offset out of bounds",
            )));
        }

        // Read signature using direct byte access (faster than BinaryReader for small reads)
        let sig = u32::from_le_bytes([
            self.mmap[offset],
            self.mmap[offset + 1],
            self.mmap[offset + 2],
            self.mmap[offset + 3],
        ]);

        if sig != LocalFileHeader::SIGNATURE && sig != LocalFileHeader::SIGNATURE_EXTENDED {
            return Err(Error::InvalidSignature {
                expected: LocalFileHeader::SIGNATURE,
                actual: sig,
            });
        }

        // Read header struct
        let header_start = signature_end;
        let header_size = std::mem::size_of::<LocalFileHeader>();

        let Some(header_end) = header_start.checked_add(header_size) else {
            return Err(Error::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "local header range overflow",
            )));
        };
        if header_end > self.mmap.len() {
            return Err(Error::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "local header out of bounds",
            )));
        }

        let mut reader = BinaryReader::new(&self.mmap[header_start..]);
        let local_header: LocalFileHeader = reader.read_struct()?;
        validate_v1_local_header_metadata(
            &local_header,
            expected_name,
            compression_method,
            crc32,
            last_mod_file_time,
            last_mod_file_date,
        )?;
        validate_v1_local_extra_fields(
            &self.mmap,
            offset,
            header_size,
            &local_header,
            expected_name,
            self.v1_local_header_span()?,
            local_header_offset,
            compressed_size,
            uncompressed_size,
        )?;

        let (data_offset, _) = select_v1_payload_offset(
            &self.mmap,
            offset,
            header_size,
            &local_header,
            compressed_size,
            expected_sha256,
        )?;
        let payload = self.slice_range(
            data_offset as u64,
            compressed_size,
            "entry data out of bounds",
        )?;

        decode_payload_to_vec(payload, compression_method, uncompressed_size, is_encrypted)
    }

    /// Read a v2 entry whose `data_offset` points directly at the
    /// compressed (and optionally AES-encrypted) payload bytes — no
    /// local file header precedes the data.
    fn read_by_data_offset(
        &self,
        data_offset: u64,
        compressed_size: u64,
        uncompressed_size: u64,
        compression_method: CompressionMethod,
        is_encrypted: bool,
        _crc32: u32,
    ) -> Result<Vec<u8>> {
        if uncompressed_size == 0 && compressed_size == 0 {
            return Ok(Vec::new());
        }

        let payload = self.payload_slice_by_data_offset(data_offset, compressed_size)?;

        decode_payload_to_vec(payload, compression_method, uncompressed_size, is_encrypted)
    }

    /// Return compressed payload slices with metadata. Used by the
    /// v1->v2 converter to avoid decompressing/recompressing entries.
    pub(crate) fn raw_entries(&self) -> Result<Vec<P4kRawEntryRef<'_>>> {
        let mut raw_entries = Vec::with_capacity(self.entries.len());
        for entry in &self.entries {
            let (payload_offset, local_record_size, payload_len) =
                if entry.flags & FLAG_V2_NO_LOCAL_HEADER != 0 {
                    self.slice_range(
                        entry.local_header_offset,
                        entry.compressed_size,
                        "v2 entry data out of bounds",
                    )?;
                    (entry.local_header_offset, 0, entry.compressed_size)
                } else {
                    let (payload_offset, local_record_size, _) = self
                        .payload_location_by_local_header(
                            &entry.name,
                            entry.local_header_offset,
                            entry.compressed_size,
                            entry.uncompressed_size,
                            CompressionMethod::try_from(entry.compression_method as u16)
                                .map_err(Error::UnsupportedCompression)?,
                            entry.crc32,
                            entry.last_mod_file_time,
                            entry.last_mod_file_date,
                            entry.sha256,
                        )?;
                    self.slice_range(
                        payload_offset,
                        entry.compressed_size,
                        "entry data out of bounds",
                    )?;
                    (payload_offset, local_record_size, entry.compressed_size)
                };
            let compression_method = CompressionMethod::try_from(entry.compression_method as u16)
                .map_err(Error::UnsupportedCompression)?;
            raw_entries.push(P4kRawEntryRef {
                name: &entry.name,
                payload_len,
                local_header_offset: entry.local_header_offset,
                payload_offset,
                local_record_size,
                uncompressed_size: entry.uncompressed_size,
                compression_method,
                is_encrypted: entry.flags & FLAG_ENCRYPTED != 0,
                crc32: entry.crc32,
                last_mod_file_time: entry.last_mod_file_time,
                last_mod_file_date: entry.last_mod_file_date,
                signature: entry.signature,
                sha256: entry.sha256,
                bytes_already_written: entry.bytes_already_written,
            });
        }
        Ok(raw_entries)
    }

    pub(crate) fn unencrypted_stored_payload_slice<'a>(
        &'a self,
        entry: &P4kEntryRef<'_>,
    ) -> Result<Option<&'a [u8]>> {
        if entry.compression_method != CompressionMethod::Store || entry.is_encrypted {
            return Ok(None);
        }

        let payload = match self.version {
            P4kVersion::V1 => {
                self.payload_slice_by_local_header(
                    entry.name,
                    entry.local_header_offset,
                    entry.compressed_size,
                    entry.uncompressed_size,
                    entry.compression_method,
                    entry.crc32,
                    entry.last_mod_file_time,
                    entry.last_mod_file_date,
                    entry.sha256,
                )?
                .2
            }
            P4kVersion::V2 => {
                self.payload_slice_by_data_offset(entry.local_header_offset, entry.compressed_size)?
            }
        };

        if payload.len() as u64 != entry.uncompressed_size {
            return Err(Error::Decompression(format!(
                "stored entry size mismatch: expected {}, got {}",
                entry.uncompressed_size,
                payload.len()
            )));
        }
        Ok(Some(payload))
    }

    fn raw_payload_slice<'a>(&'a self, entry: &P4kEntryRef<'_>) -> Result<&'a [u8]> {
        match self.version {
            P4kVersion::V1 => Ok(self
                .payload_slice_by_local_header(
                    entry.name,
                    entry.local_header_offset,
                    entry.compressed_size,
                    entry.uncompressed_size,
                    entry.compression_method,
                    entry.crc32,
                    entry.last_mod_file_time,
                    entry.last_mod_file_date,
                    entry.sha256,
                )?
                .2),
            P4kVersion::V2 => {
                self.payload_slice_by_data_offset(entry.local_header_offset, entry.compressed_size)
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn payload_slice_by_local_header(
        &self,
        expected_name: &str,
        local_header_offset: u64,
        compressed_size: u64,
        uncompressed_size: u64,
        compression_method: CompressionMethod,
        crc32: u32,
        last_mod_file_time: u16,
        last_mod_file_date: u16,
        expected_sha256: [u8; 32],
    ) -> Result<(u64, u64, &[u8], bool)> {
        let (data_offset, local_record_size, sha256_verified) = self
            .payload_location_by_local_header(
                expected_name,
                local_header_offset,
                compressed_size,
                uncompressed_size,
                compression_method,
                crc32,
                last_mod_file_time,
                last_mod_file_date,
                expected_sha256,
            )?;
        let payload = self.slice_range(data_offset, compressed_size, "entry data out of bounds")?;
        Ok((data_offset, local_record_size, payload, sha256_verified))
    }

    #[allow(clippy::too_many_arguments)]
    fn payload_location_by_local_header(
        &self,
        expected_name: &str,
        local_header_offset: u64,
        compressed_size: u64,
        uncompressed_size: u64,
        compression_method: CompressionMethod,
        crc32: u32,
        last_mod_file_time: u16,
        last_mod_file_date: u16,
        expected_sha256: [u8; 32],
    ) -> Result<(u64, u64, bool)> {
        let offset = usize::try_from(local_header_offset).map_err(|_| {
            Error::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "local header offset overflows usize",
            ))
        })?;
        let Some(signature_end) = offset.checked_add(4) else {
            return Err(Error::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "local header signature range overflow",
            )));
        };
        if signature_end > self.mmap.len() {
            return Err(Error::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "local header offset out of bounds",
            )));
        }

        let sig = u32::from_le_bytes([
            self.mmap[offset],
            self.mmap[offset + 1],
            self.mmap[offset + 2],
            self.mmap[offset + 3],
        ]);

        if sig != LocalFileHeader::SIGNATURE && sig != LocalFileHeader::SIGNATURE_EXTENDED {
            return Err(Error::InvalidSignature {
                expected: LocalFileHeader::SIGNATURE,
                actual: sig,
            });
        }

        let header_start = signature_end;
        let header_size = std::mem::size_of::<LocalFileHeader>();
        let Some(header_end) = header_start.checked_add(header_size) else {
            return Err(Error::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "local header range overflow",
            )));
        };
        if header_end > self.mmap.len() {
            return Err(Error::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "local header out of bounds",
            )));
        }

        let mut reader = BinaryReader::new(&self.mmap[header_start..]);
        let local_header: LocalFileHeader = reader.read_struct()?;
        validate_v1_local_header_metadata(
            &local_header,
            expected_name,
            compression_method,
            crc32,
            last_mod_file_time,
            last_mod_file_date,
        )?;
        validate_v1_local_extra_fields(
            &self.mmap,
            offset,
            header_size,
            &local_header,
            expected_name,
            self.v1_local_header_span()?,
            local_header_offset,
            compressed_size,
            uncompressed_size,
        )?;
        if compressed_size != 0 {
            let preferred_offset = match self.v1_payload_placement {
                V1PayloadPlacement::Unknown => None,
                V1PayloadPlacement::ZipLocalSector => self
                    .layout
                    .physical_sector_size
                    .and_then(|sector_size| local_header_offset.checked_add(sector_size))
                    .and_then(|offset| usize::try_from(offset).ok()),
                V1PayloadPlacement::ZipLocal => {
                    zip_local_payload_offset(offset, header_size, &local_header).ok()
                }
                V1PayloadPlacement::AlignedRecord => self
                    .layout
                    .physical_sector_size
                    .and_then(|sector_size| {
                        v1_local_record_size_from_name_len(
                            local_header.file_name_length as u64,
                            sector_size,
                        )
                    })
                    .and_then(|record_size| local_header_offset.checked_add(record_size))
                    .and_then(|offset| usize::try_from(offset).ok()),
            };
            if let Some(data_offset) = preferred_offset {
                self.slice_range(
                    data_offset as u64,
                    compressed_size,
                    "entry data out of bounds",
                )?;
                return Ok((
                    data_offset as u64,
                    checked_local_record_size(offset, data_offset)?,
                    false,
                ));
            }
        }
        let (data_offset, sha256_verified) = select_v1_payload_offset(
            &self.mmap,
            offset,
            header_size,
            &local_header,
            compressed_size,
            expected_sha256,
        )?;
        self.slice_range(
            data_offset as u64,
            compressed_size,
            "entry data out of bounds",
        )?;
        Ok((
            data_offset as u64,
            checked_local_record_size(offset, data_offset)?,
            sha256_verified,
        ))
    }

    fn payload_slice_by_data_offset(
        &self,
        data_offset: u64,
        compressed_size: u64,
    ) -> Result<&[u8]> {
        self.slice_range(data_offset, compressed_size, "v2 entry data out of bounds")
    }

    fn slice_range(&self, offset: u64, size: u64, eof_msg: &'static str) -> Result<&[u8]> {
        let start = usize::try_from(offset).map_err(|_| {
            Error::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "payload offset overflows usize",
            ))
        })?;
        let len = usize::try_from(size).map_err(|_| {
            Error::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "payload size overflows usize",
            ))
        })?;
        let end = start.checked_add(len).ok_or_else(|| {
            Error::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "payload range overflow",
            ))
        })?;
        if end > self.mmap.len() {
            return Err(Error::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                eof_msg,
            )));
        }
        Ok(&self.mmap[start..end])
    }

    fn payload_range_fits(&self, offset: u64, size: u64) -> bool {
        let Ok(start) = usize::try_from(offset) else {
            return false;
        };
        let Ok(len) = usize::try_from(size) else {
            return false;
        };
        let Some(end) = start.checked_add(len) else {
            return false;
        };
        end <= self.mmap.len()
    }

    fn v1_local_header_span(&self) -> Result<Option<usize>> {
        self.layout
            .physical_sector_size
            .map(|sector_size| {
                usize::try_from(sector_size).map_err(|_| {
                    Error::MalformedV1Entry("physical sector size overflows usize".to_string())
                })
            })
            .transpose()
    }

    /// Detect the on-disk version and dispatch to the right parser.
    ///
    /// V2 is identified by the "JiJi" magic in the final 4 bytes of
    /// the actual content (after stripping null padding) plus a `2`
    /// version word immediately preceding it. Anything else falls
    /// through to the legacy ZIP64 parser.
    fn parse_entries_dispatched(
        data: &[u8],
    ) -> Result<(
        P4kVersion,
        Vec<P4kEntryCompact>,
        Vec<P4kFreelistBlock>,
        P4kArchiveLayout,
    )> {
        let actual_end = simd::find_content_end(data);

        if let Some((entries, freelist_blocks, layout)) = Self::try_parse_v2(data, actual_end)? {
            return Ok((P4kVersion::V2, entries, freelist_blocks, layout));
        }

        let (entries, freelist_blocks, layout) = Self::parse_entries_v1(data, actual_end)?;
        Ok((P4kVersion::V1, entries, freelist_blocks, layout))
    }

    /// Try to parse the archive as a v2 ("JiJi") P4K. Returns
    /// `Ok(None)` if the trailer magic does not match (so the caller
    /// can fall back to v1).
    fn try_parse_v2(data: &[u8], actual_end: usize) -> Result<ParsedV2Archive> {
        if actual_end < EOCD_V2_SIZE {
            return Ok(None);
        }

        // Magic and version sit at the very last 6 bytes of content.
        let magic_off = actual_end - 4;
        let magic = u32::from_le_bytes([
            data[magic_off],
            data[magic_off + 1],
            data[magic_off + 2],
            data[magic_off + 3],
        ]);
        if magic != EOCD_V2_MAGIC {
            return Ok(None);
        }

        let ver_off = actual_end - 6;
        let version = u16::from_le_bytes([data[ver_off], data[ver_off + 1]]);
        if version != EOCD_V2_VERSION {
            return Err(Error::UnsupportedVersion(version));
        }

        let eocdr_start = actual_end - EOCD_V2_SIZE;
        let eocdr_bytes = &data[eocdr_start..actual_end];
        let eocdr = <Eocd2Record as zerocopy::FromBytes>::read_from_bytes(eocdr_bytes)
            .map_err(|_| Error::MalformedV2Eocdr("failed to read EOCDR bytes".to_string()))?;

        if !eocdr.is_valid() {
            return Err(Error::MalformedV2Eocdr(
                "EOCDR magic/version mismatch after copy".to_string(),
            ));
        }

        let parsed = Self::parse_entries_v2(data, &eocdr, actual_end)?;
        Ok(Some(parsed))
    }

    /// Parse the v2 CDR + name table into `P4kEntryCompact` records.
    fn parse_entries_v2(
        data: &[u8],
        eocdr: &Eocd2Record,
        actual_end: usize,
    ) -> Result<(
        Vec<P4kEntryCompact>,
        Vec<P4kFreelistBlock>,
        P4kArchiveLayout,
    )> {
        let reserved_08 = eocdr.reserved_08;
        let reserved_20 = eocdr.reserved_20;
        let reserved_38 = eocdr.reserved_38;
        let reserved_50 = eocdr.reserved_50;
        let reserved_58 = eocdr.reserved_58;
        for (name, value) in [
            ("reserved_08", reserved_08),
            ("reserved_20", reserved_20),
            ("reserved_38", reserved_38),
            ("reserved_50", reserved_50),
            ("reserved_58", reserved_58),
        ] {
            if value != 0 {
                return Err(Error::MalformedV2Eocdr(format!(
                    "{name} is {value}, expected 0"
                )));
            }
        }
        let flag_68 = eocdr.flag_68;
        if flag_68 != 1 {
            return Err(Error::MalformedV2Eocdr(format!(
                "flag_68 is {flag_68}, expected 1"
            )));
        }

        let cdr_start = usize::try_from(eocdr.end_of_file_block_offset)
            .map_err(|_| Error::MalformedV2Eocdr("CDR offset overflows usize".to_string()))?;
        let cdr_size = usize::try_from(eocdr.cdr_size)
            .map_err(|_| Error::MalformedV2Eocdr("CDR size overflows usize".to_string()))?;
        let num_entries = usize::try_from(eocdr.num_file_entries)
            .map_err(|_| Error::MalformedV2Eocdr("entry count overflows usize".to_string()))?;
        let num_freelist_blocks = usize::try_from(eocdr.num_freelist_blocks).map_err(|_| {
            Error::MalformedV2Eocdr("freelist block count overflows usize".to_string())
        })?;
        let names_start = usize::try_from(eocdr.name_table_abs_offset).map_err(|_| {
            Error::MalformedV2Eocdr("name table offset overflows usize".to_string())
        })?;
        let names_len = usize::try_from(eocdr.total_name_length)
            .map_err(|_| Error::MalformedV2Eocdr("name table size overflows usize".to_string()))?;
        let end_of_payload = usize::try_from(eocdr.end_of_payload)
            .map_err(|_| Error::MalformedV2Eocdr("end_of_payload overflows usize".to_string()))?;
        let physical_sector_size = eocdr.physical_sector_size;
        if physical_sector_size == 0 || !physical_sector_size.is_power_of_two() {
            return Err(Error::MalformedV2Eocdr(format!(
                "physical sector size {physical_sector_size}, expected non-zero power of two"
            )));
        }
        let physical_sector_size = usize::try_from(physical_sector_size).map_err(|_| {
            Error::MalformedV2Eocdr("physical sector size overflows usize".to_string())
        })?;
        if data.len() % physical_sector_size != 0 {
            return Err(Error::MalformedV2Eocdr(format!(
                "P4K v2 file size {} is not aligned to physical sector size {}",
                data.len(),
                physical_sector_size
            )));
        }

        let expected_cdr_size = num_entries
            .checked_mul(CDR_V2_ENTRY_SIZE)
            .ok_or_else(|| Error::MalformedV2Eocdr("cdr size overflow".to_string()))?;
        if cdr_size != expected_cdr_size {
            return Err(Error::MalformedV2Eocdr(format!(
                "cdr_size {} does not match num_entries {} * {} = {}",
                cdr_size, num_entries, CDR_V2_ENTRY_SIZE, expected_cdr_size
            )));
        }

        let cdr_end = cdr_start
            .checked_add(cdr_size)
            .ok_or_else(|| Error::MalformedV2Eocdr("cdr range overflow".to_string()))?;
        if cdr_start % V2_CDR_ALIGNMENT != 0 {
            return Err(Error::MalformedV2Eocdr(format!(
                "CDR offset {cdr_start} is not aligned to {V2_CDR_ALIGNMENT}"
            )));
        }
        let names_end = names_start
            .checked_add(names_len)
            .ok_or_else(|| Error::MalformedV2Eocdr("name table range overflow".to_string()))?;
        if names_start != cdr_end {
            return Err(Error::MalformedV2Eocdr(format!(
                "name table offset {} does not follow CDR end {}",
                names_start, cdr_end
            )));
        }
        let eof_used = cdr_size
            .checked_add(names_len)
            .and_then(|value| value.checked_add(EOCD_V2_SIZE))
            .ok_or_else(|| Error::MalformedV2Eocdr("EOF buffer size overflow".to_string()))?;
        let eof_aligned = eof_used
            .checked_add(physical_sector_size - 1)
            .map(|value| value & !(physical_sector_size - 1))
            .ok_or_else(|| Error::MalformedV2Eocdr("EOF buffer alignment overflow".to_string()))?;
        let expected_actual_end = cdr_start
            .checked_add(eof_aligned)
            .ok_or_else(|| Error::MalformedV2Eocdr("EOF buffer end overflow".to_string()))?;
        if actual_end != expected_actual_end {
            return Err(Error::MalformedV2Eocdr(format!(
                "EOF buffer ends at {actual_end}, expected {expected_actual_end}"
            )));
        }
        let eocdr_start = actual_end - EOCD_V2_SIZE;
        if cdr_end > data.len() || names_end > data.len() {
            return Err(Error::MalformedV2Eocdr(
                "cdr or name table extends past file end".to_string(),
            ));
        }
        if names_end > eocdr_start {
            return Err(Error::MalformedV2Eocdr(format!(
                "name table end {} overlaps EOCDR at {}",
                names_end, eocdr_start
            )));
        }
        if data[names_end..eocdr_start].iter().any(|byte| *byte != 0) {
            return Err(Error::MalformedV2Eocdr(
                "non-zero EOF padding before EOCDR".to_string(),
            ));
        }
        if end_of_payload > cdr_start {
            return Err(Error::MalformedV2Eocdr(format!(
                "end_of_payload {} is after CDR offset {}",
                end_of_payload, cdr_start
            )));
        }

        let cdr_bytes = &data[cdr_start..cdr_end];
        let names_bytes = &data[names_start..names_end];
        let install_bytes_needed = num_entries
            .checked_add(num_freelist_blocks.checked_mul(2).ok_or_else(|| {
                Error::MalformedV2Eocdr("install block freelist count overflow".to_string())
            })?)
            .and_then(|slots| slots.checked_mul(8))
            .ok_or_else(|| Error::MalformedV2Eocdr("install block size overflow".to_string()))?;
        let install_bytes_available = cdr_start - end_of_payload;
        if install_bytes_available < install_bytes_needed {
            return Err(Error::MalformedV2Eocdr(format!(
                "install block has {} bytes, need {}",
                install_bytes_available, install_bytes_needed
            )));
        }
        let entry_install_bytes = num_entries.checked_mul(8).ok_or_else(|| {
            Error::MalformedV2Eocdr("entry install block size overflow".to_string())
        })?;
        let install_block = &data[end_of_payload..end_of_payload + entry_install_bytes];
        let freelist_start = end_of_payload + entry_install_bytes;
        let freelist_bytes = num_freelist_blocks
            .checked_mul(16)
            .ok_or_else(|| Error::MalformedV2Eocdr("freelist byte size overflow".to_string()))?;
        let freelist_end = freelist_start + freelist_bytes;
        let freelist_blocks = read_freelist_blocks(
            &data[freelist_start..freelist_end],
            num_freelist_blocks,
            Error::MalformedV2Eocdr,
        )?;
        if data[freelist_end..cdr_start].iter().any(|byte| *byte != 0) {
            return Err(Error::MalformedV2Eocdr(
                "non-zero install padding before CDR".to_string(),
            ));
        }

        let (entries, mut payload_ranges) = Self::parse_v2_entries(
            cdr_bytes,
            names_bytes,
            num_entries,
            install_block,
            physical_sector_size,
            end_of_payload,
            data.len(),
        )?;
        payload_ranges.sort_unstable_by_key(|&(start, _, _)| start);
        for pair in payload_ranges.windows(2) {
            let (_, previous_end, previous_index) = pair[0];
            let (current_start, current_end, current_index) = pair[1];
            if current_start < previous_end {
                return Err(Error::MalformedV2Entry(format!(
                    "entry {current_index} data range [{current_start}, {current_end}) overlaps entry {previous_index} ending at {previous_end}"
                )));
            }
        }
        let layout = P4kArchiveLayout {
            file_size: data.len() as u64,
            actual_content_end: actual_end as u64,
            physical_sector_size: Some(eocdr.physical_sector_size),
            cdr_offset: eocdr.end_of_file_block_offset,
            cdr_size: eocdr.cdr_size,
            name_table_offset: Some(eocdr.name_table_abs_offset),
            name_table_size: eocdr.total_name_length,
            end_of_payload: Some(eocdr.end_of_payload),
            install_block_offset: Some(eocdr.end_of_payload),
            install_block_size: Some(eocdr.end_of_file_block_offset - eocdr.end_of_payload),
            eocd_offset: (actual_end - EOCD_V2_SIZE) as u64,
            manifest_sha256: Some(eocdr.manifest_sha256),
        };

        Ok((entries, freelist_blocks, layout))
    }

    #[cfg(feature = "parallel")]
    fn parse_v2_names(
        cdr_bytes: &[u8],
        names_bytes: &[u8],
        num_entries: usize,
    ) -> Result<Vec<String>> {
        let mut names = Vec::with_capacity(num_entries);
        let mut min_next_name_offset = 0usize;
        for i in 0..num_entries {
            let raw_name_offset = Self::read_v2_name_offset(cdr_bytes, i)?;
            names.push(Self::parse_v2_name_at(
                names_bytes,
                i,
                raw_name_offset,
                &mut min_next_name_offset,
            )?);
        }
        Ok(names)
    }

    fn parse_v2_name_at(
        names_bytes: &[u8],
        index: usize,
        raw_name_offset: u64,
        min_next_name_offset: &mut usize,
    ) -> Result<String> {
        let name_off = usize::try_from(raw_name_offset).map_err(|_| {
            Error::MalformedV2Entry(format!(
                "entry {} name offset {} overflows usize",
                index, raw_name_offset
            ))
        })?;
        if name_off >= names_bytes.len() {
            return Err(Error::MalformedV2Entry(format!(
                "entry {} name offset {} >= name table size {}",
                index,
                name_off,
                names_bytes.len()
            )));
        }
        if name_off < *min_next_name_offset {
            return Err(Error::MalformedV2Entry(format!(
                "entry {} name offset {} is before previous name end {}",
                index, name_off, min_next_name_offset
            )));
        }
        let tail = &names_bytes[name_off..];
        let null_pos = memchr::memchr(0, tail).ok_or_else(|| {
            Error::MalformedV2Entry(format!(
                "entry {} name at offset {} is not null-terminated",
                index, name_off
            ))
        })?;
        *min_next_name_offset = name_off.checked_add(null_pos + 1).ok_or_else(|| {
            Error::MalformedV2Entry(format!("entry {} name offset overflow", index))
        })?;
        let raw = &tail[..null_pos];
        let raw_name = std::str::from_utf8(raw).map_err(|err| {
            Error::MalformedV2Entry(format!("entry {} name is not valid UTF-8: {}", index, err))
        })?;
        if p4k_path_bytes_need_normalization(raw, false) {
            return Err(Error::MalformedV2Entry(format!(
                "entry {} name {:?} is not normalized",
                index, raw_name
            )));
        }
        if raw_name.is_empty() || raw_name.starts_with('/') {
            return Err(Error::MalformedV2Entry(format!(
                "entry {} name {:?} is not a relative archive path",
                index, raw_name
            )));
        }
        Ok(raw_name.to_owned())
    }

    #[cfg(any(feature = "parallel", test))]
    fn read_v2_name_offset(cdr_bytes: &[u8], index: usize) -> Result<u64> {
        let record_off = index
            .checked_mul(CDR_V2_ENTRY_SIZE)
            .ok_or_else(|| Error::MalformedV2Entry(format!("entry {} offset overflow", index)))?;
        let name_off = record_off
            .checked_add(CDR_V2_OFFSET_TO_FILENAME_OFFSET)
            .ok_or_else(|| Error::MalformedV2Entry(format!("entry {} offset overflow", index)))?;
        let name_off_end = name_off
            .checked_add(8)
            .ok_or_else(|| Error::MalformedV2Entry(format!("entry {} offset overflow", index)))?;
        let name_offset_bytes = cdr_bytes.get(name_off..name_off_end).ok_or_else(|| {
            Error::MalformedV2Entry(format!(
                "entry {} CDR name-offset field extends past CDR size {}",
                index,
                cdr_bytes.len()
            ))
        })?;
        Ok(u64::from_le_bytes(
            name_offset_bytes
                .try_into()
                .expect("name offset slice length checked"),
        ))
    }

    fn parse_v2_entries(
        cdr_bytes: &[u8],
        names_bytes: &[u8],
        num_entries: usize,
        install_block: &[u8],
        physical_sector_size: usize,
        end_of_payload: usize,
        data_len: usize,
    ) -> Result<ParsedV2Entries> {
        #[cfg(feature = "parallel")]
        {
            if num_entries >= V2_PARALLEL_CDR_ENTRY_THRESHOLD {
                let names = Self::parse_v2_names(cdr_bytes, names_bytes, num_entries)?;
                let parsed_entries = Self::parse_v2_entries_parallel(
                    cdr_bytes,
                    names,
                    install_block,
                    physical_sector_size,
                    end_of_payload,
                    data_len,
                )?;
                return Ok(Self::split_parsed_v2_entries(parsed_entries));
            }
        }

        let mut entries = Vec::with_capacity(num_entries);
        let mut payload_ranges = Vec::with_capacity(num_entries);
        let mut min_next_name_offset = 0usize;
        for index in 0..num_entries {
            let entry = Self::read_v2_cdr_entry(cdr_bytes, index)?;
            let name = Self::parse_v2_name_at(
                names_bytes,
                index,
                entry.offset_to_filename,
                &mut min_next_name_offset,
            )?;
            let parsed = Self::parse_v2_entry_record(
                entry,
                index,
                name,
                install_block,
                physical_sector_size,
                end_of_payload,
                data_len,
            )?;
            if let Some(range) = parsed.payload_range {
                payload_ranges.push(range);
            }
            entries.push(parsed.entry);
        }

        Ok((entries, payload_ranges))
    }

    #[cfg(feature = "parallel")]
    fn split_parsed_v2_entries(parsed_entries: Vec<ParsedV2Entry>) -> ParsedV2Entries {
        let mut entries = Vec::with_capacity(parsed_entries.len());
        let mut payload_ranges = Vec::with_capacity(parsed_entries.len());
        for parsed in parsed_entries {
            if let Some(range) = parsed.payload_range {
                payload_ranges.push(range);
            }
            entries.push(parsed.entry);
        }
        (entries, payload_ranges)
    }

    #[cfg(feature = "parallel")]
    fn parse_v2_entries_parallel(
        cdr_bytes: &[u8],
        names: Vec<String>,
        install_block: &[u8],
        physical_sector_size: usize,
        end_of_payload: usize,
        data_len: usize,
    ) -> Result<Vec<ParsedV2Entry>> {
        use rayon::prelude::*;

        names
            .into_par_iter()
            .enumerate()
            .map(|(index, name)| {
                Self::parse_v2_entry(
                    cdr_bytes,
                    index,
                    name,
                    install_block,
                    physical_sector_size,
                    end_of_payload,
                    data_len,
                )
            })
            .collect()
    }

    #[cfg(feature = "parallel")]
    fn parse_v2_entry(
        cdr_bytes: &[u8],
        index: usize,
        name: String,
        install_block: &[u8],
        physical_sector_size: usize,
        end_of_payload: usize,
        data_len: usize,
    ) -> Result<ParsedV2Entry> {
        let entry = Self::read_v2_cdr_entry(cdr_bytes, index)?;
        Self::parse_v2_entry_record(
            entry,
            index,
            name,
            install_block,
            physical_sector_size,
            end_of_payload,
            data_len,
        )
    }

    fn parse_v2_entry_record(
        entry: CentralDirectoryHeaderV2,
        index: usize,
        name: String,
        install_block: &[u8],
        physical_sector_size: usize,
        end_of_payload: usize,
        data_len: usize,
    ) -> Result<ParsedV2Entry> {
        let compressed_size = entry.compressed_size;
        let uncompressed_size = entry.uncompressed_size;
        let data_offset = entry.offset_to_file_data;
        let compression_method_raw = entry.compression_method;
        let method = CompressionMethod::try_from(compression_method_raw)
            .map_err(Error::UnsupportedCompression)?;
        let encryption_flag = entry.encryption_flag;
        if encryption_flag > 1 {
            return Err(Error::MalformedV2Entry(format!(
                "entry {} encryption flag {}, expected 0 or 1",
                index, encryption_flag
            )));
        }
        let is_encrypted = entry.is_encrypted();
        if data_offset % physical_sector_size as u64 != 0 {
            return Err(Error::MalformedV2Entry(format!(
                "entry {} data offset {} is not aligned to physical sector size {}",
                index, data_offset, physical_sector_size
            )));
        }
        let data_start = usize::try_from(data_offset).map_err(|_| {
            Error::MalformedV2Entry(format!(
                "entry {} data offset {} overflows usize",
                index, data_offset
            ))
        })?;
        let compressed_len = usize::try_from(compressed_size).map_err(|_| {
            Error::MalformedV2Entry(format!(
                "entry {} compressed size {} overflows usize",
                index, compressed_size
            ))
        })?;
        let data_end = data_start.checked_add(compressed_len).ok_or_else(|| {
            Error::MalformedV2Entry(format!("entry {} data range overflow", index))
        })?;
        if data_end > end_of_payload {
            return Err(Error::MalformedV2Entry(format!(
                "entry {} data range [{}, {}) extends past end_of_payload {}",
                index, data_offset, data_end, end_of_payload
            )));
        }
        if data_end > data_len {
            return Err(Error::MalformedV2Entry(format!(
                "entry {} data range [{}, {}) extends past file size {}",
                index, data_offset, data_end, data_len
            )));
        }

        let mut flags = FLAG_V2_NO_LOCAL_HEADER;
        if is_encrypted {
            flags |= FLAG_ENCRYPTED;
        }
        let install_off = index * 8;
        let bytes_already_written = u64::from_le_bytes(
            install_block[install_off..install_off + 8]
                .try_into()
                .expect("install block slice length checked"),
        );
        if bytes_already_written > compressed_size {
            return Err(Error::MalformedV2Entry(format!(
                "entry {} bytes_already_written {} > compressed_size {}",
                index, bytes_already_written, compressed_size
            )));
        }

        Ok(ParsedV2Entry {
            entry: P4kEntryCompact {
                name,
                compressed_size,
                uncompressed_size,
                compression_method: method as u8,
                flags,
                local_header_offset: data_offset,
                crc32: entry.crc32,
                last_mod_file_time: entry.last_mod_file_time,
                last_mod_file_date: entry.last_mod_file_date,
                signature: entry.signature,
                sha256: entry.sha256,
                bytes_already_written,
            },
            payload_range: (compressed_len != 0).then_some((data_start, data_end, index)),
        })
    }

    fn read_v2_cdr_entry(cdr_bytes: &[u8], index: usize) -> Result<CentralDirectoryHeaderV2> {
        let off = index
            .checked_mul(CDR_V2_ENTRY_SIZE)
            .ok_or_else(|| Error::MalformedV2Entry(format!("entry {} offset overflow", index)))?;
        let entry_bytes = cdr_bytes.get(off..off + CDR_V2_ENTRY_SIZE).ok_or_else(|| {
            Error::MalformedV2Entry(format!(
                "entry {} CDR record extends past CDR size {}",
                index,
                cdr_bytes.len()
            ))
        })?;
        <CentralDirectoryHeaderV2 as zerocopy::FromBytes>::read_from_bytes(entry_bytes)
            .map_err(|_| Error::MalformedV2Entry(format!("failed to read entry {} bytes", index)))
    }

    /// Parse the v1 (ZIP64) central directory.
    fn parse_entries_v1(
        data: &[u8],
        actual_end: usize,
    ) -> Result<(
        Vec<P4kEntryCompact>,
        Vec<P4kFreelistBlock>,
        P4kArchiveLayout,
    )> {
        Self::parse_entries_optimized_at(data, actual_end)
    }

    /// Parse entries with SIMD-accelerated operations.
    fn parse_entries_optimized_at(
        data: &[u8],
        actual_end: usize,
    ) -> Result<(
        Vec<P4kEntryCompact>,
        Vec<P4kFreelistBlock>,
        P4kArchiveLayout,
    )> {
        // Find EOCD record
        let eocd_offset = Self::find_eocd_optimized(data, actual_end)?;
        let mut reader = BinaryReader::new(&data[eocd_offset..]);

        reader.advance(4); // Skip signature
        let eocd: EocdRecord = reader.read_struct()?;

        // Get ZIP64 values if needed
        let is_zip64 = eocd.is_zip64();
        let (total_entries, central_dir_offset, central_dir_size) = if is_zip64 {
            validate_v1_zip64_classic_eocd(&eocd)?;
            Self::read_zip64_eocd(data, eocd_offset)?
        } else {
            (
                eocd.central_dir_count_total as u64,
                eocd.central_dir_offset as u64,
                eocd.central_dir_size as u64,
            )
        };
        let central_dir_start = usize::try_from(central_dir_offset).map_err(|_| {
            Error::MalformedV1InstallBlock("central directory offset overflows usize".to_string())
        })?;
        let central_dir_len = usize::try_from(central_dir_size).map_err(|_| {
            Error::MalformedV1InstallBlock("central directory size overflows usize".to_string())
        })?;
        let central_dir_end = central_dir_start
            .checked_add(central_dir_len)
            .ok_or_else(|| {
                Error::MalformedV1InstallBlock("central directory range overflow".to_string())
            })?;
        if central_dir_end > eocd_offset {
            return Err(Error::MalformedV1InstallBlock(format!(
                "central directory range [{central_dir_start}, {central_dir_end}) extends past EOCD at {eocd_offset}"
            )));
        }

        let total_entries = usize::try_from(total_entries).map_err(|_| {
            Error::MalformedV1InstallBlock(
                "central directory entry count overflows usize".to_string(),
            )
        })?;
        let mut entries = Vec::with_capacity(total_entries);

        // Parse central directory
        let cd_data = &data[central_dir_start..central_dir_end];

        let parsed_cdr_len = Self::parse_entries(cd_data, total_entries, is_zip64, &mut entries)?;
        if parsed_cdr_len != cd_data.len() {
            return Err(Error::MalformedV1InstallBlock(format!(
                "central directory parser consumed {} bytes, expected {}",
                parsed_cdr_len,
                cd_data.len()
            )));
        }
        let mut freelist_blocks = Vec::new();
        let mut physical_sector_size = None;
        let mut install_block_offset = None;
        let mut install_block_size = None;
        if is_zip64 {
            let (freelist_block_count, read_physical_sector_size) =
                Self::read_v1_freelist_metadata(data, eocd_offset, &eocd)?;
            physical_sector_size = read_physical_sector_size;
            let entry_install_size = entries.len().checked_mul(12).ok_or_else(|| {
                Error::MalformedV1InstallBlock("install block size overflow".to_string())
            })?;
            let freelist_size = usize::try_from(freelist_block_count)
                .map_err(|_| {
                    Error::MalformedV1InstallBlock(
                        "freelist block count overflows usize".to_string(),
                    )
                })?
                .checked_mul(16)
                .ok_or_else(|| {
                    Error::MalformedV1InstallBlock("freelist block size overflow".to_string())
                })?;
            let block_size = entry_install_size
                .checked_add(freelist_size)
                .ok_or_else(|| {
                    Error::MalformedV1InstallBlock("install block size overflow".to_string())
                })?;
            if block_size != 0 {
                let central_dir_offset_usize =
                    usize::try_from(central_dir_offset).map_err(|_| {
                        Error::MalformedV1InstallBlock(
                            "central directory offset overflows usize".to_string(),
                        )
                    })?;
                if central_dir_offset_usize < block_size {
                    return Err(Error::MalformedV1InstallBlock(format!(
                        "central directory offset {} is before {}-byte install block",
                        central_dir_offset_usize, block_size
                    )));
                }
                install_block_offset = Some((central_dir_offset_usize - block_size) as u64);
                install_block_size = Some(block_size as u64);
            }
            freelist_blocks = Self::apply_v1_install_block(
                data,
                central_dir_offset,
                freelist_block_count,
                &mut entries,
            )?;
            if let Some(physical_sector_size) = physical_sector_size {
                Self::deduplicate_v1_entries(
                    data,
                    physical_sector_size,
                    &mut entries,
                    &mut freelist_blocks,
                )?;
            }
        }

        let layout = P4kArchiveLayout {
            file_size: data.len() as u64,
            actual_content_end: actual_end as u64,
            physical_sector_size,
            cdr_offset: central_dir_offset,
            cdr_size: central_dir_size,
            name_table_offset: None,
            name_table_size: 0,
            end_of_payload: install_block_offset,
            install_block_offset,
            install_block_size,
            eocd_offset: eocd_offset as u64,
            manifest_sha256: None,
        };

        Ok((entries, freelist_blocks, layout))
    }

    fn parse_entries(
        cd_data: &[u8],
        count: usize,
        is_zip64: bool,
        entries: &mut Vec<P4kEntryCompact>,
    ) -> Result<usize> {
        #[cfg(feature = "parallel")]
        {
            if is_zip64 && count >= V1_PARALLEL_CDR_ENTRY_THRESHOLD {
                return Self::parse_entries_parallel(cd_data, count, is_zip64, entries);
            }
        }

        Self::parse_entries_sequential(cd_data, count, is_zip64, entries)
    }

    fn parse_entries_sequential(
        cd_data: &[u8],
        count: usize,
        is_zip64: bool,
        entries: &mut Vec<P4kEntryCompact>,
    ) -> Result<usize> {
        if is_zip64 {
            let mut offset = 0usize;
            for index in 0..count {
                let (entry, next_offset) = Self::read_v1_p4k_cd_entry_at(cd_data, offset, index)?;
                entries.push(entry);
                offset = next_offset;
            }
            return Ok(offset);
        }

        let mut reader = BinaryReader::new(cd_data);

        for _ in 0..count {
            let entry = Self::read_cd_entry_compact(&mut reader, is_zip64)?;
            entries.push(entry);
        }

        Ok(reader.position())
    }

    #[cfg(feature = "parallel")]
    fn parse_entries_parallel(
        cd_data: &[u8],
        count: usize,
        is_zip64: bool,
        entries: &mut Vec<P4kEntryCompact>,
    ) -> Result<usize> {
        use rayon::prelude::*;

        let (spans, consumed) = Self::scan_cdr_record_spans(cd_data, count)?;
        let parsed = spans
            .par_iter()
            .map(|span| {
                Self::read_cd_entry_compact_from_slice(&cd_data[span.start..span.end], is_zip64)
            })
            .collect::<Result<Vec<_>>>()?;
        entries.extend(parsed);
        Ok(consumed)
    }

    #[cfg(feature = "parallel")]
    fn scan_cdr_record_spans(cd_data: &[u8], count: usize) -> Result<(Vec<CdrRecordSpan>, usize)> {
        let mut spans = Vec::with_capacity(count);
        let mut offset = 0usize;
        let header_len = std::mem::size_of::<CentralDirectoryHeader>();

        for index in 0..count {
            let signature_end = offset.checked_add(4).ok_or_else(|| {
                Error::MalformedV1InstallBlock("central directory offset overflow".to_string())
            })?;
            let header_end = signature_end.checked_add(header_len).ok_or_else(|| {
                Error::MalformedV1InstallBlock("central directory header overflow".to_string())
            })?;
            if header_end > cd_data.len() {
                return Err(Error::MalformedV1InstallBlock(format!(
                    "central directory record {index} header extends past CDR size {}",
                    cd_data.len()
                )));
            }

            let sig = u32::from_le_bytes(
                cd_data[offset..signature_end]
                    .try_into()
                    .expect("central directory signature slice length checked"),
            );
            if sig != CentralDirectoryHeader::SIGNATURE {
                return Err(Error::InvalidSignature {
                    expected: CentralDirectoryHeader::SIGNATURE,
                    actual: sig,
                });
            }

            let header = <CentralDirectoryHeader as zerocopy::FromBytes>::read_from_bytes(
                &cd_data[signature_end..header_end],
            )
            .map_err(|_| {
                Error::MalformedV1InstallBlock(format!(
                    "failed to read central directory record {index} header"
                ))
            })?;
            let variable_len = header.variable_data_size();
            let record_end = header_end.checked_add(variable_len).ok_or_else(|| {
                Error::MalformedV1InstallBlock(format!(
                    "central directory record {index} size overflow"
                ))
            })?;
            if record_end > cd_data.len() {
                return Err(Error::MalformedV1InstallBlock(format!(
                    "central directory record {index} ends at {record_end}, past CDR size {}",
                    cd_data.len()
                )));
            }

            spans.push(CdrRecordSpan {
                start: offset,
                end: record_end,
            });
            offset = record_end;
        }

        Ok((spans, offset))
    }

    #[cfg(feature = "parallel")]
    fn read_cd_entry_compact_from_slice(bytes: &[u8], is_zip64: bool) -> Result<P4kEntryCompact> {
        if is_zip64 {
            let (entry, consumed) = Self::read_v1_p4k_cd_entry_at(bytes, 0, 0)?;
            if consumed != bytes.len() {
                return Err(Error::MalformedV1InstallBlock(format!(
                    "central directory record parser consumed {} bytes, expected {}",
                    consumed,
                    bytes.len()
                )));
            }
            return Ok(entry);
        }

        let mut reader = BinaryReader::new(bytes);
        let entry = Self::read_cd_entry_compact(&mut reader, is_zip64)?;
        if reader.position() != bytes.len() {
            return Err(Error::MalformedV1InstallBlock(format!(
                "central directory record parser consumed {} bytes, expected {}",
                reader.position(),
                bytes.len()
            )));
        }
        Ok(entry)
    }

    fn read_v1_p4k_cd_entry_at(
        cd_data: &[u8],
        offset: usize,
        index: usize,
    ) -> Result<(P4kEntryCompact, usize)> {
        let signature_end = offset.checked_add(4).ok_or_else(|| {
            Error::MalformedV1InstallBlock("central directory offset overflow".to_string())
        })?;
        let header_len = std::mem::size_of::<CentralDirectoryHeader>();
        let header_end = signature_end.checked_add(header_len).ok_or_else(|| {
            Error::MalformedV1InstallBlock("central directory header overflow".to_string())
        })?;
        if header_end > cd_data.len() {
            return Err(Error::MalformedV1InstallBlock(format!(
                "central directory record {index} header extends past CDR size {}",
                cd_data.len()
            )));
        }

        let sig = u32::from_le_bytes(
            cd_data[offset..signature_end]
                .try_into()
                .expect("central directory signature slice length checked"),
        );
        if sig != CentralDirectoryHeader::SIGNATURE {
            return Err(Error::InvalidSignature {
                expected: CentralDirectoryHeader::SIGNATURE,
                actual: sig,
            });
        }

        let header = <CentralDirectoryHeader as zerocopy::FromBytes>::read_from_bytes(
            &cd_data[signature_end..header_end],
        )
        .map_err(|_| {
            Error::MalformedV1InstallBlock(format!(
                "failed to read central directory record {index} header"
            ))
        })?;
        validate_v1_cdr_fixed_header(&header)?;

        let name_len = header.file_name_length as usize;
        let extra_len = header.extra_field_length as usize;
        let name_start = header_end;
        let name_end = name_start.checked_add(name_len).ok_or_else(|| {
            Error::MalformedV1InstallBlock(format!(
                "central directory record {index} name range overflow"
            ))
        })?;
        let extra_start = name_end;
        let extra_end = extra_start.checked_add(extra_len).ok_or_else(|| {
            Error::MalformedV1InstallBlock(format!(
                "central directory record {index} extra range overflow"
            ))
        })?;
        if extra_end > cd_data.len() {
            return Err(Error::MalformedV1InstallBlock(format!(
                "central directory record {index} ends at {extra_end}, past CDR size {}",
                cd_data.len()
            )));
        }

        let name = normalize_p4k_path_bytes(&cd_data[name_start..name_end], false);

        let zip64_end = extra_start + extra_field::ZIP64_TOTAL_SIZE as usize;
        let signature_end = zip64_end + extra_field::P4K_5000_TOTAL_SIZE as usize;
        let encryption_end = signature_end + extra_field::P4K_5002_TOTAL_SIZE as usize;
        let sha256_end = encryption_end + extra_field::P4K_5003_TOTAL_SIZE as usize;
        if sha256_end != extra_end {
            return Err(Error::MalformedV1Entry(format!(
                "central directory record {index} P4K extra span ends at {sha256_end}, expected {extra_end}"
            )));
        }

        let zip64 = <P4kZip64ExtraField as zerocopy::FromBytes>::read_from_bytes(
            &cd_data[extra_start..zip64_end],
        )
        .map_err(|_| {
            Error::MalformedV1Entry(format!(
                "failed to read central directory record {index} ZIP64 extra"
            ))
        })?;
        if zip64.id != extra_field::ZIP64 {
            return Err(Error::InvalidExtraFieldId {
                expected: extra_field::ZIP64,
                actual: zip64.id,
            });
        }
        if zip64.size != extra_field::ZIP64_TOTAL_SIZE {
            return Err(Error::InvalidExtraFieldId {
                expected: extra_field::ZIP64_TOTAL_SIZE,
                actual: zip64.size,
            });
        }
        if zip64.disk_number_start != 0 {
            let disk_number_start = zip64.disk_number_start;
            return Err(Error::MalformedV1Entry(format!(
                "ZIP64 extra disk_number_start {disk_number_start}, expected 0"
            )));
        }

        let field_5000 = <P4kSignatureExtraField as zerocopy::FromBytes>::read_from_bytes(
            &cd_data[zip64_end..signature_end],
        )
        .map_err(|_| {
            Error::MalformedV1Entry(format!(
                "failed to read central directory record {index} signature extra"
            ))
        })?;
        if field_5000.id != extra_field::P4K_5000 {
            return Err(Error::InvalidExtraFieldId {
                expected: extra_field::P4K_5000,
                actual: field_5000.id,
            });
        }
        if field_5000.size != extra_field::P4K_5000_TOTAL_SIZE {
            return Err(Error::InvalidExtraFieldId {
                expected: extra_field::P4K_5000_TOTAL_SIZE,
                actual: field_5000.size,
            });
        }

        let field_5002 = <P4kEncryptionExtraField as zerocopy::FromBytes>::read_from_bytes(
            &cd_data[signature_end..encryption_end],
        )
        .map_err(|_| {
            Error::MalformedV1Entry(format!(
                "failed to read central directory record {index} encryption extra"
            ))
        })?;
        if field_5002.id != extra_field::P4K_5002 {
            return Err(Error::InvalidExtraFieldId {
                expected: extra_field::P4K_5002,
                actual: field_5002.id,
            });
        }
        if field_5002.size != extra_field::P4K_5002_TOTAL_SIZE {
            return Err(Error::InvalidExtraFieldId {
                expected: extra_field::P4K_5002_TOTAL_SIZE,
                actual: field_5002.size,
            });
        }
        let encryption = field_5002.encryption;
        if encryption > 1 {
            return Err(Error::MalformedV1Entry(format!(
                "P4K encryption field {encryption}, expected 0 or 1"
            )));
        }

        let field_5003 = <P4kSha256ExtraField as zerocopy::FromBytes>::read_from_bytes(
            &cd_data[encryption_end..sha256_end],
        )
        .map_err(|_| {
            Error::MalformedV1Entry(format!(
                "failed to read central directory record {index} SHA-256 extra"
            ))
        })?;
        if field_5003.id != extra_field::P4K_5003 {
            return Err(Error::InvalidExtraFieldId {
                expected: extra_field::P4K_5003,
                actual: field_5003.id,
            });
        }
        if field_5003.size != extra_field::P4K_5003_TOTAL_SIZE {
            return Err(Error::InvalidExtraFieldId {
                expected: extra_field::P4K_5003_TOTAL_SIZE,
                actual: field_5003.size,
            });
        }

        let compression_method = CompressionMethod::try_from(header.compression_method)
            .map_err(Error::UnsupportedCompression)?;
        Ok((
            P4kEntryCompact {
                name,
                compressed_size: zip64.compressed_size,
                uncompressed_size: zip64.uncompressed_size,
                compression_method: compression_method as u8,
                flags: if field_5002.encryption == 1 { 1 } else { 0 },
                local_header_offset: zip64.local_header_offset,
                crc32: header.crc32,
                last_mod_file_time: (header.last_modified & 0xFFFF) as u16,
                last_mod_file_date: (header.last_modified >> 16) as u16,
                signature: field_5000.signature,
                sha256: field_5003.sha256,
                bytes_already_written: zip64.compressed_size,
            },
            extra_end,
        ))
    }

    fn read_cd_entry_compact(reader: &mut BinaryReader, is_zip64: bool) -> Result<P4kEntryCompact> {
        // Read signature
        let sig = reader.read_u32()?;
        if sig != CentralDirectoryHeader::SIGNATURE {
            return Err(Error::InvalidSignature {
                expected: CentralDirectoryHeader::SIGNATURE,
                actual: sig,
            });
        }

        let header: CentralDirectoryHeader = reader.read_struct()?;
        if is_zip64 {
            validate_v1_cdr_fixed_header(&header)?;
        }

        // Read name
        let name_bytes = reader.read_bytes(header.file_name_length as usize)?;
        let name = normalize_p4k_path_bytes(name_bytes, false);

        // Initialize values from header (may be overridden by ZIP64)
        let mut compressed_size = header.compressed_size as u64;
        let mut uncompressed_size = header.uncompressed_size as u64;
        let mut local_header_offset = header.local_header_offset as u64;
        let mut is_encrypted = false;
        let mut signature = [0u8; 128];
        let mut sha256 = [0u8; 32];

        // Parse extra fields
        let extra_data = reader.read_bytes(header.extra_field_length as usize)?;
        let mut extra_reader = BinaryReader::new(extra_data);

        if is_zip64 {
            // ZIP64 extra field
            let zip64: P4kZip64ExtraField = extra_reader.read_struct()?;
            if zip64.id != extra_field::ZIP64 {
                return Err(Error::InvalidExtraFieldId {
                    expected: extra_field::ZIP64,
                    actual: zip64.id,
                });
            }
            if zip64.size != extra_field::ZIP64_TOTAL_SIZE {
                return Err(Error::InvalidExtraFieldId {
                    expected: extra_field::ZIP64_TOTAL_SIZE,
                    actual: zip64.size,
                });
            }
            uncompressed_size = zip64.uncompressed_size;
            compressed_size = zip64.compressed_size;
            local_header_offset = zip64.local_header_offset;
            if zip64.disk_number_start != 0 {
                let disk_number_start = zip64.disk_number_start;
                return Err(Error::MalformedV1Entry(format!(
                    "ZIP64 extra disk_number_start {disk_number_start}, expected 0"
                )));
            }

            // P4K custom fields
            let field_5000: P4kSignatureExtraField = extra_reader.read_struct()?;
            if field_5000.id != extra_field::P4K_5000 {
                return Err(Error::InvalidExtraFieldId {
                    expected: extra_field::P4K_5000,
                    actual: field_5000.id,
                });
            }
            if field_5000.size != extra_field::P4K_5000_TOTAL_SIZE {
                return Err(Error::InvalidExtraFieldId {
                    expected: extra_field::P4K_5000_TOTAL_SIZE,
                    actual: field_5000.size,
                });
            }
            signature = field_5000.signature;

            // Encryption flag field
            let field_5002: P4kEncryptionExtraField = extra_reader.read_struct()?;
            if field_5002.id != extra_field::P4K_5002 {
                return Err(Error::InvalidExtraFieldId {
                    expected: extra_field::P4K_5002,
                    actual: field_5002.id,
                });
            }
            if field_5002.size != extra_field::P4K_5002_TOTAL_SIZE {
                return Err(Error::InvalidExtraFieldId {
                    expected: extra_field::P4K_5002_TOTAL_SIZE,
                    actual: field_5002.size,
                });
            }
            let encryption = field_5002.encryption;
            if encryption > 1 {
                return Err(Error::MalformedV1Entry(format!(
                    "P4K encryption field {encryption}, expected 0 or 1"
                )));
            }
            is_encrypted = field_5002.encryption == 1;

            let field_5003: P4kSha256ExtraField = extra_reader.read_struct()?;
            if field_5003.id != extra_field::P4K_5003 {
                return Err(Error::InvalidExtraFieldId {
                    expected: extra_field::P4K_5003,
                    actual: field_5003.id,
                });
            }
            if field_5003.size != extra_field::P4K_5003_TOTAL_SIZE {
                return Err(Error::InvalidExtraFieldId {
                    expected: extra_field::P4K_5003_TOTAL_SIZE,
                    actual: field_5003.size,
                });
            }
            sha256 = field_5003.sha256;
        }

        // Skip file comment
        if header.file_comment_length > 0 {
            reader.read_bytes(header.file_comment_length as usize)?;
        }

        let compression_method = CompressionMethod::try_from(header.compression_method)
            .map_err(Error::UnsupportedCompression)?;
        Ok(P4kEntryCompact {
            name,
            compressed_size,
            uncompressed_size,
            compression_method: compression_method as u8,
            flags: if is_encrypted { 1 } else { 0 },
            local_header_offset,
            crc32: header.crc32,
            last_mod_file_time: (header.last_modified & 0xFFFF) as u16,
            last_mod_file_date: (header.last_modified >> 16) as u16,
            signature,
            sha256,
            bytes_already_written: compressed_size,
        })
    }

    fn apply_v1_install_block(
        data: &[u8],
        central_dir_offset: u64,
        freelist_block_count: u64,
        entries: &mut [P4kEntryCompact],
    ) -> Result<Vec<P4kFreelistBlock>> {
        let entry_install_size = entries.len().checked_mul(12).ok_or_else(|| {
            Error::MalformedV1InstallBlock("install block size overflow".to_string())
        })?;
        let freelist_size = usize::try_from(freelist_block_count)
            .map_err(|_| {
                Error::MalformedV1InstallBlock("freelist block count overflows usize".to_string())
            })?
            .checked_mul(16)
            .ok_or_else(|| {
                Error::MalformedV1InstallBlock("freelist block size overflow".to_string())
            })?;
        let block_size = entry_install_size
            .checked_add(freelist_size)
            .ok_or_else(|| {
                Error::MalformedV1InstallBlock("install block size overflow".to_string())
            })?;
        if block_size == 0 {
            return Ok(Vec::new());
        }

        let central_dir_offset = usize::try_from(central_dir_offset).map_err(|_| {
            Error::MalformedV1InstallBlock("central directory offset overflows usize".to_string())
        })?;
        if central_dir_offset < block_size {
            return Err(Error::MalformedV1InstallBlock(format!(
                "central directory offset {} is before {}-byte install block",
                central_dir_offset, block_size
            )));
        }
        let install_start = central_dir_offset - block_size;
        let install_end = install_start + block_size;
        if install_end > data.len() {
            return Err(Error::MalformedV1InstallBlock(
                "install block extends past file end".to_string(),
            ));
        }

        let install = &data[install_start..install_end];
        for (index, entry) in entries.iter_mut().enumerate() {
            let off = index * 12;
            let value = u64::from_le_bytes(
                install[off..off + 8]
                    .try_into()
                    .expect("install block slice length checked"),
            );
            if value > entry.compressed_size {
                return Err(Error::MalformedV1InstallBlock(format!(
                    "entry {} bytes_already_written {} > compressed_size {}",
                    index, value, entry.compressed_size
                )));
            }
            let reserved = u32::from_le_bytes(
                install[off + 8..off + 12]
                    .try_into()
                    .expect("install block reserved-word slice length checked"),
            );
            if reserved != 0 {
                return Err(Error::MalformedV1InstallBlock(format!(
                    "entry {} install reserved word {reserved:#010x}, expected 0",
                    index
                )));
            }
            entry.bytes_already_written = value;
        }

        let freelist_start = entry_install_size;
        let freelist_end = freelist_start + freelist_size;
        read_freelist_blocks(
            &install[freelist_start..freelist_end],
            usize::try_from(freelist_block_count).map_err(|_| {
                Error::MalformedV1InstallBlock("freelist block count overflows usize".to_string())
            })?,
            Error::MalformedV1InstallBlock,
        )
    }

    fn read_v1_freelist_metadata(
        data: &[u8],
        eocd_offset: usize,
        eocd: &EocdRecord,
    ) -> Result<(u64, Option<u64>)> {
        let comment_start = eocd_offset
            .checked_add(4 + std::mem::size_of::<EocdRecord>())
            .ok_or_else(|| {
                Error::MalformedV1InstallBlock("EOCD comment offset overflow".to_string())
            })?;
        let comment_end = comment_start
            .checked_add(eocd.comment_length as usize)
            .ok_or_else(|| {
                Error::MalformedV1InstallBlock("EOCD comment size overflow".to_string())
            })?;
        if comment_end > data.len() {
            return Err(Error::MalformedV1InstallBlock(
                "EOCD comment extends past file end".to_string(),
            ));
        }

        let comment = &data[comment_start..comment_end];
        if comment.len() < 2 || &comment[..2] != b"CI" {
            return Ok((0, None));
        }
        if comment.len() != 16 {
            return Err(Error::MalformedV1InstallBlock(format!(
                "P4K CI EOCD comment has {} bytes, expected 16",
                comment.len()
            )));
        }

        let marker = u32::from_le_bytes(
            comment[2..6]
                .try_into()
                .expect("CI comment marker slice length checked"),
        );
        if marker != 0x0001_0047 {
            return Err(Error::MalformedV1InstallBlock(format!(
                "P4K CI EOCD marker {marker:#010x}, expected 0x00010047"
            )));
        }

        let sector_size = u16::from_le_bytes(
            comment[6..8]
                .try_into()
                .expect("CI comment sector size slice length checked"),
        );
        if sector_size == 0 || !sector_size.is_power_of_two() {
            return Err(Error::MalformedV1InstallBlock(format!(
                "P4K CI EOCD sector size {sector_size}, expected non-zero power of two"
            )));
        }
        if data.len() % sector_size as usize != 0 {
            return Err(Error::MalformedV1InstallBlock(format!(
                "P4K v1 file size {} is not aligned to CI EOCD sector size {}",
                data.len(),
                sector_size
            )));
        }
        if data[comment_end..].iter().any(|byte| *byte != 0) {
            return Err(Error::MalformedV1InstallBlock(
                "non-zero P4K v1 sector padding after EOCD comment".to_string(),
            ));
        }

        let freelist_count = u64::from_le_bytes(
            comment[8..16]
                .try_into()
                .expect("CI comment freelist count slice length checked"),
        );
        Ok((freelist_count, Some(sector_size as u64)))
    }

    fn deduplicate_v1_entries(
        data: &[u8],
        physical_sector_size: u64,
        entries: &mut Vec<P4kEntryCompact>,
        freelist_blocks: &mut Vec<P4kFreelistBlock>,
    ) -> Result<()> {
        let mut seen = HashSet::with_capacity(entries.len());
        let mut keep_entries = vec![true; entries.len()];
        let mut removed_duplicates = 0usize;
        for read in 0..entries.len() {
            let filename_handle_key = filename_handle_key(&entries[read].name);
            if entries[read].local_header_offset % physical_sector_size != 0 {
                return Err(Error::MalformedV1Entry(format!(
                    "entry {} local header offset {} is not aligned to physical sector size {}",
                    entries[read].name, entries[read].local_header_offset, physical_sector_size
                )));
            }
            if seen.insert(filename_handle_key) {
                continue;
            }

            keep_entries[read] = false;
            removed_duplicates += 1;
            let local_record_size = v1_local_record_size(data, entries[read].local_header_offset)?;
            let free_size = align_up_u64(
                local_record_size
                    .checked_add(entries[read].compressed_size)
                    .ok_or_else(|| {
                        Error::MalformedV1Entry(format!(
                            "duplicate entry {} free span overflow",
                            entries[read].name
                        ))
                    })?,
                physical_sector_size,
            )?;
            freelist_blocks.push(P4kFreelistBlock {
                offset: entries[read].local_header_offset,
                size: free_size,
            });
        }
        drop(seen);
        if removed_duplicates != 0 {
            let mut index = 0usize;
            entries.retain(|_| {
                let keep = keep_entries[index];
                index += 1;
                keep
            });
        }
        merge_freelist_blocks(freelist_blocks)?;
        Ok(())
    }

    /// Find EOCD using SIMD-accelerated signature search.
    fn find_eocd_optimized(data: &[u8], actual_end: usize) -> Result<usize> {
        let search_start = actual_end.saturating_sub(65557);

        simd::find_eocd_signature(data, search_start, actual_end).ok_or(Error::EocdNotFound)
    }

    fn read_zip64_eocd(data: &[u8], eocd_offset: usize) -> Result<(u64, u64, u64)> {
        let locator_size = std::mem::size_of::<Eocd64Locator>() + 4;
        if eocd_offset < locator_size {
            return Err(Error::Zip64EocdNotFound);
        }

        // Search backwards for locator
        let search_start = eocd_offset.saturating_sub(100);
        let mut locator_offset = None;

        for i in (search_start..eocd_offset).rev() {
            if i + 4 <= data.len() && data[i..i + 4] == Eocd64Locator::MAGIC {
                locator_offset = Some(i);
                break;
            }
        }

        let locator_offset = locator_offset.ok_or(Error::Zip64EocdNotFound)?;
        if locator_offset + locator_size != eocd_offset {
            return Err(Error::MalformedV1InstallBlock(format!(
                "ZIP64 locator ends at {}, expected EOCD offset {}",
                locator_offset + locator_size,
                eocd_offset
            )));
        }
        let mut reader = BinaryReader::new(&data[locator_offset..]);

        reader.advance(4);
        let locator: Eocd64Locator = reader.read_struct()?;
        validate_v1_zip64_locator(&locator)?;

        // Read ZIP64 EOCD
        let eocd64_offset = usize::try_from(locator.zip64_eocd_offset).map_err(|_| {
            Error::MalformedV1InstallBlock("ZIP64 EOCD offset overflows usize".to_string())
        })?;
        if eocd64_offset + 4 > data.len() {
            return Err(Error::Zip64EocdNotFound);
        }

        let sig = u32::from_le_bytes([
            data[eocd64_offset],
            data[eocd64_offset + 1],
            data[eocd64_offset + 2],
            data[eocd64_offset + 3],
        ]);

        if sig != Eocd64Record::SIGNATURE {
            return Err(Error::InvalidSignature {
                expected: Eocd64Record::SIGNATURE,
                actual: sig,
            });
        }

        let mut reader = BinaryReader::new(&data[eocd64_offset + 4..]);
        let eocd64: Eocd64Record = reader.read_struct()?;
        validate_v1_zip64_eocd(&eocd64, eocd64_offset, locator_offset)?;

        Ok((
            eocd64.central_dir_count_total,
            eocd64.central_dir_offset,
            eocd64.central_dir_size,
        ))
    }
}

fn read_freelist_blocks<F>(bytes: &[u8], count: usize, error: F) -> Result<Vec<P4kFreelistBlock>>
where
    F: Fn(String) -> Error,
{
    let expected_len = count
        .checked_mul(16)
        .ok_or_else(|| error("freelist byte size overflow".to_string()))?;
    if bytes.len() != expected_len {
        return Err(error(format!(
            "freelist block bytes have {} bytes, expected {}",
            bytes.len(),
            expected_len
        )));
    }

    let mut blocks = Vec::with_capacity(count);
    for index in 0..count {
        let off = index * 16;
        let offset = u64::from_le_bytes(
            bytes[off..off + 8]
                .try_into()
                .expect("freelist offset slice length checked"),
        );
        let size = u64::from_le_bytes(
            bytes[off + 8..off + 16]
                .try_into()
                .expect("freelist size slice length checked"),
        );
        if size == 0 {
            return Err(error(format!("freelist block {index} has zero size")));
        }
        let Some(_) = offset.checked_add(size) else {
            return Err(error(format!(
                "freelist block {index} range {offset:#x}+{size:#x} overflows u64"
            )));
        };
        blocks.push(P4kFreelistBlock { offset, size });
    }

    Ok(blocks)
}

fn merge_freelist_blocks(blocks: &mut Vec<P4kFreelistBlock>) -> Result<()> {
    blocks.retain(|block| block.size != 0);
    blocks.sort_by_key(|block| block.offset);

    let mut write = 0;
    for read in 0..blocks.len() {
        let block = blocks[read];
        if write == 0 {
            blocks[write] = block;
            write += 1;
            continue;
        }

        let prev = &mut blocks[write - 1];
        if prev.offset.checked_add(prev.size) == Some(block.offset) {
            prev.size = prev.size.checked_add(block.size).ok_or_else(|| {
                Error::MalformedV1Entry("merged freelist block size overflow".to_string())
            })?;
        } else {
            blocks[write] = block;
            write += 1;
        }
    }
    blocks.truncate(write);
    Ok(())
}

fn v1_local_record_size(data: &[u8], local_header_offset: u64) -> Result<u64> {
    let offset = usize::try_from(local_header_offset)
        .map_err(|_| Error::MalformedV1Entry("local header offset overflows usize".to_string()))?;
    if offset + 4 > data.len() {
        return Err(Error::MalformedV1Entry(
            "local header offset out of bounds".to_string(),
        ));
    }

    let sig = u32::from_le_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
    ]);
    if sig != LocalFileHeader::SIGNATURE && sig != LocalFileHeader::SIGNATURE_EXTENDED {
        return Err(Error::InvalidSignature {
            expected: LocalFileHeader::SIGNATURE,
            actual: sig,
        });
    }

    let header_start = offset + 4;
    let header_size = std::mem::size_of::<LocalFileHeader>();
    if header_start + header_size > data.len() {
        return Err(Error::MalformedV1Entry(
            "local header out of bounds".to_string(),
        ));
    }

    let mut reader = BinaryReader::new(&data[header_start..]);
    let local_header: LocalFileHeader = reader.read_struct()?;
    let payload_offset = local_payload_offset(offset, header_size, &local_header)?;
    checked_local_record_size(offset, payload_offset)
}

fn checked_local_record_size(local_header_offset: usize, payload_offset: usize) -> Result<u64> {
    let size = payload_offset.checked_sub(local_header_offset).ok_or_else(|| {
        Error::MalformedV1Entry(format!(
            "local payload offset {payload_offset} is before local header offset {local_header_offset}"
        ))
    })?;
    u64::try_from(size)
        .map_err(|_| Error::MalformedV1Entry("local record size overflows u64".to_string()))
}

fn validate_v1_zip64_classic_eocd(eocd: &EocdRecord) -> Result<()> {
    let disk_number = eocd.disk_number;
    let central_dir_disk = eocd.central_dir_disk;
    if disk_number != u16::MAX || central_dir_disk != u16::MAX {
        return Err(Error::MalformedV1InstallBlock(format!(
            "ZIP64 EOCD disk sentinels {disk_number:#06x}/{central_dir_disk:#06x}, expected 0xffff/0xffff"
        )));
    }
    let central_dir_count_disk = eocd.central_dir_count_disk;
    let central_dir_count_total = eocd.central_dir_count_total;
    if central_dir_count_disk != u16::MAX || central_dir_count_total != u16::MAX {
        return Err(Error::MalformedV1InstallBlock(format!(
            "ZIP64 EOCD count sentinels {central_dir_count_disk:#06x}/{central_dir_count_total:#06x}, expected 0xffff/0xffff"
        )));
    }
    let central_dir_size = eocd.central_dir_size;
    let central_dir_offset = eocd.central_dir_offset;
    if central_dir_size != u32::MAX || central_dir_offset != u32::MAX {
        return Err(Error::MalformedV1InstallBlock(format!(
            "ZIP64 EOCD size/offset sentinels {central_dir_size:#010x}/{central_dir_offset:#010x}, expected 0xffffffff/0xffffffff"
        )));
    }
    let comment_length = eocd.comment_length;
    if comment_length != 16 {
        return Err(Error::MalformedV1InstallBlock(format!(
            "ZIP64 EOCD CI comment length {comment_length}, expected 16"
        )));
    }
    Ok(())
}

fn validate_v1_zip64_locator(locator: &Eocd64Locator) -> Result<()> {
    let zip64_eocd_disk = locator.zip64_eocd_disk;
    let total_disks = locator.total_disks;
    if zip64_eocd_disk != 0 || total_disks != 1 {
        return Err(Error::MalformedV1InstallBlock(format!(
            "ZIP64 locator disk fields {zip64_eocd_disk}/{total_disks}, expected 0/1"
        )));
    }
    Ok(())
}

fn validate_v1_zip64_eocd(
    eocd64: &Eocd64Record,
    eocd64_offset: usize,
    locator_offset: usize,
) -> Result<()> {
    let record_size = eocd64.record_size;
    if record_size != 44 {
        return Err(Error::MalformedV1InstallBlock(format!(
            "ZIP64 EOCD record_size {record_size}, expected 44"
        )));
    }
    let version_made_by = eocd64.version_made_by;
    let version_needed = eocd64.version_needed;
    if version_made_by != 46 || version_needed != 45 {
        return Err(Error::MalformedV1InstallBlock(format!(
            "ZIP64 EOCD versions {version_made_by}/{version_needed}, expected 46/45"
        )));
    }
    let disk_number = eocd64.disk_number;
    let central_dir_disk = eocd64.central_dir_disk;
    if disk_number != 0 || central_dir_disk != 0 {
        return Err(Error::MalformedV1InstallBlock(format!(
            "ZIP64 EOCD disk fields {disk_number}/{central_dir_disk}, expected 0/0"
        )));
    }
    let central_dir_count_disk = eocd64.central_dir_count_disk;
    let central_dir_count_total = eocd64.central_dir_count_total;
    if central_dir_count_disk != central_dir_count_total {
        return Err(Error::MalformedV1InstallBlock(format!(
            "ZIP64 EOCD entry counts {central_dir_count_disk}/{central_dir_count_total} do not match"
        )));
    }
    if central_dir_count_total > u32::MAX as u64 {
        return Err(Error::MalformedV1InstallBlock(format!(
            "ZIP64 EOCD entry count {central_dir_count_total} exceeds official v1 loader u32 limit"
        )));
    }
    let central_dir_offset = usize::try_from(eocd64.central_dir_offset).map_err(|_| {
        Error::MalformedV1InstallBlock("ZIP64 EOCD CDR offset overflows usize".to_string())
    })?;
    let central_dir_size = usize::try_from(eocd64.central_dir_size).map_err(|_| {
        Error::MalformedV1InstallBlock("ZIP64 EOCD CDR size overflows usize".to_string())
    })?;
    let central_dir_end = central_dir_offset
        .checked_add(central_dir_size)
        .ok_or_else(|| {
            Error::MalformedV1InstallBlock("ZIP64 EOCD CDR range overflow".to_string())
        })?;
    if central_dir_end != eocd64_offset {
        return Err(Error::MalformedV1InstallBlock(format!(
            "ZIP64 EOCD CDR ends at {central_dir_end}, expected ZIP64 EOCD offset {eocd64_offset}"
        )));
    }
    let eocd64_end = eocd64_offset
        .checked_add(4 + std::mem::size_of::<Eocd64Record>())
        .ok_or_else(|| Error::MalformedV1InstallBlock("ZIP64 EOCD range overflow".to_string()))?;
    if eocd64_end != locator_offset {
        return Err(Error::MalformedV1InstallBlock(format!(
            "ZIP64 EOCD ends at {eocd64_end}, expected locator offset {locator_offset}"
        )));
    }
    Ok(())
}

fn validate_v1_cdr_fixed_header(header: &CentralDirectoryHeader) -> Result<()> {
    let version_made_by = header.version_made_by;
    if version_made_by != 46 {
        return Err(Error::MalformedV1Entry(format!(
            "central directory version_made_by {version_made_by}, expected 46"
        )));
    }
    let version_needed = header.version_needed;
    if version_needed != 45 {
        return Err(Error::MalformedV1Entry(format!(
            "central directory version_needed {version_needed}, expected 45"
        )));
    }
    let flags = header.flags;
    if flags != 0 {
        return Err(Error::MalformedV1Entry(format!(
            "central directory flags {flags}, expected 0"
        )));
    }
    let compressed_size = header.compressed_size;
    let uncompressed_size = header.uncompressed_size;
    if compressed_size != u32::MAX || uncompressed_size != u32::MAX {
        return Err(Error::MalformedV1Entry(format!(
            "central directory size sentinels {compressed_size:#010x}/{uncompressed_size:#010x}, expected 0xffffffff/0xffffffff"
        )));
    }
    let file_name_length = header.file_name_length;
    if file_name_length == 0 {
        return Err(Error::MalformedV1Entry(
            "central directory file_name_length is zero".to_string(),
        ));
    }
    let extra_field_length = header.extra_field_length;
    if extra_field_length != 0xCE {
        return Err(Error::MalformedV1Entry(format!(
            "central directory extra_field_length {extra_field_length:#06x}, expected 0x00ce"
        )));
    }
    let file_comment_length = header.file_comment_length;
    if file_comment_length != 0 {
        return Err(Error::MalformedV1Entry(format!(
            "central directory file_comment_length {file_comment_length}, expected 0"
        )));
    }
    let disk_number_start = header.disk_number_start;
    if disk_number_start != u16::MAX {
        return Err(Error::MalformedV1Entry(format!(
            "central directory disk_number_start {disk_number_start:#06x}, expected 0xffff"
        )));
    }
    let internal_attrs = header.internal_attrs;
    let external_attrs = header.external_attrs;
    if internal_attrs != 0 || external_attrs != 0 {
        return Err(Error::MalformedV1Entry(format!(
            "central directory attributes {internal_attrs:#06x}/{external_attrs:#010x}, expected zero"
        )));
    }
    let local_header_offset = header.local_header_offset;
    if local_header_offset != u32::MAX {
        return Err(Error::MalformedV1Entry(format!(
            "central directory local_header_offset sentinel {local_header_offset:#010x}, expected 0xffffffff"
        )));
    }
    Ok(())
}

fn validate_v1_local_header_metadata(
    local_header: &LocalFileHeader,
    expected_name: &str,
    compression_method: CompressionMethod,
    crc32: u32,
    last_mod_file_time: u16,
    last_mod_file_date: u16,
) -> Result<()> {
    let version_needed = local_header.version_needed;
    if version_needed != 45 {
        return Err(Error::MalformedV1Entry(format!(
            "local header version_needed {version_needed}, expected 45"
        )));
    }
    let flags = local_header.flags;
    if flags != 0 {
        return Err(Error::MalformedV1Entry(format!(
            "local header flags {flags}, expected 0"
        )));
    }
    let local_method = local_header.compression_method;
    if local_method != compression_method as u16 {
        return Err(Error::MalformedV1Entry(format!(
            "local header compression method {local_method}, expected {}",
            compression_method as u16
        )));
    }
    let local_last_mod_file_time = (local_header.last_modified & 0xFFFF) as u16;
    let local_last_mod_file_date = (local_header.last_modified >> 16) as u16;
    if local_last_mod_file_time != last_mod_file_time
        || local_last_mod_file_date != last_mod_file_date
    {
        return Err(Error::MalformedV1Entry(format!(
            "local header DOS timestamp {local_last_mod_file_time:#06x}/{local_last_mod_file_date:#06x}, expected {last_mod_file_time:#06x}/{last_mod_file_date:#06x}"
        )));
    }
    let local_crc32 = local_header.crc32;
    if local_crc32 != crc32 {
        return Err(Error::MalformedV1Entry(format!(
            "local header CRC32 {local_crc32:#010x}, expected {crc32:#010x}"
        )));
    }
    let local_compressed_size = local_header.compressed_size;
    let local_uncompressed_size = local_header.uncompressed_size;
    if local_compressed_size != u32::MAX || local_uncompressed_size != u32::MAX {
        return Err(Error::MalformedV1Entry(format!(
            "local header size sentinels {local_compressed_size:#010x}/{local_uncompressed_size:#010x}, expected 0xffffffff/0xffffffff"
        )));
    }
    let local_name_len = local_header.file_name_length as usize;
    let expected_name_len = expected_name.len();
    if local_name_len != expected_name_len {
        return Err(Error::MalformedV1Entry(format!(
            "local header file_name_length {local_name_len}, expected {expected_name_len}"
        )));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn validate_v1_local_extra_fields(
    data: &[u8],
    local_header_offset: usize,
    header_size_without_signature: usize,
    local_header: &LocalFileHeader,
    expected_name: &str,
    expected_local_header_span: Option<usize>,
    expected_local_header_offset: u64,
    expected_compressed_size: u64,
    expected_uncompressed_size: u64,
) -> Result<()> {
    let name_len = local_header.file_name_length as usize;
    let extra_len = local_header.extra_field_length as usize;
    let header_span_without_local_offset = 4usize
        .checked_add(header_size_without_signature)
        .and_then(|value| value.checked_add(name_len))
        .and_then(|value| value.checked_add(extra_len))
        .ok_or_else(|| Error::MalformedV1Entry("local header span overflow".to_string()))?;
    if let Some(expected) = expected_local_header_span {
        if header_span_without_local_offset != expected {
            return Err(Error::MalformedV1Entry(format!(
                "local header span {header_span_without_local_offset}, expected physical sector size {expected}"
            )));
        }
    }

    let name_start = local_header_offset
        .checked_add(4 + header_size_without_signature)
        .ok_or_else(|| Error::MalformedV1Entry("local file name offset overflow".to_string()))?;
    let name_end = name_start
        .checked_add(name_len)
        .ok_or_else(|| Error::MalformedV1Entry("local file name range overflow".to_string()))?;
    if name_end > data.len() {
        return Err(Error::MalformedV1Entry(
            "local file name extends past file end".to_string(),
        ));
    }
    if &data[name_start..name_end] != expected_name.as_bytes() {
        return Err(Error::MalformedV1Entry(
            "local header file name does not match CDR".to_string(),
        ));
    }
    let extra_start = name_end;
    let extra_end = extra_start
        .checked_add(extra_len)
        .ok_or_else(|| Error::MalformedV1Entry("local extra range overflow".to_string()))?;
    if extra_end > data.len() {
        return Err(Error::MalformedV1Entry(
            "local extra fields extend past file end".to_string(),
        ));
    }
    if extra_len < std::mem::size_of::<P4kZip64ExtraField>() + 4 {
        return Err(Error::MalformedV1Entry(format!(
            "local extra_field_length {extra_len}, expected at least ZIP64 plus dummy header"
        )));
    }

    let extra = &data[extra_start..extra_end];
    let mut reader = BinaryReader::new(extra);
    let zip64: P4kZip64ExtraField = reader.read_struct()?;
    if zip64.id != extra_field::ZIP64 {
        return Err(Error::InvalidExtraFieldId {
            expected: extra_field::ZIP64,
            actual: zip64.id,
        });
    }
    if zip64.size != extra_field::ZIP64_TOTAL_SIZE {
        return Err(Error::InvalidExtraFieldId {
            expected: extra_field::ZIP64_TOTAL_SIZE,
            actual: zip64.size,
        });
    }
    let local_offset_matches =
        zip64.local_header_offset == 0 || zip64.local_header_offset == expected_local_header_offset;
    if zip64.uncompressed_size != expected_uncompressed_size
        || zip64.compressed_size != expected_compressed_size
        || !local_offset_matches
    {
        let uncompressed_size = zip64.uncompressed_size;
        let compressed_size = zip64.compressed_size;
        let local_offset = zip64.local_header_offset;
        return Err(Error::MalformedV1Entry(format!(
            "local ZIP64 metadata {uncompressed_size}/{compressed_size}/{local_offset} does not match CDR {expected_uncompressed_size}/{expected_compressed_size}/{expected_local_header_offset}"
        )));
    }
    if zip64.disk_number_start != 0 {
        let disk_number_start = zip64.disk_number_start;
        return Err(Error::MalformedV1Entry(format!(
            "local ZIP64 disk_number_start {disk_number_start}, expected 0"
        )));
    }

    let dummy_offset = std::mem::size_of::<P4kZip64ExtraField>();
    let dummy_id = u16::from_le_bytes([extra[dummy_offset], extra[dummy_offset + 1]]);
    let dummy_size = u16::from_le_bytes([extra[dummy_offset + 2], extra[dummy_offset + 3]]);
    let expected_dummy_size = extra_len - dummy_offset;
    if dummy_id != 0x0666 {
        return Err(Error::MalformedV1Entry(format!(
            "local dummy extra id {dummy_id:#06x}, expected 0x0666"
        )));
    }
    if dummy_size as usize != expected_dummy_size {
        return Err(Error::MalformedV1Entry(format!(
            "local dummy extra size {dummy_size}, expected {expected_dummy_size}"
        )));
    }
    Ok(())
}

fn select_v1_payload_offset(
    data: &[u8],
    local_header_offset: usize,
    header_size_without_signature: usize,
    local_header: &LocalFileHeader,
    compressed_size: u64,
    expected_sha256: [u8; 32],
) -> Result<(usize, bool)> {
    let zip_payload_offset = zip_local_payload_offset(
        local_header_offset,
        header_size_without_signature,
        local_header,
    )?;
    let aligned_payload_offset = local_payload_offset(
        local_header_offset,
        header_size_without_signature,
        local_header,
    )?;
    if compressed_size == 0 {
        return Ok((aligned_payload_offset, false));
    }

    let expected_sha256_is_set = expected_sha256.iter().any(|byte| *byte != 0);
    if expected_sha256_is_set {
        let compressed_len = usize::try_from(compressed_size).map_err(|_| {
            Error::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "compressed size overflows usize",
            ))
        })?;
        // Retail v1 archives commonly store payload bytes immediately
        // after the advertised ZIP local header data. Try that placement
        // first so raw SHA verification does not hash a whole wrong
        // aligned payload candidate before the real one.
        for candidate in [zip_payload_offset, aligned_payload_offset] {
            let Some(end) = candidate.checked_add(compressed_len) else {
                continue;
            };
            if end > data.len() {
                continue;
            }
            let actual: [u8; 32] = Sha256::digest(&data[candidate..end]).into();
            if actual == expected_sha256 {
                return Ok((candidate, true));
            }
        }
    }

    Ok((aligned_payload_offset, false))
}

fn zip_local_payload_offset(
    local_header_offset: usize,
    header_size_without_signature: usize,
    local_header: &LocalFileHeader,
) -> Result<usize> {
    local_header_offset
        .checked_add(4 + header_size_without_signature + local_header.variable_data_size())
        .ok_or_else(|| {
            Error::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "local payload offset overflow",
            ))
        })
}

fn local_payload_offset(
    local_header_offset: usize,
    header_size_without_signature: usize,
    local_header: &LocalFileHeader,
) -> Result<usize> {
    if local_header.compressed_size == u32::MAX
        && local_header.uncompressed_size == u32::MAX
        && local_header.extra_field_length as usize >= 0x24
    {
        let sector_size = 4
            + header_size_without_signature
            + local_header.file_name_length as usize
            + local_header.extra_field_length as usize;
        if sector_size.is_power_of_two() {
            let record_size = align_up_usize(
                local_header.file_name_length as usize + sector_size + 0x3D,
                sector_size,
            )?;
            return local_header_offset.checked_add(record_size).ok_or_else(|| {
                Error::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "P4K v1 local record offset overflow",
                ))
            });
        }
    }

    zip_local_payload_offset(
        local_header_offset,
        header_size_without_signature,
        local_header,
    )
}

fn normalize_p4k_path(path: &str, lowercase: bool) -> String {
    normalize_p4k_path_bytes(path.as_bytes(), lowercase)
}

fn filename_handle_key(name: &str) -> Cow<'_, str> {
    if p4k_path_bytes_need_normalization(name.as_bytes(), true) {
        Cow::Owned(normalize_p4k_path(name, true))
    } else {
        Cow::Borrowed(name)
    }
}

fn p4k_path_bytes_need_normalization(path: &[u8], lowercase: bool) -> bool {
    if path.last() == Some(&b' ') {
        return true;
    }

    let mut previous = None;
    for (index, &byte) in path.iter().enumerate() {
        if byte == b'\\'
            || previous == Some(b'/') && byte == b'/'
            || previous == Some(b'.') && byte == b'/'
            || lowercase && byte.is_ascii_uppercase()
        {
            return true;
        }
        if byte == b'.'
            && index >= 2
            && path.get(index - 2..=index) == Some(b"/..".as_slice())
            && path.get(index + 1) == Some(&b'/')
        {
            return true;
        }
        previous = Some(byte);
    }

    false
}

fn normalize_p4k_path_bytes(path: &[u8], lowercase: bool) -> String {
    let mut out = Vec::with_capacity(path.len());
    for &byte in path {
        let mut value = if byte == b'\\' { b'/' } else { byte };
        if out.len() > 2 && out.ends_with(b"/..") && value == b'/' {
            out.truncate(out.len() - 3);
            while out.last().is_some_and(|last| *last != b'/') {
                out.pop();
            }
            continue;
        }

        match out.last().copied() {
            Some(b'.') if value == b'/' => {
                out.pop();
                continue;
            }
            Some(b'/') if value == b'/' => continue,
            _ => {}
        }

        if lowercase && value.is_ascii_uppercase() {
            value = value.to_ascii_lowercase();
        }
        out.push(value);
    }

    while out.last() == Some(&b' ') {
        out.pop();
    }

    String::from_utf8_lossy(&out).into_owned()
}

fn validate_crc32_value(actual: u32, expected: u32) -> Result<()> {
    if actual != expected {
        return Err(Error::CrcMismatch { expected, actual });
    }
    Ok(())
}

fn decode_payload_to_vec(
    payload: &[u8],
    method: CompressionMethod,
    uncompressed_size: u64,
    encrypted: bool,
) -> Result<Vec<u8>> {
    let expected_size = usize::try_from(uncompressed_size).map_err(|_| {
        Error::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "uncompressed size overflows usize",
        ))
    })?;

    if method == CompressionMethod::Store && !encrypted && payload.len() != expected_size {
        return Err(Error::Decompression(format!(
            "stored entry size mismatch: expected {}, got {}",
            expected_size,
            payload.len()
        )));
    }

    if encrypted && payload.len() % 16 != 0 {
        return Err(Error::Decryption(
            "data length must be a multiple of 16 bytes".to_string(),
        ));
    }

    let mut output = Vec::with_capacity(expected_size);
    if encrypted {
        let reader = crypto::DecryptReader::new(payload);
        decompress::decode_reader_to_writer(reader, method, expected_size, &mut output)?;
    } else {
        decompress::decode_to_writer(payload, method, expected_size, &mut output)?;
    }
    Ok(output)
}

#[derive(Debug, Default)]
struct Crc32Sink {
    crc32: u32,
    len: u64,
}

impl Crc32Sink {
    fn new() -> Self {
        Self::default()
    }

    fn finish(self) -> u32 {
        self.crc32
    }
}

impl Write for Crc32Sink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.crc32 = svarog_common::crc::hash_bytes_with_seed(buf, self.crc32);
        self.len = self.len.checked_add(buf.len() as u64).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "decoded payload too large",
            )
        })?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn validate_sha256(data: &[u8], expected: [u8; 32]) -> Result<()> {
    let actual: [u8; 32] = Sha256::digest(data).into();
    if actual != expected {
        return Err(Error::Sha256Mismatch { expected, actual });
    }
    Ok(())
}

fn detect_v1_payload_placement(
    data: &[u8],
    version: P4kVersion,
    entries: &[P4kEntryCompact],
    sector_size: Option<u64>,
) -> V1PayloadPlacement {
    const SAMPLE_MAX_ENTRIES: usize = 128;
    const SAMPLE_MAX_BYTES: u64 = 64 * 1024 * 1024;
    const SAMPLE_MAX_ENTRY_BYTES: u64 = 16 * 1024 * 1024;
    const SAMPLE_CONFIDENT_ENTRIES: usize = 16;

    if version != P4kVersion::V1 {
        return V1PayloadPlacement::Unknown;
    }
    let Some(sector_size) = sector_size.filter(|value| value.is_power_of_two()) else {
        return V1PayloadPlacement::Unknown;
    };

    let mut detected = V1PayloadPlacement::Unknown;
    let mut sampled_entries = 0usize;
    let mut sampled_bytes = 0u64;
    for entry in entries {
        if entry.compressed_size == 0
            || entry.compressed_size > SAMPLE_MAX_ENTRY_BYTES
            || entry.sha256.iter().all(|byte| *byte == 0)
        {
            continue;
        }

        let Some((zip_offset, aligned_offset)) =
            v1_payload_candidate_offsets(data, entry, sector_size)
        else {
            continue;
        };

        let Some(placement) = detect_v1_entry_payload_placement(
            data,
            entry,
            sector_size,
            zip_offset,
            aligned_offset,
            detected,
        ) else {
            continue;
        };

        if detected == V1PayloadPlacement::Unknown {
            detected = placement;
        } else if let Some(merged) = merge_v1_payload_placement(detected, placement) {
            detected = merged;
        } else {
            return V1PayloadPlacement::Unknown;
        }

        sampled_entries += 1;
        sampled_bytes = sampled_bytes.saturating_add(entry.compressed_size);
        if sampled_entries >= SAMPLE_MAX_ENTRIES
            || sampled_bytes >= SAMPLE_MAX_BYTES
            || sampled_entries >= SAMPLE_CONFIDENT_ENTRIES
        {
            break;
        }
    }

    detected
}

fn detect_v1_entry_payload_placement(
    data: &[u8],
    entry: &P4kEntryCompact,
    sector_size: u64,
    zip_offset: u64,
    aligned_offset: u64,
    preferred: V1PayloadPlacement,
) -> Option<V1PayloadPlacement> {
    let probe_zip = || {
        payload_sha256_matches(data, zip_offset, entry.compressed_size, entry.sha256)
            .then(|| zip_local_v1_placement(entry, zip_offset, sector_size))
    };
    let probe_aligned = || {
        (zip_offset != aligned_offset
            && payload_sha256_matches(data, aligned_offset, entry.compressed_size, entry.sha256))
        .then_some(V1PayloadPlacement::AlignedRecord)
    };

    match preferred {
        V1PayloadPlacement::AlignedRecord => probe_aligned().or_else(probe_zip),
        V1PayloadPlacement::ZipLocal | V1PayloadPlacement::ZipLocalSector => {
            probe_zip().or_else(probe_aligned)
        }
        V1PayloadPlacement::Unknown => probe_zip().or_else(probe_aligned),
    }
}

fn v1_payload_candidate_offsets(
    data: &[u8],
    entry: &P4kEntryCompact,
    sector_size: u64,
) -> Option<(u64, u64)> {
    let local_header_offset = usize::try_from(entry.local_header_offset).ok()?;
    let header_size = std::mem::size_of::<LocalFileHeader>();
    let signature_end = local_header_offset.checked_add(4)?;
    let header_end = signature_end.checked_add(header_size)?;
    if header_end > data.len() {
        return None;
    }

    let sig = u32::from_le_bytes(data[local_header_offset..signature_end].try_into().ok()?);
    if sig != LocalFileHeader::SIGNATURE && sig != LocalFileHeader::SIGNATURE_EXTENDED {
        return None;
    }
    let local_header =
        <LocalFileHeader as zerocopy::FromBytes>::read_from_bytes(&data[signature_end..header_end])
            .ok()?;
    let variable_end = header_end.checked_add(local_header.variable_data_size())?;
    if variable_end > data.len() {
        return None;
    }

    let zip_offset = zip_local_payload_offset(local_header_offset, header_size, &local_header)
        .ok()
        .and_then(|offset| u64::try_from(offset).ok())?;
    let aligned_offset = local_payload_offset(local_header_offset, header_size, &local_header)
        .ok()
        .and_then(|offset| u64::try_from(offset).ok())
        .or_else(|| {
            v1_local_record_size_from_name_len(entry.name.len() as u64, sector_size)
                .and_then(|record_size| entry.local_header_offset.checked_add(record_size))
        })?;
    Some((zip_offset, aligned_offset))
}

fn zip_local_v1_placement(
    entry: &P4kEntryCompact,
    zip_offset: u64,
    sector_size: u64,
) -> V1PayloadPlacement {
    if entry
        .local_header_offset
        .checked_add(sector_size)
        .is_some_and(|sector_offset| sector_offset == zip_offset)
    {
        V1PayloadPlacement::ZipLocalSector
    } else {
        V1PayloadPlacement::ZipLocal
    }
}

fn merge_v1_payload_placement(
    current: V1PayloadPlacement,
    next: V1PayloadPlacement,
) -> Option<V1PayloadPlacement> {
    match (current, next) {
        (a, b) if a == b => Some(a),
        (V1PayloadPlacement::ZipLocal, V1PayloadPlacement::ZipLocalSector)
        | (V1PayloadPlacement::ZipLocalSector, V1PayloadPlacement::ZipLocal) => {
            Some(V1PayloadPlacement::ZipLocal)
        }
        _ => None,
    }
}

fn payload_sha256_matches(data: &[u8], offset: u64, size: u64, expected: [u8; 32]) -> bool {
    let Ok(start) = usize::try_from(offset) else {
        return false;
    };
    let Ok(len) = usize::try_from(size) else {
        return false;
    };
    let Some(end) = start.checked_add(len) else {
        return false;
    };
    if end > data.len() {
        return false;
    }
    let actual: [u8; 32] = Sha256::digest(&data[start..end]).into();
    actual == expected
}

fn v1_local_record_size_from_name_len(name_len: u64, sector_size: u64) -> Option<u64> {
    if sector_size == 0 || !sector_size.is_power_of_two() {
        return None;
    }
    name_len
        .checked_add(sector_size)?
        .checked_add(0x3D)
        .and_then(|value| value.checked_add(sector_size - 1))
        .map(|value| value & !(sector_size - 1))
}

fn align_up_usize(value: usize, align: usize) -> Result<usize> {
    debug_assert!(align.is_power_of_two());
    value
        .checked_add(align - 1)
        .map(|v| v & !(align - 1))
        .ok_or_else(|| {
            Error::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "alignment overflow",
            ))
        })
}

fn align_up_u64(value: u64, align: u64) -> Result<u64> {
    debug_assert!(align.is_power_of_two());
    value
        .checked_add(align - 1)
        .map(|v| v & !(align - 1))
        .ok_or_else(|| {
            Error::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "alignment overflow",
            ))
        })
}

impl std::fmt::Debug for P4kArchive {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("P4kArchive")
            .field("name", &self.name)
            .field("version", &self.version)
            .field("entries", &self.entries.len())
            .finish()
    }
}

// Legacy compatibility - re-export P4kEntry for backward compat
pub use crate::entry::P4kEntry;

impl P4kArchive {
    /// Get entries in legacy format (allocates new Vec).
    /// Prefer using `iter()` for zero-copy access.
    pub fn entries(&self) -> Vec<P4kEntry> {
        self.iter()
            .map(|e| {
                P4kEntry::new(
                    e.name.to_string(),
                    e.compressed_size,
                    e.uncompressed_size,
                    e.compression_method,
                    e.is_encrypted,
                    e.local_header_offset,
                    ((e.last_mod_file_date as u32) << 16) | e.last_mod_file_time as u32,
                    e.crc32,
                )
            })
            .collect()
    }

    /// Legacy read method. Dispatches v1/v2 based on detected version.
    pub fn read_entry(&self, entry: &P4kEntry) -> Result<Vec<u8>> {
        match self.version {
            P4kVersion::V1 => self.read_by_offset(
                entry.name(),
                entry.local_header_offset(),
                entry.compressed_size(),
                entry.uncompressed_size(),
                entry.compression_method(),
                entry.is_encrypted(),
                entry.crc32(),
                entry.last_mod_file_time(),
                entry.last_mod_file_date(),
                [0; 32],
            ),
            P4kVersion::V2 => self.read_by_data_offset(
                entry.local_header_offset(),
                entry.compressed_size(),
                entry.uncompressed_size(),
                entry.compression_method(),
                entry.is_encrypted(),
                entry.crc32(),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::writer::{P4kBuilder, P4kWriterOptions};
    #[cfg(feature = "parallel")]
    use std::io::Cursor;
    use zerocopy::IntoBytes;

    #[test]
    fn archive_freelist_merge_rejects_merged_size_overflow() {
        let mut blocks = vec![
            P4kFreelistBlock {
                offset: 0,
                size: u64::MAX,
            },
            P4kFreelistBlock {
                offset: u64::MAX,
                size: 1,
            },
        ];

        let err = merge_freelist_blocks(&mut blocks).unwrap_err();
        assert!(err
            .to_string()
            .contains("merged freelist block size overflow"));
    }

    #[test]
    fn local_record_size_rejects_payload_before_header() {
        let err = checked_local_record_size(128, 64).unwrap_err();
        assert!(err
            .to_string()
            .contains("payload offset 64 is before local header offset 128"));
    }

    #[test]
    fn byte_path_normalization_matches_string_path_normalization() {
        for raw in [
            b"Dir\\./Sub//Leaf.txt   ".as_slice(),
            b"Dir/Sub/../Leaf.txt".as_slice(),
            b"Dir\\Mixed/Case.txt".as_slice(),
            b"../escape.bin".as_slice(),
            b"Dir/\xffbad/Leaf.txt ".as_slice(),
        ] {
            let lossy = String::from_utf8_lossy(raw);
            assert_eq!(
                normalize_p4k_path_bytes(raw, false),
                normalize_p4k_path(&lossy, false)
            );
            assert_eq!(
                normalize_p4k_path_bytes(raw, true),
                normalize_p4k_path(&lossy, true)
            );
        }
    }

    #[test]
    fn filename_handle_key_borrows_already_normalized_lowercase_names() {
        match filename_handle_key("data/path/file.bin") {
            Cow::Borrowed(value) => assert_eq!(value, "data/path/file.bin"),
            Cow::Owned(value) => panic!("expected borrowed key, got owned {value}"),
        }

        for name in [
            "Data/path/file.bin",
            "data\\path/file.bin",
            "data/path/../file.bin",
            "data/path/file.bin ",
        ] {
            assert_eq!(
                filename_handle_key(name).as_ref(),
                normalize_p4k_path(name, true)
            );
            assert!(matches!(filename_handle_key(name), Cow::Owned(_)));
        }
    }

    #[test]
    fn v2_name_normalization_check_accepts_packed_normalized_names_without_rewrite() {
        assert!(!p4k_path_bytes_need_normalization(
            b"Data/Path/File.bin",
            false
        ));
        for raw in [
            b"Data\\Path/File.bin".as_slice(),
            b"Data//Path/File.bin".as_slice(),
            b"Data/./File.bin".as_slice(),
            b"Data/Path/../File.bin".as_slice(),
            b"Data/Path/File.bin ".as_slice(),
        ] {
            assert!(p4k_path_bytes_need_normalization(raw, false));
        }
    }

    #[test]
    fn v2_name_offset_reader_uses_packed_field_bytes() {
        let mut cdr = vec![0u8; CDR_V2_ENTRY_SIZE * 2];
        cdr[CDR_V2_OFFSET_TO_FILENAME_OFFSET..CDR_V2_OFFSET_TO_FILENAME_OFFSET + 8]
            .copy_from_slice(&17u64.to_le_bytes());
        let second = CDR_V2_ENTRY_SIZE + CDR_V2_OFFSET_TO_FILENAME_OFFSET;
        cdr[second..second + 8].copy_from_slice(&1234u64.to_le_bytes());

        assert_eq!(P4kArchive::read_v2_name_offset(&cdr, 0).unwrap(), 17);
        assert_eq!(P4kArchive::read_v2_name_offset(&cdr, 1).unwrap(), 1234);

        let err = P4kArchive::read_v2_name_offset(&cdr[..second + 7], 1).unwrap_err();
        assert!(err
            .to_string()
            .contains("CDR name-offset field extends past CDR size"));
    }

    #[test]
    fn detect_v1_payload_placement_uses_actual_zip_local_header_span() {
        let name = "Data/file.bin";
        let payload = b"zip-local-payload";
        let (data, entry) = test_v1_detection_entry(name, payload, 0, false);

        assert_eq!(
            detect_v1_payload_placement(&data, P4kVersion::V1, &[entry], Some(4096)),
            V1PayloadPlacement::ZipLocal
        );
    }

    #[test]
    fn detect_v1_payload_placement_recognizes_sector_zip_local_fast_path() {
        let name = "Data/retail-sector-local.bin";
        let payload = b"retail payload after one local-header sector";
        let sector_size = 4096;
        let (data, entry) = test_v1_sector_zip_local_entry(name, payload, sector_size);

        assert_eq!(
            detect_v1_payload_placement(
                &data,
                P4kVersion::V1,
                std::slice::from_ref(&entry),
                Some(sector_size)
            ),
            V1PayloadPlacement::ZipLocalSector
        );

        let path = temp_archive_path("svarog_p4k_known_zip_local_sector_offset.p4k");
        std::fs::write(&path, &data).unwrap();
        let file = File::open(&path).unwrap();
        let mmap = unsafe { Mmap::map(&file).unwrap() };
        let archive = test_v1_archive_with_placement(
            mmap,
            data.len() as u64,
            entry,
            sector_size,
            V1PayloadPlacement::ZipLocalSector,
        );

        assert_eq!(
            archive.v1_known_payload_offset(&archive.entries[0]),
            Some(sector_size)
        );
        assert_eq!(
            archive
                .v1_known_payload_slice(&archive.entries[0])
                .unwrap()
                .unwrap(),
            payload
        );

        drop(archive);
        drop(file);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn known_v1_zip_local_payload_offset_uses_actual_header_span() {
        let name = "Data/retail-local.bin";
        let payload = b"retail zip-local payload";
        let (data, entry) = test_v1_detection_entry(name, payload, 0, false);
        let expected_payload_offset = 4 + std::mem::size_of::<LocalFileHeader>() + name.len();
        assert_ne!(expected_payload_offset as u64, 4096);

        let path = temp_archive_path("svarog_p4k_known_zip_local_offset.p4k");
        std::fs::write(&path, &data).unwrap();
        let file = File::open(&path).unwrap();
        let mmap = unsafe { Mmap::map(&file).unwrap() };
        let archive = test_v1_archive_with_placement(
            mmap,
            data.len() as u64,
            entry,
            4096,
            V1PayloadPlacement::ZipLocal,
        );

        assert_eq!(
            archive.v1_known_payload_offset(&archive.entries[0]),
            Some(expected_payload_offset as u64)
        );
        assert_eq!(
            archive
                .v1_known_payload_slice(&archive.entries[0])
                .unwrap()
                .unwrap(),
            payload
        );

        drop(archive);
        drop(file);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn known_payload_offset_is_none_when_payload_range_is_out_of_bounds() {
        let name = "Data/truncated.bin";
        let payload = b"declared payload";
        let (mut data, mut entry) = test_v1_detection_entry(name, payload, 0, false);
        entry.compressed_size = payload.len() as u64 + 1;
        data.pop();

        let path = temp_archive_path("svarog_p4k_known_offset_truncated_payload.p4k");
        std::fs::write(&path, &data).unwrap();
        let file = File::open(&path).unwrap();
        let mmap = unsafe { Mmap::map(&file).unwrap() };
        let archive = test_v1_archive_with_placement(
            mmap,
            data.len() as u64,
            entry,
            4096,
            V1PayloadPlacement::ZipLocal,
        );
        let entry = archive.entry_ref(&archive.entries[0]);

        assert_eq!(archive.known_payload_offset(&entry), None);

        drop(archive);
        drop(file);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn payload_offset_resolves_v1_unknown_placement_with_sha256_fallback() {
        let path = temp_archive_path("svarog_p4k_payload_offset_unknown_v1.p4k");
        let payload = b"payload selected by raw sha";
        let mut builder = P4kBuilder::with_options(P4kWriterOptions {
            compression: CompressionMethod::Store,
            ..Default::default()
        });
        builder
            .add_bytes("Data/unknown-placement.bin", payload)
            .unwrap();
        builder.write_v1_to_file(&path).unwrap();

        let mut archive = P4kArchive::open(&path).unwrap();
        let expected_payload_offset = archive.known_payload_offset(&archive.get(0).unwrap());
        archive.v1_payload_placement = V1PayloadPlacement::Unknown;
        let entry = archive.entry_ref(&archive.entries[0]);

        assert_eq!(archive.known_payload_offset(&entry), None);
        assert_eq!(
            archive.payload_offset(&entry).unwrap(),
            expected_payload_offset.unwrap()
        );
        assert_eq!(archive.read(&entry).unwrap(), payload);

        drop(archive);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn integrity_verifier_uses_physical_payload_order() {
        let path = temp_archive_path("svarog_p4k_integrity_verify_physical_order.p4k");
        let mut builder = P4kBuilder::with_options(P4kWriterOptions {
            compression: CompressionMethod::Store,
            ..Default::default()
        });
        builder.add_bytes("Data/early.bin", b"early").unwrap();
        builder.add_bytes("Data/late.bin", b"late").unwrap();
        builder.write_v1_to_file(&path).unwrap();

        let mut archive = P4kArchive::open(&path).unwrap();
        archive.entries.reverse();

        let mut order = Vec::new();
        archive
            .verify_integrity_with_progress(|index, name| {
                order.push((index, name.to_string()));
            })
            .unwrap();

        assert_eq!(
            order,
            vec![
                (1, "Data/early.bin".to_string()),
                (0, "Data/late.bin".to_string())
            ]
        );

        drop(archive);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn detect_v1_payload_placement_uses_actual_aligned_record_span() {
        let name = "Data/file.bin";
        let payload = b"aligned-record-payload";
        let (data, entry) = test_v1_detection_entry(name, payload, 4096, true);

        assert_eq!(
            detect_v1_payload_placement(&data, P4kVersion::V1, &[entry], Some(4096)),
            V1PayloadPlacement::AlignedRecord
        );
    }

    #[test]
    fn detect_v1_payload_placement_stops_after_confident_samples() {
        let sector_size = 4096usize;
        let mut data = Vec::new();
        let mut entries = Vec::new();

        for index in 0..16 {
            let name = format!("Data/sector-{index:02}.bin");
            let payload = format!("sector payload {index:02}");
            let (chunk, mut entry) =
                test_v1_sector_zip_local_entry(&name, payload.as_bytes(), sector_size as u64);
            let offset = align_test_data(&mut data, sector_size);
            entry.local_header_offset = offset as u64;
            data.extend_from_slice(&chunk);
            entries.push(entry);
        }

        let (chunk, mut conflicting) =
            test_v1_detection_entry("Data/conflict.bin", b"aligned after confidence", 4096, true);
        let offset = align_test_data(&mut data, sector_size);
        conflicting.local_header_offset = offset as u64;
        data.extend_from_slice(&chunk);
        entries.push(conflicting);

        assert_eq!(
            detect_v1_payload_placement(&data, P4kVersion::V1, &entries, Some(4096)),
            V1PayloadPlacement::ZipLocalSector
        );
    }

    #[test]
    fn detect_v1_payload_placement_checks_alternate_candidate_on_early_mismatch() {
        let sector_size = 4096usize;
        let mut data = Vec::new();
        let mut entries = Vec::new();

        let (chunk, mut sector_entry) =
            test_v1_sector_zip_local_entry("Data/sector.bin", b"sector", sector_size as u64);
        let offset = align_test_data(&mut data, sector_size);
        sector_entry.local_header_offset = offset as u64;
        data.extend_from_slice(&chunk);
        entries.push(sector_entry);

        let (chunk, mut aligned_entry) =
            test_v1_detection_entry("Data/aligned.bin", b"aligned", sector_size, true);
        let offset = align_test_data(&mut data, sector_size);
        aligned_entry.local_header_offset = offset as u64;
        data.extend_from_slice(&chunk);
        entries.push(aligned_entry);

        assert_eq!(
            detect_v1_payload_placement(&data, P4kVersion::V1, &entries, Some(4096)),
            V1PayloadPlacement::Unknown
        );
    }

    fn test_v1_detection_entry(
        name: &str,
        payload: &[u8],
        sector_size: usize,
        aligned_record: bool,
    ) -> (Vec<u8>, P4kEntryCompact) {
        let local_header_size = 4 + std::mem::size_of::<LocalFileHeader>();
        let extra_len = if aligned_record {
            sector_size - local_header_size - name.len()
        } else {
            sector_size
        };
        let local_header = LocalFileHeader {
            version_needed: 45,
            flags: 0,
            compression_method: CompressionMethod::Store as u16,
            last_modified: 0,
            crc32: svarog_common::crc::hash_bytes(payload),
            compressed_size: u32::MAX,
            uncompressed_size: u32::MAX,
            file_name_length: name.len() as u16,
            extra_field_length: extra_len as u16,
        };

        let mut data = Vec::new();
        data.extend_from_slice(&LocalFileHeader::SIGNATURE.to_le_bytes());
        data.extend_from_slice(local_header.as_bytes());
        data.extend_from_slice(name.as_bytes());
        data.resize(local_header_size + name.len() + extra_len, 0);
        let payload_offset = if aligned_record {
            v1_local_record_size_from_name_len(name.len() as u64, sector_size as u64).unwrap()
                as usize
        } else {
            data.len()
        };
        data.resize(payload_offset, 0);
        data.extend_from_slice(payload);

        (
            data,
            P4kEntryCompact {
                name: name.to_string(),
                compressed_size: payload.len() as u64,
                uncompressed_size: payload.len() as u64,
                compression_method: CompressionMethod::Store as u8,
                flags: 0,
                local_header_offset: 0,
                crc32: svarog_common::crc::hash_bytes(payload),
                last_mod_file_time: 0,
                last_mod_file_date: 0,
                signature: [0; 128],
                sha256: Sha256::digest(payload).into(),
                bytes_already_written: payload.len() as u64,
            },
        )
    }

    fn test_v1_sector_zip_local_entry(
        name: &str,
        payload: &[u8],
        sector_size: u64,
    ) -> (Vec<u8>, P4kEntryCompact) {
        let local_header_size = 4 + std::mem::size_of::<LocalFileHeader>();
        let extra_len = sector_size as usize - local_header_size - name.len();
        let local_header = LocalFileHeader {
            version_needed: 45,
            flags: 0,
            compression_method: CompressionMethod::Store as u16,
            last_modified: 0,
            crc32: svarog_common::crc::hash_bytes(payload),
            compressed_size: u32::MAX,
            uncompressed_size: u32::MAX,
            file_name_length: name.len() as u16,
            extra_field_length: extra_len as u16,
        };

        let mut data = Vec::new();
        data.extend_from_slice(&LocalFileHeader::SIGNATURE.to_le_bytes());
        data.extend_from_slice(local_header.as_bytes());
        data.extend_from_slice(name.as_bytes());
        data.resize(sector_size as usize, 0);
        data.extend_from_slice(payload);

        (
            data,
            P4kEntryCompact {
                name: name.to_string(),
                compressed_size: payload.len() as u64,
                uncompressed_size: payload.len() as u64,
                compression_method: CompressionMethod::Store as u8,
                flags: 0,
                local_header_offset: 0,
                crc32: svarog_common::crc::hash_bytes(payload),
                last_mod_file_time: 0,
                last_mod_file_date: 0,
                signature: [0; 128],
                sha256: Sha256::digest(payload).into(),
                bytes_already_written: payload.len() as u64,
            },
        )
    }

    fn test_v1_archive_with_placement(
        mmap: Mmap,
        file_size: u64,
        entry: P4kEntryCompact,
        sector_size: u64,
        placement: V1PayloadPlacement,
    ) -> P4kArchive {
        test_v1_archive_with_entries(mmap, file_size, vec![entry], sector_size, placement)
    }

    fn test_v1_archive_with_entries(
        mmap: Mmap,
        file_size: u64,
        entries: Vec<P4kEntryCompact>,
        sector_size: u64,
        placement: V1PayloadPlacement,
    ) -> P4kArchive {
        P4kArchive {
            mmap,
            name: "test.p4k".to_string(),
            version: P4kVersion::V1,
            entries,
            freelist_blocks: Vec::new(),
            layout: P4kArchiveLayout {
                file_size,
                actual_content_end: file_size,
                physical_sector_size: Some(sector_size),
                cdr_offset: 0,
                cdr_size: 0,
                name_table_offset: None,
                name_table_size: 0,
                end_of_payload: None,
                install_block_offset: None,
                install_block_size: None,
                eocd_offset: 0,
                manifest_sha256: None,
            },
            v1_payload_placement: placement,
        }
    }

    fn temp_archive_path(name: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!("{}-{}-{nanos}", name, std::process::id()))
    }

    fn align_test_data(data: &mut Vec<u8>, alignment: usize) -> usize {
        let aligned_len = (data.len() + alignment - 1) & !(alignment - 1);
        data.resize(aligned_len, 0);
        aligned_len
    }

    #[cfg(feature = "parallel")]
    #[test]
    fn parallel_v1_cdr_parse_matches_sequential_parse() {
        let mut cd = Vec::new();
        cd.extend_from_slice(&test_v1_cdr_record(
            "Data/Beta.bin",
            11,
            22,
            0x1000,
            0x1122_3344,
            0xAA,
            0xBB,
        ));
        cd.extend_from_slice(&test_v1_cdr_record(
            "Data/Alpha.bin",
            33,
            44,
            0x3000,
            0x5566_7788,
            0xCC,
            0xDD,
        ));

        let mut sequential = Vec::new();
        let sequential_len =
            P4kArchive::parse_entries_sequential(&cd, 2, true, &mut sequential).unwrap();
        let mut parallel = Vec::new();
        let parallel_len = P4kArchive::parse_entries_parallel(&cd, 2, true, &mut parallel).unwrap();

        assert_eq!(parallel_len, sequential_len);
        assert_eq!(parallel_len, cd.len());
        assert_eq!(parallel.len(), sequential.len());
        for (parallel, sequential) in parallel.iter().zip(&sequential) {
            assert_eq!(parallel.name, sequential.name);
            assert_eq!(parallel.compressed_size, sequential.compressed_size);
            assert_eq!(parallel.uncompressed_size, sequential.uncompressed_size);
            assert_eq!(parallel.local_header_offset, sequential.local_header_offset);
            assert_eq!(parallel.crc32, sequential.crc32);
            assert_eq!(parallel.signature, sequential.signature);
            assert_eq!(parallel.sha256, sequential.sha256);
        }
    }

    #[cfg(feature = "parallel")]
    fn test_v1_cdr_record(
        name: &str,
        compressed_size: u64,
        uncompressed_size: u64,
        local_header_offset: u64,
        crc32: u32,
        signature_byte: u8,
        sha256_byte: u8,
    ) -> Vec<u8> {
        let header = CentralDirectoryHeader {
            version_made_by: 46,
            version_needed: 45,
            flags: 0,
            compression_method: CompressionMethod::Store as u16,
            last_modified: 0x5678_1234,
            crc32,
            compressed_size: u32::MAX,
            uncompressed_size: u32::MAX,
            file_name_length: name.len() as u16,
            extra_field_length: 0xCE,
            file_comment_length: 0,
            disk_number_start: u16::MAX,
            internal_attrs: 0,
            external_attrs: 0,
            local_header_offset: u32::MAX,
        };
        let zip64 = P4kZip64ExtraField {
            id: extra_field::ZIP64,
            size: extra_field::ZIP64_TOTAL_SIZE,
            uncompressed_size,
            compressed_size,
            local_header_offset,
            disk_number_start: 0,
        };
        let mut signature = [0u8; 128];
        signature[0] = signature_byte;
        let signature = P4kSignatureExtraField {
            id: extra_field::P4K_5000,
            size: extra_field::P4K_5000_TOTAL_SIZE,
            signature,
        };
        let encryption = P4kEncryptionExtraField {
            id: extra_field::P4K_5002,
            size: extra_field::P4K_5002_TOTAL_SIZE,
            encryption: 0,
        };
        let mut sha256 = [0u8; 32];
        sha256[0] = sha256_byte;
        let sha256 = P4kSha256ExtraField {
            id: extra_field::P4K_5003,
            size: extra_field::P4K_5003_TOTAL_SIZE,
            sha256,
        };

        let mut out = Vec::new();
        out.extend_from_slice(&CentralDirectoryHeader::SIGNATURE.to_le_bytes());
        out.extend_from_slice(header.as_bytes());
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(zip64.as_bytes());
        out.extend_from_slice(signature.as_bytes());
        out.extend_from_slice(encryption.as_bytes());
        out.extend_from_slice(sha256.as_bytes());
        out
    }

    #[cfg(feature = "parallel")]
    #[test]
    fn parallel_v2_cdr_parse_matches_sequential_parse() {
        let mut builder = P4kBuilder::with_options(P4kWriterOptions {
            compression: CompressionMethod::Store,
            ..Default::default()
        });
        builder.add_bytes("Data/Beta.bin", b"beta").unwrap();
        builder.add_bytes("Data/Alpha.bin", b"alpha").unwrap();

        let mut output = Cursor::new(Vec::new());
        let stats = builder.write_to(&mut output).unwrap();
        let bytes = output.into_inner();

        let cdr_start = stats.cdr_offset as usize;
        let cdr_end = cdr_start + stats.cdr_size as usize;
        let names_start = cdr_end;
        let names_end = names_start + stats.name_table_size as usize;
        let cdr_bytes = &bytes[cdr_start..cdr_end];
        let names_bytes = &bytes[names_start..names_end];
        let install_block = &bytes[stats.payload_end as usize..stats.payload_end as usize + 16];

        let sequential = P4kArchive::parse_v2_entries(
            cdr_bytes,
            names_bytes,
            2,
            install_block,
            4096,
            stats.payload_end as usize,
            bytes.len(),
        )
        .unwrap();
        let parallel = P4kArchive::split_parsed_v2_entries(
            P4kArchive::parse_v2_entries_parallel(
                cdr_bytes,
                P4kArchive::parse_v2_names(cdr_bytes, names_bytes, 2).unwrap(),
                install_block,
                4096,
                stats.payload_end as usize,
                bytes.len(),
            )
            .unwrap(),
        );

        assert_eq!(parallel.0.len(), sequential.0.len());
        assert_eq!(parallel.1, sequential.1);
        for (parallel, sequential) in parallel.0.iter().zip(&sequential.0) {
            assert_eq!(parallel.name, sequential.name);
            assert_eq!(parallel.compressed_size, sequential.compressed_size);
            assert_eq!(parallel.uncompressed_size, sequential.uncompressed_size);
            assert_eq!(parallel.local_header_offset, sequential.local_header_offset);
            assert_eq!(parallel.crc32, sequential.crc32);
            assert_eq!(parallel.signature, sequential.signature);
            assert_eq!(parallel.sha256, sequential.sha256);
            assert_eq!(
                parallel.bytes_already_written,
                sequential.bytes_already_written
            );
        }
    }
}
