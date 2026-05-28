//! P4K archive writer, dumper, and v1-to-v2 converter.
//!
//! The v2 writer follows `CCigPakFile::UpdateAndWriteCDR_v2` from the
//! CigDataPatcher dump:
//!
//! ```text
//! [payload bytes]
//! [zero padding to physical sector]
//! [install block: u64 bytes_already_written_to_disk per entry]
//! [zero padding to physical sector]
//! [zero padding to next 64 KiB boundary]
//! [CDR: entry_count * 0xCC]
//! [name table: null-terminated normalized paths]
//! [zero padding to place EOCDR at end of aligned EOF buffer]
//! [EOCDR: 0xAF bytes ending in version=2, magic="JiJi"]
//! ```

#[cfg(feature = "parallel")]
use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter, Read, Seek, SeekFrom, Write};
#[cfg(target_os = "linux")]
use std::os::fd::AsRawFd;
#[cfg(target_os = "linux")]
use std::os::raw::{c_int, c_ulong};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use flate2::write::{DeflateEncoder, ZlibEncoder};
use sha2::{Digest, Sha256};
use zerocopy::IntoBytes;

use crate::zip::central_dir::extra_field;
use crate::zip::{
    CentralDirectoryHeaderV2, CompressionMethod, Eocd2Record, LocalFileHeader,
    P4kEncryptionExtraField, P4kSha256ExtraField, P4kSignatureExtraField, P4kZip64ExtraField,
    CDR_V2_ENTRY_SIZE, EOCD_V2_MAGIC, EOCD_V2_SIZE, EOCD_V2_VERSION,
};
use crate::{crypto, decompress, Error, P4kArchive, P4kVersion, Result};

const DEFAULT_SECTOR_SIZE: u64 = 4096;
const CDR_ALIGNMENT: u64 = 0x1_0000;
const COPY_BUFFER_SIZE: usize = 1024 * 1024;
static TEMP_PAYLOAD_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Options controlling P4K v2 creation.
#[derive(Debug, Clone)]
pub struct P4kWriterOptions {
    /// Physical sector size recorded in the v2 EOCDR and used for
    /// install/EOF buffer alignment. The official writer requires a
    /// power-of-two sector size; 4096 is the normal fallback.
    pub sector_size: u64,
    /// Compression method used by [`P4kBuilder::add_bytes`].
    pub compression: CompressionMethod,
    /// Zstandard level used when `compression == Zstd`.
    pub zstd_level: i32,
    /// DEFLATE level used when `compression == Deflate`.
    pub deflate_level: u32,
    /// Two SHA-256 digests stored verbatim in the v2 EOCDR. The first
    /// 32 bytes are the manifest hash in official archives.
    pub manifest_sha256: [u8; 64],
}

impl Default for P4kWriterOptions {
    fn default() -> Self {
        Self {
            sector_size: DEFAULT_SECTOR_SIZE,
            compression: CompressionMethod::Zstd,
            zstd_level: 1,
            deflate_level: 9,
            manifest_sha256: [0; 64],
        }
    }
}

/// Summary returned after writing a v2 archive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct P4kWriteStats {
    /// Number of entries written.
    pub entry_count: usize,
    /// Absolute byte offset immediately after payload data, before
    /// sector padding.
    pub payload_end: u64,
    /// Absolute byte offset where the v2 CDR starts.
    pub cdr_offset: u64,
    /// Total byte length of the CDR entry array.
    pub cdr_size: u64,
    /// Total byte length of the name table.
    pub name_table_size: u64,
    /// Final file size after official sector padding.
    pub file_size: u64,
}

/// Metadata supplied for a newly staged P4K entry.
///
/// The official writer fills `signature` with `RSA1024_SignMetaData`,
/// which signs [`crate::signature_metadata_sha256`] using a caller
/// supplied private key. `svarog-p4k` does not fabricate those
/// signatures, but callers that sign externally can supply the
/// resulting 128-byte field here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct P4kEntryMetadata {
    /// DOS last-modified time stored in local/CDR metadata.
    pub last_mod_file_time: u16,
    /// DOS last-modified date stored in local/CDR metadata.
    pub last_mod_file_date: u16,
    /// RSA-1024 signature field stored in P4K custom metadata.
    pub signature: [u8; 128],
}

impl Default for P4kEntryMetadata {
    fn default() -> Self {
        Self {
            last_mod_file_time: 0,
            last_mod_file_date: 0,
            signature: [0; 128],
        }
    }
}

/// Builder for creating P4K v2 archives.
#[derive(Debug, Default)]
pub struct P4kBuilder {
    options: P4kWriterOptions,
    entries: Vec<BuilderEntry>,
}

/// A filesystem entry staged with [`P4kBuilder::stage_file_with_options`].
///
/// This is useful for parallel creators: workers can hash/compress/encrypt
/// independent files into staged entries, then append those entries to one
/// builder in deterministic caller order without allocating one builder per
/// file.
#[derive(Debug)]
pub struct P4kStagedEntry {
    entry: BuilderEntry,
}

#[derive(Debug)]
struct BuilderEntry {
    name: String,
    payload: PayloadSource,
    uncompressed_size: u64,
    compression_method: CompressionMethod,
    crc32: Option<u32>,
    last_mod_file_time: u16,
    last_mod_file_date: u16,
    encrypted: bool,
    signature: [u8; 128],
    sha256: Option<[u8; 32]>,
}

#[derive(Debug)]
enum PayloadSource {
    Memory(Vec<u8>),
    File {
        path: PathBuf,
        len: u64,
        validation: FileValidation,
        cleanup: bool,
    },
}

#[derive(Debug, Clone, Copy)]
enum FileValidation {
    SizeOnly,
    Sha256([u8; 32]),
}

#[derive(Debug, Clone, Copy)]
struct PayloadWriteMetadata {
    len: u64,
    crc32: u32,
    sha256: [u8; 32],
}

impl PayloadSource {
    #[inline]
    fn len(&self) -> u64 {
        match self {
            Self::Memory(bytes) => bytes.len() as u64,
            Self::File { len, .. } => *len,
        }
    }
}

impl Drop for PayloadSource {
    fn drop(&mut self) {
        if let Self::File {
            path,
            cleanup: true,
            ..
        } = self
        {
            let _ = fs::remove_file(path);
        }
    }
}

struct V2EntryMeta<'a> {
    name: &'a str,
    offset_to_file_data: u64,
    compressed_size: u64,
    uncompressed_size: u64,
    compression_method: CompressionMethod,
    crc32: u32,
    last_mod_file_time: u16,
    last_mod_file_date: u16,
    encrypted: bool,
    signature: [u8; 128],
    sha256: [u8; 32],
    bytes_already_written: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FreelistBlock {
    offset: u64,
    size: u64,
}

#[allow(clippy::too_many_arguments)]
fn stage_file_entry_with_metadata(
    options: &P4kWriterOptions,
    path: &Path,
    archive_name: &str,
    encrypted: bool,
    file_len: u64,
    entry_metadata: P4kEntryMetadata,
) -> Result<BuilderEntry> {
    let (method, encrypted) = if file_len == 0 {
        (CompressionMethod::Store, false)
    } else {
        (options.compression, encrypted)
    };

    if method == CompressionMethod::Store && !encrypted {
        return build_entry_with_metadata(
            archive_name,
            PayloadSource::File {
                path: path.to_path_buf(),
                len: file_len,
                validation: FileValidation::SizeOnly,
                cleanup: false,
            },
            file_len,
            method,
            None,
            entry_metadata.last_mod_file_time,
            entry_metadata.last_mod_file_date,
            false,
            entry_metadata.signature,
            None,
        );
    }

    if !encrypted {
        let (payload, uncompressed_len, crc32, sha256) =
            compress_file_payload(path, method, options)?;
        return build_entry_with_metadata(
            archive_name,
            payload,
            uncompressed_len,
            method,
            Some(crc32),
            entry_metadata.last_mod_file_time,
            entry_metadata.last_mod_file_date,
            false,
            entry_metadata.signature,
            Some(sha256),
        );
    }

    let (payload, uncompressed_len, crc32, sha256) = encrypt_file_payload(path, method, options)?;
    build_entry_with_metadata(
        archive_name,
        payload,
        uncompressed_len,
        method,
        Some(crc32),
        entry_metadata.last_mod_file_time,
        entry_metadata.last_mod_file_date,
        encrypted,
        entry_metadata.signature,
        Some(sha256),
    )
}

#[allow(clippy::too_many_arguments)]
fn build_entry_with_metadata(
    name: &str,
    payload: PayloadSource,
    uncompressed_size: u64,
    method: CompressionMethod,
    crc32: Option<u32>,
    last_mod_file_time: u16,
    last_mod_file_date: u16,
    encrypted: bool,
    signature: [u8; 128],
    sha256: Option<[u8; 32]>,
) -> Result<BuilderEntry> {
    let name = normalize_archive_name(name)?;
    if encrypted && payload.len() % 16 != 0 {
        return Err(Error::Io(io::Error::new(
            io::ErrorKind::InvalidInput,
            "encrypted payload must be block-aligned",
        )));
    }
    if method == CompressionMethod::Store {
        if encrypted {
            if payload.len() < uncompressed_size {
                return Err(Error::Io(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "encrypted stored payload must be at least uncompressed_size",
                )));
            }
        } else if payload.len() != uncompressed_size {
            return Err(Error::Io(io::Error::new(
                io::ErrorKind::InvalidInput,
                "stored payload size must match uncompressed_size",
            )));
        }
    }
    if let PayloadSource::Memory(bytes) = &payload {
        let crc32 = crc32.ok_or_else(|| {
            Error::Io(io::Error::new(
                io::ErrorKind::InvalidInput,
                "memory payload is missing CRC32C metadata",
            ))
        })?;
        let sha256 = sha256.ok_or_else(|| {
            Error::Io(io::Error::new(
                io::ErrorKind::InvalidInput,
                "memory payload is missing SHA-256 metadata",
            ))
        })?;
        let actual_sha256 = sha256_array(bytes);
        if actual_sha256 != sha256 {
            return Err(Error::Io(io::Error::new(
                io::ErrorKind::InvalidInput,
                "payload SHA-256 metadata does not match raw payload bytes",
            )));
        }
        if method == CompressionMethod::Store {
            validate_stored_crc32_metadata(bytes, uncompressed_size, encrypted, crc32)?;
        }
    }
    Ok(BuilderEntry {
        name,
        payload,
        uncompressed_size,
        compression_method: method,
        crc32,
        last_mod_file_time,
        last_mod_file_date,
        encrypted,
        signature,
        sha256,
    })
}

impl P4kBuilder {
    /// Create a v2 builder with default writer options.
    pub fn new() -> Self {
        Self {
            options: P4kWriterOptions::default(),
            entries: Vec::new(),
        }
    }

    /// Create a v2 builder with explicit writer options.
    pub fn with_options(options: P4kWriterOptions) -> Self {
        Self {
            options,
            entries: Vec::new(),
        }
    }

    /// Borrow the writer options.
    #[inline]
    pub fn options(&self) -> &P4kWriterOptions {
        &self.options
    }

    /// Mutably borrow writer options.
    #[inline]
    pub fn options_mut(&mut self) -> &mut P4kWriterOptions {
        &mut self.options
    }

    /// Number of entries currently staged.
    #[inline]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether no entries are staged.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Move every staged entry from another builder into this one.
    ///
    /// This is primarily useful for parallel staging: worker threads can
    /// compress/hash independent files into one-entry builders, then the caller
    /// can append them in the desired deterministic archive order.
    pub fn append(&mut self, mut other: Self) -> &mut Self {
        self.entries.append(&mut other.entries);
        self
    }

    /// Append entries produced by [`P4kBuilder::stage_file_with_options`].
    pub fn append_staged<I>(&mut self, staged: I) -> &mut Self
    where
        I: IntoIterator<Item = P4kStagedEntry>,
    {
        self.entries
            .extend(staged.into_iter().map(|staged| staged.entry));
        self
    }

    /// Stage a filesystem file using explicit writer options.
    ///
    /// The returned entry is not tied to a builder and can be produced on a
    /// worker thread. Append staged entries in caller order with
    /// [`P4kBuilder::append_staged`] before writing the archive.
    pub fn stage_file_with_options<P, N>(
        options: &P4kWriterOptions,
        path: P,
        archive_name: N,
        encrypted: bool,
    ) -> Result<P4kStagedEntry>
    where
        P: AsRef<Path>,
        N: AsRef<str>,
    {
        let path = path.as_ref();
        let metadata = fs::metadata(path)?;
        let entry_metadata = metadata
            .modified()
            .ok()
            .and_then(dos_datetime_from_system_time)
            .map(
                |(last_mod_file_time, last_mod_file_date)| P4kEntryMetadata {
                    last_mod_file_time,
                    last_mod_file_date,
                    signature: [0; 128],
                },
            )
            .unwrap_or_default();
        let entry = stage_file_entry_with_metadata(
            options,
            path,
            archive_name.as_ref(),
            encrypted,
            metadata.len(),
            entry_metadata,
        )?;
        Ok(P4kStagedEntry { entry })
    }

    fn entries_for_fresh_write(&self) -> Vec<&BuilderEntry> {
        let filename_handles = self.fresh_filename_handles();
        let mut entries: Vec<_> = self.entries.iter().enumerate().collect();
        entries.sort_by(|(a_index, a), (b_index, b)| {
            b.payload.len().cmp(&a.payload.len()).then_with(|| {
                filename_handles[*a_index]
                    .cmp(&filename_handles[*b_index])
                    .then_with(|| a_index.cmp(b_index))
            })
        });
        entries.into_iter().map(|(_, entry)| entry).collect()
    }

    fn fresh_filename_handles(&self) -> Vec<usize> {
        let mut normalized_names: Vec<_> = self
            .entries
            .iter()
            .enumerate()
            .map(|(index, entry)| (normalize_p4k_path(&entry.name, true), index))
            .collect();
        normalized_names.sort_unstable_by(|(a_name, a_index), (b_name, b_index)| {
            a_name.cmp(b_name).then_with(|| a_index.cmp(b_index))
        });

        let mut handles = vec![0; self.entries.len()];
        let mut previous_name = None;
        let mut current_handle = 0usize;
        for (name, index) in &normalized_names {
            if previous_name.is_some_and(|previous| previous != name) {
                current_handle += 1;
            }
            handles[*index] = current_handle;
            previous_name = Some(name);
        }
        handles
    }

    /// Add uncompressed bytes, using the builder's configured
    /// compression method.
    pub fn add_bytes<N, B>(&mut self, name: N, bytes: B) -> Result<&mut Self>
    where
        N: AsRef<str>,
        B: AsRef<[u8]>,
    {
        self.add_bytes_with_method_and_encryption(name, bytes, self.options.compression, false)
    }

    /// Add uncompressed bytes with an explicit compression method.
    pub fn add_bytes_with_method<N, B>(
        &mut self,
        name: N,
        bytes: B,
        method: CompressionMethod,
    ) -> Result<&mut Self>
    where
        N: AsRef<str>,
        B: AsRef<[u8]>,
    {
        self.add_bytes_with_method_and_encryption(name, bytes, method, false)
    }

    /// Add uncompressed bytes as an encrypted entry, using the
    /// builder's configured compression method.
    pub fn add_bytes_encrypted<N, B>(&mut self, name: N, bytes: B) -> Result<&mut Self>
    where
        N: AsRef<str>,
        B: AsRef<[u8]>,
    {
        self.add_bytes_with_method_and_encryption(name, bytes, self.options.compression, true)
    }

    /// Add uncompressed bytes as an encrypted entry with an explicit
    /// compression method.
    pub fn add_bytes_with_method_encrypted<N, B>(
        &mut self,
        name: N,
        bytes: B,
        method: CompressionMethod,
    ) -> Result<&mut Self>
    where
        N: AsRef<str>,
        B: AsRef<[u8]>,
    {
        self.add_bytes_with_method_and_encryption(name, bytes, method, true)
    }

    /// Add uncompressed bytes with explicit compression, encryption,
    /// DOS timestamp, and signature metadata.
    pub fn add_bytes_with_entry_metadata<N, B>(
        &mut self,
        name: N,
        bytes: B,
        method: CompressionMethod,
        encrypted: bool,
        metadata: P4kEntryMetadata,
    ) -> Result<&mut Self>
    where
        N: AsRef<str>,
        B: AsRef<[u8]>,
    {
        self.add_bytes_with_method_encryption_metadata(name, bytes, method, encrypted, metadata)
    }

    fn add_bytes_with_method_and_encryption<N, B>(
        &mut self,
        name: N,
        bytes: B,
        method: CompressionMethod,
        encrypted: bool,
    ) -> Result<&mut Self>
    where
        N: AsRef<str>,
        B: AsRef<[u8]>,
    {
        self.add_bytes_with_method_encryption_metadata(
            name,
            bytes,
            method,
            encrypted,
            P4kEntryMetadata::default(),
        )
    }

    fn add_bytes_with_method_encryption_metadata<N, B>(
        &mut self,
        name: N,
        bytes: B,
        method: CompressionMethod,
        encrypted: bool,
        metadata: P4kEntryMetadata,
    ) -> Result<&mut Self>
    where
        N: AsRef<str>,
        B: AsRef<[u8]>,
    {
        let bytes = bytes.as_ref();
        let (method, encrypted) = if bytes.is_empty() {
            (CompressionMethod::Store, false)
        } else {
            (method, encrypted)
        };
        let mut payload = compress_bytes(bytes, method, &self.options)?;
        if encrypted {
            payload = crypto::encrypt(&payload).map_err(|e| Error::Encryption(e.to_string()))?;
        }
        let crc32 = svarog_common::crc::hash_bytes(bytes);
        let sha256 = sha256_array(&payload);
        self.add_payload_with_metadata(
            name,
            PayloadSource::Memory(payload),
            bytes.len() as u64,
            method,
            Some(crc32),
            metadata.last_mod_file_time,
            metadata.last_mod_file_date,
            encrypted,
            metadata.signature,
            Some(sha256),
        )
    }

    /// Add a filesystem file using `archive_name` as the path stored
    /// inside the P4K.
    pub fn add_file<P, N>(&mut self, path: P, archive_name: N) -> Result<&mut Self>
    where
        P: AsRef<Path>,
        N: AsRef<str>,
    {
        self.add_file_with_encryption(path, archive_name, false)
    }

    /// Add a filesystem file as an encrypted entry.
    pub fn add_file_encrypted<P, N>(&mut self, path: P, archive_name: N) -> Result<&mut Self>
    where
        P: AsRef<Path>,
        N: AsRef<str>,
    {
        self.add_file_with_encryption(path, archive_name, true)
    }

    /// Add a filesystem file with explicit encryption, DOS timestamp,
    /// and signature metadata.
    ///
    /// Compression still follows [`P4kWriterOptions::compression`].
    pub fn add_file_with_entry_metadata<P, N>(
        &mut self,
        path: P,
        archive_name: N,
        encrypted: bool,
        metadata: P4kEntryMetadata,
    ) -> Result<&mut Self>
    where
        P: AsRef<Path>,
        N: AsRef<str>,
    {
        self.add_file_with_encryption_metadata(path, archive_name, encrypted, metadata)
    }

    fn add_file_with_encryption<P, N>(
        &mut self,
        path: P,
        archive_name: N,
        encrypted: bool,
    ) -> Result<&mut Self>
    where
        P: AsRef<Path>,
        N: AsRef<str>,
    {
        let path = path.as_ref();
        let metadata = fs::metadata(path)?;
        let entry_metadata = metadata
            .modified()
            .ok()
            .and_then(dos_datetime_from_system_time)
            .map(
                |(last_mod_file_time, last_mod_file_date)| P4kEntryMetadata {
                    last_mod_file_time,
                    last_mod_file_date,
                    signature: [0; 128],
                },
            )
            .unwrap_or_default();

        self.add_file_with_metadata(
            path,
            archive_name,
            encrypted,
            metadata.len(),
            entry_metadata,
        )
    }

    fn add_file_with_encryption_metadata<P, N>(
        &mut self,
        path: P,
        archive_name: N,
        encrypted: bool,
        entry_metadata: P4kEntryMetadata,
    ) -> Result<&mut Self>
    where
        P: AsRef<Path>,
        N: AsRef<str>,
    {
        let path = path.as_ref();
        let metadata = fs::metadata(path)?;
        self.add_file_with_metadata(
            path,
            archive_name,
            encrypted,
            metadata.len(),
            entry_metadata,
        )
    }

    fn add_file_with_metadata<N>(
        &mut self,
        path: &Path,
        archive_name: N,
        encrypted: bool,
        file_len: u64,
        entry_metadata: P4kEntryMetadata,
    ) -> Result<&mut Self>
    where
        N: AsRef<str>,
    {
        let entry = stage_file_entry_with_metadata(
            &self.options,
            path,
            archive_name.as_ref(),
            encrypted,
            file_len,
            entry_metadata,
        )?;
        self.entries.push(entry);
        Ok(self)
    }

    /// Add already-compressed payload bytes.
    ///
    /// This is used by the v1->v2 converter so it can preserve the
    /// payload bytes and encryption state without decompressing.
    #[allow(clippy::too_many_arguments)]
    pub fn add_precompressed<N, B>(
        &mut self,
        name: N,
        payload: B,
        uncompressed_size: u64,
        method: CompressionMethod,
        crc32: u32,
        encrypted: bool,
        signature: [u8; 128],
    ) -> Result<&mut Self>
    where
        N: AsRef<str>,
        B: Into<Vec<u8>>,
    {
        let payload = payload.into();
        let sha256 = sha256_array(&payload);
        self.add_precompressed_with_metadata(
            name,
            payload,
            uncompressed_size,
            method,
            crc32,
            0,
            0,
            encrypted,
            signature,
            sha256,
        )
    }

    /// Add already-compressed payload bytes with all P4K metadata
    /// supplied by the caller.
    #[allow(clippy::too_many_arguments)]
    pub fn add_precompressed_with_metadata<N, B>(
        &mut self,
        name: N,
        payload: B,
        uncompressed_size: u64,
        method: CompressionMethod,
        crc32: u32,
        last_mod_file_time: u16,
        last_mod_file_date: u16,
        encrypted: bool,
        signature: [u8; 128],
        sha256: [u8; 32],
    ) -> Result<&mut Self>
    where
        N: AsRef<str>,
        B: Into<Vec<u8>>,
    {
        let payload = payload.into();
        if encrypted && payload.len() % 16 != 0 {
            return Err(Error::Io(io::Error::new(
                io::ErrorKind::InvalidInput,
                "encrypted payload must be block-aligned",
            )));
        }
        if method == CompressionMethod::Store {
            if encrypted {
                if (payload.len() as u64) < uncompressed_size {
                    return Err(Error::Io(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "encrypted stored payload must be at least uncompressed_size",
                    )));
                }
            } else if payload.len() as u64 != uncompressed_size {
                return Err(Error::Io(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "stored payload size must match uncompressed_size",
                )));
            }
        }
        validate_memory_payload_metadata(
            &payload,
            uncompressed_size,
            method,
            encrypted,
            crc32,
            sha256,
        )?;
        self.add_payload_with_metadata(
            name,
            PayloadSource::Memory(payload),
            uncompressed_size,
            method,
            Some(crc32),
            last_mod_file_time,
            last_mod_file_date,
            encrypted,
            signature,
            Some(sha256),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn add_payload_with_metadata<N>(
        &mut self,
        name: N,
        payload: PayloadSource,
        uncompressed_size: u64,
        method: CompressionMethod,
        crc32: Option<u32>,
        last_mod_file_time: u16,
        last_mod_file_date: u16,
        encrypted: bool,
        signature: [u8; 128],
        sha256: Option<[u8; 32]>,
    ) -> Result<&mut Self>
    where
        N: AsRef<str>,
    {
        let entry = build_entry_with_metadata(
            name.as_ref(),
            payload,
            uncompressed_size,
            method,
            crc32,
            last_mod_file_time,
            last_mod_file_date,
            encrypted,
            signature,
            sha256,
        )?;
        self.entries.push(entry);
        Ok(self)
    }

    /// Write the archive to a new file.
    pub fn write_to_file<P: AsRef<Path>>(&self, path: P) -> Result<P4kWriteStats> {
        write_archive_to_temp_output(path.as_ref(), |writer| self.write_to(writer))
    }

    /// Write a legacy ZIP64-based P4K v1 archive to a new file.
    pub fn write_v1_to_file<P: AsRef<Path>>(&self, path: P) -> Result<P4kWriteStats> {
        write_archive_to_temp_output(path.as_ref(), |writer| self.write_v1_to(writer))
    }

    /// Write the archive to any seekable writer.
    pub fn write_to<W: Write + Seek>(&self, writer: &mut W) -> Result<P4kWriteStats> {
        validate_options(&self.options)?;

        let ordered_entries = self.entries_for_fresh_write();
        let mut entries = Vec::with_capacity(self.entries.len());
        for entry in ordered_entries {
            let offset = writer.stream_position()?;
            let payload_metadata = write_entry_payload(writer, entry)?;
            let next_offset = writer.stream_position()?;
            let next_aligned = align_up(next_offset, self.options.sector_size)?;
            write_zeroes(writer, next_aligned - next_offset)?;
            entries.push(V2EntryMeta {
                name: &entry.name,
                offset_to_file_data: if payload_metadata.len == 0 { 0 } else { offset },
                compressed_size: payload_metadata.len,
                uncompressed_size: entry.uncompressed_size,
                compression_method: entry.compression_method,
                crc32: payload_metadata.crc32,
                last_mod_file_time: entry.last_mod_file_time,
                last_mod_file_date: entry.last_mod_file_date,
                encrypted: entry.encrypted,
                signature: entry.signature,
                sha256: payload_metadata.sha256,
                bytes_already_written: payload_metadata.len,
            });
        }

        let payload_end_unaligned = writer.stream_position()?;
        write_v2_tail(
            writer,
            &entries,
            &[],
            payload_end_unaligned,
            &self.options,
            true,
        )
    }

    /// Write a legacy ZIP64-based P4K v1 archive to any seekable writer.
    ///
    /// This follows `CCigPakFileEntry::WriteLocalFileHeaderToBuffer_v1`
    /// and `CCigPakFile::UpdateAndWriteCDR_v1` from the dump:
    /// each payload is preceded by a padded local file record, then
    /// an install block, ZIP64 CDR, ZIP64 EOCD, ZIP64 locator, and
    /// EOCD with the 16-byte P4K comment (`CI` plus marker `0x00010047`).
    pub fn write_v1_to<W: Write + Seek>(&self, writer: &mut W) -> Result<P4kWriteStats> {
        validate_options(&self.options)?;
        if self.options.sector_size > u16::MAX as u64 {
            return Err(Error::Io(io::Error::new(
                io::ErrorKind::InvalidInput,
                "v1 sector size must fit in u16",
            )));
        }
        if self.entries.len() > u32::MAX as usize {
            return Err(Error::Io(io::Error::new(
                io::ErrorKind::InvalidInput,
                "v1 entry count must fit in u32",
            )));
        }

        let ordered_entries = self.entries_for_fresh_write();
        let mut resolved_metadata = Vec::with_capacity(ordered_entries.len());
        let mut local_header_offsets = Vec::with_capacity(self.entries.len());
        for entry in &ordered_entries {
            let local_header_offset = writer.stream_position()?;
            local_header_offsets.push(local_header_offset);

            let copied_metadata = if can_defer_v1_payload_metadata(entry) {
                let local_record_size =
                    checked_v1_local_record_size_for_entry(entry, self.options.sector_size)?;
                write_zeroes(writer, local_record_size)?;
                let payload_start = writer.stream_position()?;
                let copied_metadata = write_entry_payload(writer, entry)?;
                let payload_end = writer.stream_position()?;
                if copied_metadata.len != entry.payload.len() {
                    return Err(staged_file_changed_error_for_entry(entry));
                }
                writer.seek(SeekFrom::Start(local_header_offset))?;
                write_v1_local_record(
                    writer,
                    entry,
                    copied_metadata,
                    local_header_offset,
                    self.options.sector_size,
                )?;
                debug_assert_eq!(writer.stream_position()?, payload_start);
                writer.seek(SeekFrom::Start(payload_end))?;
                copied_metadata
            } else {
                let metadata = resolve_entry_payload_metadata(entry)?;
                write_v1_local_record(
                    writer,
                    entry,
                    metadata,
                    local_header_offset,
                    self.options.sector_size,
                )?;
                let copied_metadata = write_entry_payload(writer, entry)?;
                if copied_metadata.crc32 != metadata.crc32
                    || copied_metadata.sha256 != metadata.sha256
                {
                    return Err(staged_file_changed_error_for_entry(entry));
                }
                metadata
            };
            resolved_metadata.push(copied_metadata);
            let next_offset = writer.stream_position()?;
            let next_aligned = align_up(next_offset, self.options.sector_size)?;
            write_zeroes(writer, next_aligned - next_offset)?;
        }

        let payload_end = writer.stream_position()?;
        for metadata in &resolved_metadata {
            writer.write_all(&metadata.len.to_le_bytes())?;
            writer.write_all(&0u32.to_le_bytes())?;
        }

        let cdr_offset = writer.stream_position()?;
        for (index, (entry, metadata)) in ordered_entries.iter().zip(&resolved_metadata).enumerate()
        {
            write_v1_central_directory_entry(
                writer,
                entry,
                *metadata,
                local_header_offsets[index],
            )?;
        }
        let cdr_size = writer.stream_position()? - cdr_offset;

        let zip64_eocd_offset = writer.stream_position()?;
        write_v1_zip64_eocd(writer, self.entries.len() as u64, cdr_size, cdr_offset)?;
        write_v1_zip64_locator(writer, zip64_eocd_offset)?;
        write_v1_eocd(writer, self.options.sector_size, 0)?;

        let unaligned_file_size = writer.stream_position()?;
        let file_size = align_up(unaligned_file_size, self.options.sector_size)?;
        write_zeroes(writer, file_size - unaligned_file_size)?;

        Ok(P4kWriteStats {
            entry_count: self.entries.len(),
            payload_end,
            cdr_offset,
            cdr_size,
            name_table_size: 0,
            file_size,
        })
    }
}

/// Dump every entry from `archive` to `output_dir`.
pub fn dump_archive_to_dir<P: AsRef<Path>>(archive: &P4kArchive, output_dir: P) -> Result<()> {
    let output_dir = output_dir.as_ref();
    fs::create_dir_all(output_dir)?;

    #[cfg(feature = "parallel")]
    {
        dump_archive_to_dir_parallel(archive, output_dir)
    }

    #[cfg(not(feature = "parallel"))]
    {
        dump_archive_to_dir_sequential(archive, output_dir)
    }
}

fn dump_archive_to_dir_sequential(archive: &P4kArchive, output_dir: &Path) -> Result<()> {
    for (index, entry) in archive.iter().enumerate() {
        if entry.name.ends_with('\\') || entry.name.ends_with('/') {
            continue;
        }

        let out_path = safe_output_path(output_dir, entry.name)?;
        dump_entry_to_path(archive, index, &out_path)?;
    }

    Ok(())
}

#[cfg(feature = "parallel")]
fn dump_archive_to_dir_parallel(archive: &P4kArchive, output_dir: &Path) -> Result<()> {
    use rayon::prelude::*;

    let mut tasks = Vec::with_capacity(archive.entry_count());
    let mut seen = HashSet::with_capacity(archive.entry_count());
    let mut has_duplicate_output = false;

    for (index, entry) in archive.iter().enumerate() {
        if entry.name.ends_with('\\') || entry.name.ends_with('/') {
            continue;
        }

        let out_path = safe_output_path(output_dir, entry.name)?;
        if !seen.insert(out_path.clone()) {
            has_duplicate_output = true;
        }
        tasks.push((index, out_path));
    }

    if has_duplicate_output {
        return dump_archive_to_dir_sequential(archive, output_dir);
    }

    let mut parent_dirs = HashSet::with_capacity(tasks.len());
    for (_, out_path) in &tasks {
        if let Some(parent) = out_path.parent() {
            parent_dirs.insert(parent.to_path_buf());
        }
    }
    for dir in parent_dirs {
        fs::create_dir_all(dir)?;
    }

    tasks
        .par_iter()
        .try_for_each(|(index, out_path)| dump_entry_to_prepared_path(archive, *index, out_path))
}

fn dump_entry_to_path(archive: &P4kArchive, index: usize, out_path: &Path) -> Result<()> {
    if let Some(parent) = out_path.parent() {
        fs::create_dir_all(parent)?;
    }
    dump_entry_to_prepared_path(archive, index, out_path)
}

fn dump_entry_to_prepared_path(archive: &P4kArchive, index: usize, out_path: &Path) -> Result<()> {
    let mut output = BufWriter::with_capacity(1024 * 1024, File::create(out_path)?);
    archive.extract_index_to_writer(index, &mut output)?;
    output.flush()?;
    Ok(())
}

/// Convert a v1 P4K archive to v2.
///
/// The converter preserves compressed payload bytes, compression
/// method, CRC, sizes, encryption flags, RSA signatures, and SHA-256
/// metadata.
pub fn convert_v1_to_v2<P, Q>(
    input: P,
    output: Q,
    options: P4kWriterOptions,
) -> Result<P4kWriteStats>
where
    P: AsRef<Path>,
    Q: AsRef<Path>,
{
    validate_options(&options)?;

    let input = input.as_ref();
    let output = output.as_ref();
    let archive = P4kArchive::open(input)?;
    if archive.version() != P4kVersion::V1 {
        return Err(Error::Io(io::Error::new(
            io::ErrorKind::InvalidInput,
            "source archive is not P4K v1",
        )));
    }
    if same_existing_file(input, output)? {
        return Err(Error::Io(io::Error::new(
            io::ErrorKind::InvalidInput,
            "source and destination archive paths refer to the same file",
        )));
    }

    let source_file_size = fs::metadata(input)?.len();
    let raw_entries = archive.raw_entries()?;
    let mut payload_end_unaligned = max_v1_payload_end(&raw_entries)?;
    if payload_end_unaligned > source_file_size {
        return Err(Error::Io(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "source v1 payload range extends past file end",
        )));
    }

    let mut entries = Vec::with_capacity(raw_entries.len());
    let mut freelist_blocks =
        Vec::with_capacity(raw_entries.len() + archive.freelist_blocks().len());
    freelist_blocks.extend(archive.freelist_blocks().iter().map(|block| FreelistBlock {
        offset: block.offset,
        size: block.size,
    }));
    for raw in raw_entries {
        if raw.payload_len != 0 && raw.payload_offset % options.sector_size != 0 {
            return Err(Error::Io(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "destination sector size {} is incompatible with source payload offset {} for {}",
                    options.sector_size, raw.payload_offset, raw.name
                ),
            )));
        }
        entries.push(V2EntryMeta {
            name: raw.name,
            offset_to_file_data: if raw.payload_len == 0 {
                0
            } else {
                raw.payload_offset
            },
            compressed_size: raw.payload_len,
            uncompressed_size: raw.uncompressed_size,
            compression_method: raw.compression_method,
            crc32: raw.crc32,
            last_mod_file_time: raw.last_mod_file_time,
            last_mod_file_date: raw.last_mod_file_date,
            encrypted: raw.is_encrypted,
            signature: raw.signature,
            sha256: raw.sha256,
            bytes_already_written: raw.bytes_already_written,
        });
        freelist_blocks.push(FreelistBlock {
            offset: raw.local_header_offset,
            size: raw.local_record_size,
        });
    }
    merge_freelist_blocks(&mut freelist_blocks)?;
    trim_trailing_freelist_blocks(
        &mut freelist_blocks,
        &mut payload_end_unaligned,
        options.sector_size,
    )?;

    let (temp_output, mut file) = create_temp_output_file(output)?;
    let result = (|| {
        if payload_end_unaligned > 0 {
            let input_file = File::open(input)?;
            if !try_clone_prefix(&input_file, &mut file, payload_end_unaligned)?
                && !try_copy_file_range_prefix(&input_file, &mut file, payload_end_unaligned)?
            {
                copy_exact_prefix(input_file, &mut file, payload_end_unaligned)?;
            }
        }
        let mut out = BufWriter::with_capacity(1024 * 1024, file);
        let stats = write_v2_tail(
            &mut out,
            &entries,
            &freelist_blocks,
            payload_end_unaligned,
            &options,
            false,
        )?;
        out.flush()?;
        let file = out
            .into_inner()
            .map_err(|err| Error::Io(err.into_error()))?;
        drop(file);
        Ok(stats)
    })();

    finish_temp_output(temp_output, output, result)
}

fn max_v1_payload_end(raw_entries: &[crate::archive::P4kRawEntryRef<'_>]) -> Result<u64> {
    let mut max_payload_end = 0u64;
    for raw in raw_entries {
        if raw.payload_len == 0 {
            continue;
        }
        let payload_end = raw
            .payload_offset
            .checked_add(raw.payload_len)
            .ok_or_else(|| {
                Error::Io(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "source v1 payload range overflow",
                ))
            })?;
        max_payload_end = max_payload_end.max(payload_end);
    }
    Ok(max_payload_end)
}

fn write_archive_to_temp_output<F>(output: &Path, write: F) -> Result<P4kWriteStats>
where
    F: FnOnce(&mut BufWriter<File>) -> Result<P4kWriteStats>,
{
    let (temp_output, file) = create_temp_output_file(output)?;
    let result = (|| {
        let mut writer = BufWriter::with_capacity(COPY_BUFFER_SIZE, file);
        let stats = write(&mut writer)?;
        writer.flush()?;
        let file = writer
            .into_inner()
            .map_err(|err| Error::Io(err.into_error()))?;
        drop(file);
        Ok(stats)
    })();
    finish_temp_output(temp_output, output, result)
}

fn finish_temp_output<T>(temp_output: PathBuf, output: &Path, result: Result<T>) -> Result<T> {
    match result {
        Ok(value) => {
            if let Err(err) = fs::rename(&temp_output, output) {
                let _ = fs::remove_file(&temp_output);
                return Err(Error::Io(err));
            }
            Ok(value)
        }
        Err(err) => {
            let _ = fs::remove_file(&temp_output);
            Err(err)
        }
    }
}

fn same_existing_file(input: &Path, output: &Path) -> Result<bool> {
    let output = match fs::canonicalize(output) {
        Ok(path) => path,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(err) => return Err(Error::Io(err)),
    };
    Ok(fs::canonicalize(input)? == output)
}

fn create_temp_output_file(output: &Path) -> Result<(PathBuf, File)> {
    let parent = output
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let file_name = output.file_name().ok_or_else(|| {
        Error::Io(io::Error::new(
            io::ErrorKind::InvalidInput,
            "output path must include a file name",
        ))
    })?;
    let file_name = file_name.to_string_lossy();

    for _ in 0..1024 {
        let counter = TEMP_PAYLOAD_COUNTER.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        let path = parent.join(format!(
            ".{file_name}.svarog-p4k-output-{}-{nanos}-{counter}.tmp",
            std::process::id()
        ));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => return Ok((path, file)),
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(Error::Io(err)),
        }
    }

    Err(Error::Io(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not create unique temporary output file",
    )))
}

fn try_clone_prefix(source: &File, destination: &mut File, expected_len: u64) -> Result<bool> {
    #[cfg(target_os = "linux")]
    {
        try_clone_prefix_linux(source, destination, expected_len)
    }

    #[cfg(not(target_os = "linux"))]
    {
        let _ = (source, destination, expected_len);
        Ok(false)
    }
}

fn try_copy_file_range_prefix(
    source: &File,
    destination: &mut File,
    expected_len: u64,
) -> Result<bool> {
    #[cfg(target_os = "linux")]
    {
        try_copy_file_range_prefix_linux(source, destination, expected_len)
    }

    #[cfg(not(target_os = "linux"))]
    {
        let _ = (source, destination, expected_len);
        Ok(false)
    }
}

#[cfg(target_os = "linux")]
fn try_clone_prefix_linux(
    source: &File,
    destination: &mut File,
    expected_len: u64,
) -> Result<bool> {
    const FICLONE: c_ulong = 0x4004_9409;

    unsafe extern "C" {
        fn ioctl(fd: c_int, request: c_ulong, ...) -> c_int;
    }

    // SAFETY: FICLONE expects the destination fd as the ioctl target and a
    // readable source fd as its integer vararg. Both descriptors remain owned
    // by their File values; the kernel only clones extents into destination.
    let cloned = unsafe { ioctl(destination.as_raw_fd(), FICLONE, source.as_raw_fd()) } == 0;
    if !cloned {
        destination.set_len(0)?;
        destination.seek(SeekFrom::Start(0))?;
        return Ok(false);
    }

    destination.set_len(expected_len)?;
    destination.seek(SeekFrom::Start(expected_len))?;
    Ok(true)
}

#[cfg(target_os = "linux")]
fn try_copy_file_range_prefix_linux(
    source: &File,
    destination: &mut File,
    expected_len: u64,
) -> Result<bool> {
    unsafe extern "C" {
        fn copy_file_range(
            fd_in: c_int,
            off_in: *mut i64,
            fd_out: c_int,
            off_out: *mut i64,
            len: usize,
            flags: u32,
        ) -> isize;
    }

    if expected_len == 0 {
        return Ok(true);
    }
    if expected_len > i64::MAX as u64 {
        return Err(Error::Io(io::Error::new(
            io::ErrorKind::InvalidInput,
            "copy_file_range prefix length overflows i64",
        )));
    }

    let mut offset_in = 0i64;
    let mut offset_out = 0i64;
    let mut copied = 0u64;
    while copied < expected_len {
        let remaining = expected_len - copied;
        let len = remaining.min(1 << 30) as usize;
        let result = unsafe {
            copy_file_range(
                source.as_raw_fd(),
                &mut offset_in,
                destination.as_raw_fd(),
                &mut offset_out,
                len,
                0,
            )
        };
        if result < 0 {
            if copied != 0 {
                destination.set_len(0)?;
                destination.seek(SeekFrom::Start(0))?;
                return Ok(false);
            }
            return Ok(false);
        }
        if result == 0 {
            destination.set_len(0)?;
            destination.seek(SeekFrom::Start(0))?;
            return Err(Error::Io(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                format!("source ended after {copied} bytes, expected {expected_len} bytes"),
            )));
        }
        copied = copied.checked_add(result as u64).ok_or_else(|| {
            Error::Io(io::Error::new(io::ErrorKind::InvalidInput, "copy overflow"))
        })?;
    }

    destination.seek(SeekFrom::Start(expected_len))?;
    Ok(true)
}

fn copy_exact_prefix<R, W>(reader: R, writer: &mut W, expected_len: u64) -> Result<()>
where
    R: Read,
    W: Write,
{
    const BUFFER_SIZE: usize = 8 * 1024 * 1024;

    let mut reader = reader.take(expected_len);
    let mut buffer = vec![0u8; BUFFER_SIZE];
    let mut copied = 0u64;
    while copied < expected_len {
        let remaining = expected_len - copied;
        let limit = remaining.min(BUFFER_SIZE as u64) as usize;
        let read = reader.read(&mut buffer[..limit])?;
        if read == 0 {
            return Err(Error::Io(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                format!("source ended after {copied} bytes, expected {expected_len} bytes"),
            )));
        }
        writer.write_all(&buffer[..read])?;
        copied += read as u64;
    }
    Ok(())
}

fn write_v2_tail<W: Write + Seek>(
    writer: &mut W,
    entries: &[V2EntryMeta<'_>],
    freelist_blocks: &[FreelistBlock],
    payload_end_unaligned: u64,
    options: &P4kWriterOptions,
    align_payload_end: bool,
) -> Result<P4kWriteStats> {
    let current = writer.stream_position()?;
    if current > payload_end_unaligned {
        return Err(Error::Io(io::Error::new(
            io::ErrorKind::InvalidInput,
            "writer is past the v2 payload end",
        )));
    }

    let end_of_payload = if align_payload_end {
        align_up(payload_end_unaligned, options.sector_size)?
    } else {
        payload_end_unaligned
    };
    write_zeroes(writer, end_of_payload - current)?;

    let install_block_offset = end_of_payload;
    for entry in entries {
        writer.write_all(&entry.bytes_already_written.to_le_bytes())?;
    }
    for block in freelist_blocks {
        writer.write_all(&block.offset.to_le_bytes())?;
        writer.write_all(&block.size.to_le_bytes())?;
    }
    let install_used = writer.stream_position()? - install_block_offset;
    let install_aligned = align_up(install_used, options.sector_size)?;
    write_zeroes(writer, install_aligned - install_used)?;

    let cdr_unaligned = install_block_offset
        .checked_add(install_aligned)
        .ok_or_else(|| {
            Error::Io(io::Error::new(
                io::ErrorKind::InvalidInput,
                "v2 CDR offset overflow",
            ))
        })?;
    let cdr_offset = align_up(cdr_unaligned, CDR_ALIGNMENT)?;
    let current = writer.stream_position()?;
    write_zeroes(writer, cdr_offset - current)?;

    let mut name_table_len = 0u64;
    for entry in entries {
        let name_len = usize_to_u64(entry.name.len(), "name length")?;
        name_table_len = name_table_len
            .checked_add(name_len)
            .and_then(|value| value.checked_add(1))
            .ok_or_else(|| {
                Error::Io(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "v2 name table size overflow",
                ))
            })?;
    }

    let mut name_offset = 0u64;
    for entry in entries {
        let cdr_entry = CentralDirectoryHeaderV2 {
            compression_method: entry.compression_method as u16,
            last_mod_file_time: entry.last_mod_file_time,
            last_mod_file_date: entry.last_mod_file_date,
            crc32: entry.crc32,
            compressed_size: entry.compressed_size,
            uncompressed_size: entry.uncompressed_size,
            offset_to_file_data: entry.offset_to_file_data,
            offset_to_filename: name_offset,
            signature: entry.signature,
            encryption_flag: u16::from(entry.encrypted),
            sha256: entry.sha256,
        };
        writer.write_all(cdr_entry.as_bytes())?;
        name_offset = name_offset
            .checked_add(usize_to_u64(entry.name.len(), "name length")?)
            .and_then(|value| value.checked_add(1))
            .ok_or_else(|| {
                Error::Io(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "v2 name table size overflow",
                ))
            })?;
    }
    debug_assert_eq!(name_offset, name_table_len);

    let cdr_size = entries
        .len()
        .checked_mul(CDR_V2_ENTRY_SIZE)
        .and_then(|value| u64::try_from(value).ok())
        .ok_or_else(|| {
            Error::Io(io::Error::new(
                io::ErrorKind::InvalidInput,
                "v2 CDR size overflow",
            ))
        })?;
    let name_table_abs_offset = cdr_offset.checked_add(cdr_size).ok_or_else(|| {
        Error::Io(io::Error::new(
            io::ErrorKind::InvalidInput,
            "v2 name table offset overflow",
        ))
    })?;
    for entry in entries {
        writer.write_all(entry.name.as_bytes())?;
        writer.write_all(&[0])?;
    }

    let eocdr = Eocd2Record {
        num_file_entries: usize_to_u64(entries.len(), "entry count")?,
        reserved_08: 0,
        end_of_file_block_offset: cdr_offset,
        cdr_size,
        reserved_20: 0,
        name_table_abs_offset,
        total_name_length: name_table_len,
        reserved_38: 0,
        end_of_payload,
        num_freelist_blocks: usize_to_u64(freelist_blocks.len(), "freelist block count")?,
        reserved_50: 0,
        reserved_58: 0,
        physical_sector_size: options.sector_size,
        flag_68: 1,
        manifest_sha256: options.manifest_sha256,
        version: EOCD_V2_VERSION,
        magic: EOCD_V2_MAGIC,
    };
    let eof_used = cdr_size
        .checked_add(name_table_len)
        .and_then(|value| value.checked_add(EOCD_V2_SIZE as u64))
        .ok_or_else(|| {
            Error::Io(io::Error::new(
                io::ErrorKind::InvalidInput,
                "v2 EOF buffer size overflow",
            ))
        })?;
    let eof_aligned = align_up(eof_used, options.sector_size)?;
    write_zeroes(writer, eof_aligned - eof_used)?;
    writer.write_all(eocdr.as_bytes())?;

    Ok(P4kWriteStats {
        entry_count: entries.len(),
        payload_end: end_of_payload,
        cdr_offset,
        cdr_size,
        name_table_size: name_table_len,
        file_size: writer.stream_position()?,
    })
}

fn merge_freelist_blocks(blocks: &mut Vec<FreelistBlock>) -> Result<()> {
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
        let Some(prev_end) = prev.offset.checked_add(prev.size) else {
            blocks[write] = block;
            write += 1;
            continue;
        };
        if block.offset == prev_end {
            prev.size = prev.size.checked_add(block.size).ok_or_else(|| {
                Error::Io(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "merged freelist block size overflow",
                ))
            })?;
        } else {
            blocks[write] = block;
            write += 1;
        }
    }
    blocks.truncate(write);
    Ok(())
}

fn trim_trailing_freelist_blocks(
    blocks: &mut Vec<FreelistBlock>,
    payload_end_unaligned: &mut u64,
    sector_size: u64,
) -> Result<()> {
    let mut end_of_payload = align_up(*payload_end_unaligned, sector_size)?;
    while let Some(block) = blocks.last().copied() {
        let block_end = block.offset.checked_add(block.size).ok_or_else(|| {
            Error::Io(io::Error::new(
                io::ErrorKind::InvalidInput,
                "freelist block range overflow",
            ))
        })?;
        if block_end != end_of_payload {
            break;
        }
        end_of_payload = block.offset;
        blocks.pop();
    }
    *payload_end_unaligned = end_of_payload;
    Ok(())
}

fn compress_bytes(
    bytes: &[u8],
    method: CompressionMethod,
    options: &P4kWriterOptions,
) -> Result<Vec<u8>> {
    match method {
        CompressionMethod::Store => Ok(bytes.to_vec()),
        CompressionMethod::Deflate => {
            let mut encoder = DeflateEncoder::new(
                Vec::with_capacity(bytes.len()),
                flate2::Compression::new(options.deflate_level),
            );
            encoder.write_all(bytes)?;
            Ok(encoder.finish()?)
        }
        CompressionMethod::DeflateZlib => {
            let mut encoder = ZlibEncoder::new(
                Vec::with_capacity(bytes.len()),
                flate2::Compression::new(options.deflate_level),
            );
            encoder.write_all(bytes)?;
            Ok(encoder.finish()?)
        }
        CompressionMethod::Zstd | CompressionMethod::ZstdDeprecated => {
            zstd::stream::encode_all(bytes, options.zstd_level).map_err(Error::Io)
        }
    }
}

fn resolve_entry_payload_metadata(entry: &BuilderEntry) -> Result<PayloadWriteMetadata> {
    if let (Some(crc32), Some(sha256)) = (entry.crc32, entry.sha256) {
        return Ok(PayloadWriteMetadata {
            len: entry.payload.len(),
            crc32,
            sha256,
        });
    }

    match &entry.payload {
        PayloadSource::File {
            path,
            len,
            validation: FileValidation::SizeOnly,
            ..
        } if entry.compression_method == CompressionMethod::Store && !entry.encrypted => {
            let (actual_len, crc32, sha256) = hash_file_payload(path)?;
            if actual_len != *len {
                return Err(staged_file_changed_error(path));
            }
            Ok(PayloadWriteMetadata {
                len: actual_len,
                crc32,
                sha256,
            })
        }
        _ => Err(Error::Io(io::Error::new(
            io::ErrorKind::InvalidInput,
            "payload is missing CRC32C/SHA-256 metadata",
        ))),
    }
}

fn can_defer_v1_payload_metadata(entry: &BuilderEntry) -> bool {
    matches!(
        &entry.payload,
        PayloadSource::File {
            validation: FileValidation::SizeOnly,
            ..
        }
    ) && entry.compression_method == CompressionMethod::Store
        && !entry.encrypted
}

fn write_entry_payload<W: Write>(
    writer: &mut W,
    entry: &BuilderEntry,
) -> Result<PayloadWriteMetadata> {
    match &entry.payload {
        PayloadSource::Memory(bytes) => {
            writer.write_all(bytes).map_err(Error::Io)?;
            resolve_entry_payload_metadata(entry)
        }
        PayloadSource::File {
            path,
            len,
            validation,
            ..
        } => match validation {
            FileValidation::SizeOnly => copy_file_payload_hashed(path, *len, writer),
            FileValidation::Sha256(expected_sha256) => copy_file_payload_validated_sha256(
                path,
                *len,
                entry.crc32.ok_or_else(|| {
                    Error::Io(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "file payload is missing CRC32C metadata",
                    ))
                })?,
                *expected_sha256,
                writer,
            ),
        },
    }
}

fn hash_file_payload(path: &Path) -> Result<(u64, u32, [u8; 32])> {
    let mut file = File::open(path)?;
    hash_reader(&mut file)
}

fn validate_stored_crc32_metadata(
    payload: &[u8],
    uncompressed_size: u64,
    encrypted: bool,
    expected_crc32: u32,
) -> Result<()> {
    let uncompressed_len = usize::try_from(uncompressed_size).map_err(|_| {
        Error::Io(io::Error::new(
            io::ErrorKind::InvalidInput,
            "uncompressed size overflows usize",
        ))
    })?;
    let actual_crc32 = if encrypted {
        let mut sink = Crc32Sink::new();
        decompress::decode_reader_to_writer(
            crypto::DecryptReader::new(payload),
            CompressionMethod::Store,
            uncompressed_len,
            &mut sink,
        )?;
        sink.finish()
    } else {
        svarog_common::crc::hash_bytes(payload)
    };
    if actual_crc32 != expected_crc32 {
        return Err(Error::Io(io::Error::new(
            io::ErrorKind::InvalidInput,
            "stored payload CRC32C metadata does not match uncompressed bytes",
        )));
    }
    Ok(())
}

fn validate_memory_payload_metadata(
    payload: &[u8],
    uncompressed_size: u64,
    method: CompressionMethod,
    encrypted: bool,
    expected_crc32: u32,
    expected_sha256: [u8; 32],
) -> Result<()> {
    let actual_sha256 = sha256_array(payload);
    if actual_sha256 != expected_sha256 {
        return Err(Error::Io(io::Error::new(
            io::ErrorKind::InvalidInput,
            "payload SHA-256 metadata does not match raw payload bytes",
        )));
    }

    let uncompressed_len = usize::try_from(uncompressed_size).map_err(|_| {
        Error::Io(io::Error::new(
            io::ErrorKind::InvalidInput,
            "uncompressed size overflows usize",
        ))
    })?;
    let actual_crc32 = crc32_memory_payload(payload, uncompressed_len, method, encrypted)?;
    if actual_crc32 != expected_crc32 {
        return Err(Error::Io(io::Error::new(
            io::ErrorKind::InvalidInput,
            "payload CRC32C metadata does not match uncompressed bytes",
        )));
    }
    Ok(())
}

fn crc32_memory_payload(
    payload: &[u8],
    uncompressed_len: usize,
    method: CompressionMethod,
    encrypted: bool,
) -> Result<u32> {
    if method == CompressionMethod::Store && !encrypted && payload.len() != uncompressed_len {
        return Err(Error::Io(io::Error::new(
            io::ErrorKind::InvalidInput,
            "stored payload size must match uncompressed_size",
        )));
    }

    let mut sink = Crc32Sink::new();
    if encrypted {
        decompress::decode_reader_to_writer(
            crypto::DecryptReader::new(payload),
            method,
            uncompressed_len,
            &mut sink,
        )?;
    } else {
        decompress::decode_to_writer(payload, method, uncompressed_len, &mut sink)?;
    }
    Ok(sink.finish())
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
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.crc32 = svarog_common::crc::hash_bytes_with_seed(buf, self.crc32);
        self.len = self.len.checked_add(buf.len() as u64).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "decoded payload too large")
        })?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[derive(Debug)]
struct Crc32Reader<R> {
    inner: R,
    crc32: u32,
    len: u64,
}

impl<R> Crc32Reader<R> {
    fn new(inner: R) -> Self {
        Self {
            inner,
            crc32: 0,
            len: 0,
        }
    }

    fn into_parts(self) -> (u64, u32) {
        (self.len, self.crc32)
    }
}

impl<R: Read> Read for Crc32Reader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let read = self.inner.read(buf)?;
        if read == 0 {
            return Ok(0);
        }

        self.crc32 = svarog_common::crc::hash_bytes_with_seed(&buf[..read], self.crc32);
        self.len = self
            .len
            .checked_add(read as u64)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "file too large"))?;
        Ok(read)
    }
}

fn copy_file_payload_validated_sha256<W: Write>(
    path: &Path,
    expected_len: u64,
    crc32: u32,
    expected_sha256: [u8; 32],
    writer: &mut W,
) -> Result<PayloadWriteMetadata> {
    let metadata = fs::metadata(path)?;
    if metadata.len() != expected_len {
        return Err(staged_file_changed_error(path));
    }

    let mut file = File::open(path)?;
    let mut sha256 = Sha256::new();
    let mut remaining = expected_len;
    let mut buffer = vec![0u8; COPY_BUFFER_SIZE];
    while remaining > 0 {
        let limit = remaining.min(COPY_BUFFER_SIZE as u64) as usize;
        let read = file.read(&mut buffer[..limit])?;
        if read == 0 {
            return Err(staged_file_changed_error(path));
        }
        sha256.update(&buffer[..read]);
        writer.write_all(&buffer[..read])?;
        remaining -= read as u64;
    }

    let mut extra = [0u8; 1];
    if file.read(&mut extra)? != 0 {
        return Err(staged_file_changed_error(path));
    }

    if finish_sha256(sha256) != expected_sha256 {
        return Err(staged_file_changed_error(path));
    }

    Ok(PayloadWriteMetadata {
        len: expected_len,
        crc32,
        sha256: expected_sha256,
    })
}

fn copy_file_payload_hashed<W: Write>(
    path: &Path,
    expected_len: u64,
    writer: &mut W,
) -> Result<PayloadWriteMetadata> {
    let metadata = fs::metadata(path)?;
    if metadata.len() != expected_len {
        return Err(staged_file_changed_error(path));
    }

    let mut file = File::open(path)?;
    let mut crc32 = 0u32;
    let mut sha256 = Sha256::new();
    let mut remaining = expected_len;
    let mut buffer = vec![0u8; COPY_BUFFER_SIZE];
    while remaining > 0 {
        let limit = remaining.min(COPY_BUFFER_SIZE as u64) as usize;
        let read = file.read(&mut buffer[..limit])?;
        if read == 0 {
            return Err(staged_file_changed_error(path));
        }
        let chunk = &buffer[..read];
        crc32 = svarog_common::crc::hash_bytes_with_seed(chunk, crc32);
        sha256.update(chunk);
        writer.write_all(chunk)?;
        remaining -= read as u64;
    }

    let mut extra = [0u8; 1];
    if file.read(&mut extra)? != 0 {
        return Err(staged_file_changed_error(path));
    }

    Ok(PayloadWriteMetadata {
        len: expected_len,
        crc32,
        sha256: finish_sha256(sha256),
    })
}

fn hash_reader(reader: &mut File) -> Result<(u64, u32, [u8; 32])> {
    const BUFFER_SIZE: usize = 128 * 1024;

    let mut buffer = [0u8; BUFFER_SIZE];
    let mut total = 0u64;
    let mut crc32 = 0u32;
    let mut sha256 = Sha256::new();

    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }

        let chunk = &buffer[..read];
        crc32 = svarog_common::crc::hash_bytes_with_seed(chunk, crc32);
        sha256.update(chunk);
        total = total.checked_add(read as u64).ok_or_else(|| {
            Error::Io(io::Error::new(
                io::ErrorKind::InvalidInput,
                "file too large",
            ))
        })?;
    }

    Ok((total, crc32, finish_sha256(sha256)))
}

fn staged_file_changed_error(path: &Path) -> Error {
    Error::Io(io::Error::new(
        io::ErrorKind::InvalidInput,
        format!(
            "staged file changed before archive write: {}",
            path.display()
        ),
    ))
}

fn staged_file_changed_error_for_entry(entry: &BuilderEntry) -> Error {
    match &entry.payload {
        PayloadSource::File { path, .. } => staged_file_changed_error(path),
        PayloadSource::Memory(_) => Error::Io(io::Error::new(
            io::ErrorKind::InvalidInput,
            "payload metadata changed before archive write",
        )),
    }
}

fn compress_file_payload(
    path: &Path,
    method: CompressionMethod,
    options: &P4kWriterOptions,
) -> Result<(PayloadSource, u64, u32, [u8; 32])> {
    let (temp_path, temp_file) = create_temp_payload_file()?;
    let result = (|| {
        let sink = HashingFileWriter::new(temp_file);
        let (uncompressed_len, crc32, sink) = compress_file_to_writer(path, method, options, sink)?;
        let (_file, compressed_len, sha256) = sink.into_parts();
        Ok((
            PayloadSource::File {
                path: temp_path.clone(),
                len: compressed_len,
                validation: FileValidation::Sha256(sha256),
                cleanup: true,
            },
            uncompressed_len,
            crc32,
            sha256,
        ))
    })();

    if result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }

    result
}

fn encrypt_file_payload(
    path: &Path,
    method: CompressionMethod,
    options: &P4kWriterOptions,
) -> Result<(PayloadSource, u64, u32, [u8; 32])> {
    if method == CompressionMethod::Store {
        return encrypt_stored_file_payload(path);
    }

    compress_encrypt_file_payload(path, method, options)
}

fn encrypt_stored_file_payload(path: &Path) -> Result<(PayloadSource, u64, u32, [u8; 32])> {
    let (temp_path, temp_file) = create_temp_payload_file()?;
    let result = (|| {
        let input = File::open(path)?;
        let mut input = Crc32Reader::new(input);
        let mut sink = HashingFileWriter::new(temp_file);
        let encrypted_len = crypto::encrypt_reader_to_writer(&mut input, &mut sink)?;
        let (uncompressed_len, crc32) = input.into_parts();
        sink.flush()?;
        let (_file, hashed_len, sha256) = sink.into_parts();
        if encrypted_len != hashed_len {
            return Err(Error::Io(io::Error::new(
                io::ErrorKind::InvalidData,
                "encrypted staged payload length mismatch",
            )));
        }
        Ok((
            PayloadSource::File {
                path: temp_path.clone(),
                len: encrypted_len,
                validation: FileValidation::Sha256(sha256),
                cleanup: true,
            },
            uncompressed_len,
            crc32,
            sha256,
        ))
    })();

    if result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }

    result
}

fn compress_encrypt_file_payload(
    path: &Path,
    method: CompressionMethod,
    options: &P4kWriterOptions,
) -> Result<(PayloadSource, u64, u32, [u8; 32])> {
    let (temp_path, temp_file) = create_temp_payload_file()?;
    let result = (|| {
        let sink = HashingFileWriter::new(temp_file);
        let encrypting = crypto::EncryptWriter::new(sink);
        let (uncompressed_len, crc32, encrypting) =
            compress_file_to_writer(path, method, options, encrypting)?;
        let (sink, encrypted_len) = encrypting.finish()?;
        let (_file, hashed_len, sha256) = sink.into_parts();
        if encrypted_len != hashed_len {
            return Err(Error::Io(io::Error::new(
                io::ErrorKind::InvalidData,
                "encrypted staged payload length mismatch",
            )));
        }
        Ok((
            PayloadSource::File {
                path: temp_path.clone(),
                len: encrypted_len,
                validation: FileValidation::Sha256(sha256),
                cleanup: true,
            },
            uncompressed_len,
            crc32,
            sha256,
        ))
    })();

    if result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }

    result
}

fn compress_file_to_writer<W: Write>(
    path: &Path,
    method: CompressionMethod,
    options: &P4kWriterOptions,
    sink: W,
) -> Result<(u64, u32, W)> {
    match method {
        CompressionMethod::Store => unreachable!("stored files use direct file-backed payloads"),
        CompressionMethod::Deflate => {
            let encoder =
                DeflateEncoder::new(sink, flate2::Compression::new(options.deflate_level));
            compress_file_through_encoder(path, encoder)
        }
        CompressionMethod::DeflateZlib => {
            let encoder = ZlibEncoder::new(sink, flate2::Compression::new(options.deflate_level));
            compress_file_through_encoder(path, encoder)
        }
        CompressionMethod::Zstd | CompressionMethod::ZstdDeprecated => {
            let encoder = zstd::stream::Encoder::new(sink, options.zstd_level)?;
            compress_file_through_encoder(path, encoder)
        }
    }
}

trait FinishCompression {
    type Writer;

    fn finish(self) -> io::Result<Self::Writer>;
}

impl<W: Write> FinishCompression for DeflateEncoder<W> {
    type Writer = W;

    fn finish(self) -> io::Result<Self::Writer> {
        DeflateEncoder::finish(self)
    }
}

impl<W: Write> FinishCompression for ZlibEncoder<W> {
    type Writer = W;

    fn finish(self) -> io::Result<Self::Writer> {
        ZlibEncoder::finish(self)
    }
}

impl<'a, W: Write> FinishCompression for zstd::stream::Encoder<'a, W> {
    type Writer = W;

    fn finish(self) -> io::Result<Self::Writer> {
        zstd::stream::Encoder::finish(self)
    }
}

fn compress_file_through_encoder<E>(path: &Path, mut encoder: E) -> Result<(u64, u32, E::Writer)>
where
    E: Write + FinishCompression,
{
    const BUFFER_SIZE: usize = 128 * 1024;

    let mut input = File::open(path)?;
    let mut buffer = [0u8; BUFFER_SIZE];
    let mut total = 0u64;
    let mut crc32 = 0u32;

    loop {
        let read = input.read(&mut buffer)?;
        if read == 0 {
            break;
        }

        let chunk = &buffer[..read];
        crc32 = svarog_common::crc::hash_bytes_with_seed(chunk, crc32);
        total = total.checked_add(read as u64).ok_or_else(|| {
            Error::Io(io::Error::new(
                io::ErrorKind::InvalidInput,
                "file too large",
            ))
        })?;
        encoder.write_all(chunk)?;
    }

    Ok((total, crc32, encoder.finish()?))
}

#[derive(Debug)]
struct HashingFileWriter<W> {
    inner: W,
    sha256: Sha256,
    len: u64,
}

impl<W> HashingFileWriter<W> {
    fn new(inner: W) -> Self {
        Self {
            inner,
            sha256: Sha256::new(),
            len: 0,
        }
    }

    fn into_parts(self) -> (W, u64, [u8; 32]) {
        (self.inner, self.len, finish_sha256(self.sha256))
    }
}

impl<W: Write> Write for HashingFileWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let written = self.inner.write(buf)?;
        self.sha256.update(&buf[..written]);
        self.len = self.len.checked_add(written as u64).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "compressed payload too large")
        })?;
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

fn create_temp_payload_file() -> Result<(PathBuf, File)> {
    for _ in 0..1024 {
        let counter = TEMP_PAYLOAD_COUNTER.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!(
            "svarog-p4k-payload-{}-{nanos}-{counter}.tmp",
            std::process::id()
        ));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => return Ok((path, file)),
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(Error::Io(err)),
        }
    }

    Err(Error::Io(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not create unique temporary payload file",
    )))
}

fn write_v1_local_record<W: Write>(
    writer: &mut W,
    entry: &BuilderEntry,
    payload_metadata: PayloadWriteMetadata,
    local_header_offset: u64,
    sector_size: u64,
) -> Result<()> {
    let name = entry.name.as_bytes();
    checked_v1_local_record_size_for_entry(entry, sector_size)?;

    let local_header_size = 4 + std::mem::size_of::<LocalFileHeader>();
    let extra_len = sector_size as usize - local_header_size - name.len();
    let record_size = v1_local_file_record_size(name.len() as u64, sector_size)?;

    writer.write_all(&LocalFileHeader::SIGNATURE.to_le_bytes())?;
    writer.write_all(&45u16.to_le_bytes())?;
    writer.write_all(&0u16.to_le_bytes())?;
    writer.write_all(&(entry.compression_method as u16).to_le_bytes())?;
    writer.write_all(&entry.last_mod_file_time.to_le_bytes())?;
    writer.write_all(&entry.last_mod_file_date.to_le_bytes())?;
    writer.write_all(&payload_metadata.crc32.to_le_bytes())?;
    writer.write_all(&u32::MAX.to_le_bytes())?;
    writer.write_all(&u32::MAX.to_le_bytes())?;
    writer.write_all(&(name.len() as u16).to_le_bytes())?;
    writer.write_all(&(extra_len as u16).to_le_bytes())?;
    writer.write_all(name)?;

    let zip64 = P4kZip64ExtraField {
        id: extra_field::ZIP64,
        size: extra_field::ZIP64_TOTAL_SIZE,
        uncompressed_size: entry.uncompressed_size,
        compressed_size: payload_metadata.len,
        local_header_offset,
        disk_number_start: 0,
    };
    writer.write_all(zip64.as_bytes())?;

    let dummy_total_size = extra_len - std::mem::size_of::<P4kZip64ExtraField>();
    writer.write_all(&0x0666u16.to_le_bytes())?;
    writer.write_all(&(dummy_total_size as u16).to_le_bytes())?;
    write_zeroes(writer, (dummy_total_size - 4) as u64)?;

    write_zeroes(writer, record_size - sector_size)?;

    Ok(())
}

fn checked_v1_local_record_size_for_entry(entry: &BuilderEntry, sector_size: u64) -> Result<u64> {
    let name = entry.name.as_bytes();
    if name.len() > u16::MAX as usize {
        return Err(Error::Io(io::Error::new(
            io::ErrorKind::InvalidInput,
            "v1 entry name is too long",
        )));
    }
    let sector_size = usize::try_from(sector_size).map_err(|_| {
        Error::Io(io::Error::new(
            io::ErrorKind::InvalidInput,
            "v1 sector size overflows usize",
        ))
    })?;

    let local_header_size = 4 + std::mem::size_of::<LocalFileHeader>();
    if local_header_size + name.len() >= sector_size {
        return Err(Error::Io(io::Error::new(
            io::ErrorKind::InvalidInput,
            "v1 entry name does not fit in the local header sector",
        )));
    }

    let extra_len = sector_size - local_header_size - name.len();
    if extra_len < std::mem::size_of::<P4kZip64ExtraField>() + 4 {
        return Err(Error::Io(io::Error::new(
            io::ErrorKind::InvalidInput,
            "v1 local header sector is too small",
        )));
    }
    v1_local_file_record_size(name.len() as u64, sector_size as u64)
}

fn write_v1_central_directory_entry<W: Write>(
    writer: &mut W,
    entry: &BuilderEntry,
    payload_metadata: PayloadWriteMetadata,
    local_header_offset: u64,
) -> Result<()> {
    let name = entry.name.as_bytes();
    if name.len() > u16::MAX as usize {
        return Err(Error::Io(io::Error::new(
            io::ErrorKind::InvalidInput,
            "v1 entry name is too long",
        )));
    }

    let dos_datetime = ((entry.last_mod_file_date as u32) << 16) | entry.last_mod_file_time as u32;
    writer.write_all(&0x0201_4b50u32.to_le_bytes())?;
    writer.write_all(&46u16.to_le_bytes())?;
    writer.write_all(&45u16.to_le_bytes())?;
    writer.write_all(&0u16.to_le_bytes())?;
    writer.write_all(&(entry.compression_method as u16).to_le_bytes())?;
    writer.write_all(&dos_datetime.to_le_bytes())?;
    writer.write_all(&payload_metadata.crc32.to_le_bytes())?;
    writer.write_all(&u32::MAX.to_le_bytes())?;
    writer.write_all(&u32::MAX.to_le_bytes())?;
    writer.write_all(&(name.len() as u16).to_le_bytes())?;
    writer.write_all(&0xCEu16.to_le_bytes())?;
    writer.write_all(&0u16.to_le_bytes())?;
    writer.write_all(&u16::MAX.to_le_bytes())?;
    writer.write_all(&0u16.to_le_bytes())?;
    writer.write_all(&0u32.to_le_bytes())?;
    writer.write_all(&u32::MAX.to_le_bytes())?;
    writer.write_all(name)?;

    let zip64 = P4kZip64ExtraField {
        id: extra_field::ZIP64,
        size: extra_field::ZIP64_TOTAL_SIZE,
        uncompressed_size: entry.uncompressed_size,
        compressed_size: payload_metadata.len,
        local_header_offset,
        disk_number_start: 0,
    };
    writer.write_all(zip64.as_bytes())?;

    let signature = P4kSignatureExtraField {
        id: extra_field::P4K_5000,
        size: extra_field::P4K_5000_TOTAL_SIZE,
        signature: entry.signature,
    };
    writer.write_all(signature.as_bytes())?;

    let encryption = P4kEncryptionExtraField {
        id: extra_field::P4K_5002,
        size: extra_field::P4K_5002_TOTAL_SIZE,
        encryption: u16::from(entry.encrypted),
    };
    writer.write_all(encryption.as_bytes())?;

    let sha256 = P4kSha256ExtraField {
        id: extra_field::P4K_5003,
        size: extra_field::P4K_5003_TOTAL_SIZE,
        sha256: payload_metadata.sha256,
    };
    writer.write_all(sha256.as_bytes())?;

    Ok(())
}

fn write_v1_zip64_eocd<W: Write>(
    writer: &mut W,
    entry_count: u64,
    cdr_size: u64,
    cdr_offset: u64,
) -> Result<()> {
    writer.write_all(&0x0606_4b50u32.to_le_bytes())?;
    writer.write_all(&44u64.to_le_bytes())?;
    writer.write_all(&46u16.to_le_bytes())?;
    writer.write_all(&45u16.to_le_bytes())?;
    writer.write_all(&0u32.to_le_bytes())?;
    writer.write_all(&0u32.to_le_bytes())?;
    writer.write_all(&entry_count.to_le_bytes())?;
    writer.write_all(&entry_count.to_le_bytes())?;
    writer.write_all(&cdr_size.to_le_bytes())?;
    writer.write_all(&cdr_offset.to_le_bytes())?;
    Ok(())
}

fn write_v1_zip64_locator<W: Write>(writer: &mut W, zip64_eocd_offset: u64) -> Result<()> {
    writer.write_all(&0x0706_4b50u32.to_le_bytes())?;
    writer.write_all(&0u32.to_le_bytes())?;
    writer.write_all(&zip64_eocd_offset.to_le_bytes())?;
    writer.write_all(&1u32.to_le_bytes())?;
    Ok(())
}

fn write_v1_eocd<W: Write>(writer: &mut W, sector_size: u64, freelist_blocks: u64) -> Result<()> {
    writer.write_all(&0x0605_4b50u32.to_le_bytes())?;
    writer.write_all(&u16::MAX.to_le_bytes())?;
    writer.write_all(&u16::MAX.to_le_bytes())?;
    writer.write_all(&u16::MAX.to_le_bytes())?;
    writer.write_all(&u16::MAX.to_le_bytes())?;
    writer.write_all(&u32::MAX.to_le_bytes())?;
    writer.write_all(&u32::MAX.to_le_bytes())?;
    writer.write_all(&16u16.to_le_bytes())?;
    writer.write_all(b"CI")?;
    writer.write_all(&0x0001_0047u32.to_le_bytes())?;
    writer.write_all(&(sector_size as u16).to_le_bytes())?;
    writer.write_all(&freelist_blocks.to_le_bytes())?;
    Ok(())
}

fn normalize_archive_name(name: &str) -> Result<String> {
    if name.is_empty() || name.as_bytes().contains(&0) {
        return Err(Error::Io(io::Error::new(
            io::ErrorKind::InvalidInput,
            "archive entry name is empty or contains NUL",
        )));
    }

    let normalized = normalize_p4k_path(name, false);
    if normalized.is_empty() || normalized.starts_with('/') {
        return Err(Error::Io(io::Error::new(
            io::ErrorKind::InvalidInput,
            "archive entry name must be relative and normalized",
        )));
    }
    if normalized
        .split('/')
        .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(Error::Io(io::Error::new(
            io::ErrorKind::InvalidInput,
            "archive entry name must be relative and normalized",
        )));
    }

    Ok(normalized)
}

fn normalize_p4k_path(path: &str, lowercase: bool) -> String {
    let mut out = Vec::with_capacity(path.len());
    for &byte in path.as_bytes() {
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

fn safe_output_path(root: &Path, archive_name: &str) -> Result<PathBuf> {
    let mut out = PathBuf::from(root);
    for part in archive_name.replace('\\', "/").split('/') {
        if part.is_empty() || part == "." || part == ".." || part.contains(':') {
            return Err(Error::Io(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("unsafe archive path: {archive_name}"),
            )));
        }
        out.push(part);
    }
    Ok(out)
}

fn validate_options(options: &P4kWriterOptions) -> Result<()> {
    if options.sector_size == 0 || !options.sector_size.is_power_of_two() {
        return Err(Error::Io(io::Error::new(
            io::ErrorKind::InvalidInput,
            "sector size must be a non-zero power of two",
        )));
    }
    Ok(())
}

fn align_up(value: u64, align: u64) -> Result<u64> {
    debug_assert!(align.is_power_of_two());
    value
        .checked_add(align - 1)
        .map(|v| v & !(align - 1))
        .ok_or_else(|| {
            Error::Io(io::Error::new(
                io::ErrorKind::InvalidInput,
                "alignment overflow",
            ))
        })
}

fn usize_to_u64(value: usize, what: &'static str) -> Result<u64> {
    u64::try_from(value).map_err(|_| {
        Error::Io(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{what} overflows u64"),
        ))
    })
}

fn dos_datetime_from_system_time(time: SystemTime) -> Option<(u16, u16)> {
    let seconds = time.duration_since(UNIX_EPOCH).ok()?.as_secs();
    Some(dos_datetime_from_unix_secs(seconds))
}

fn dos_datetime_from_unix_secs(seconds: u64) -> (u16, u16) {
    let days = (seconds / 86_400) as i64;
    let seconds_of_day = seconds % 86_400;
    let (year, month, day) = civil_from_days(days);
    let hour = (seconds_of_day / 3_600) as u16;
    let minute = ((seconds_of_day % 3_600) / 60) as u16;
    let second = (seconds_of_day % 60) as u16;

    if year < 1980 {
        return (0, (1 << 5) | 1);
    }

    let dos_year = (year - 1980).min(127) as u16;
    let dos_time = (hour << 11) | (minute << 5) | (second / 2);
    let dos_date = (dos_year << 9) | ((month as u16) << 5) | day as u16;
    (dos_time, dos_date)
}

fn civil_from_days(days_since_unix_epoch: i64) -> (i32, u32, u32) {
    let z = days_since_unix_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    let year = y + if month <= 2 { 1 } else { 0 };
    (year as i32, month as u32, day as u32)
}

fn v1_local_file_record_size(name_len: u64, sector_size: u64) -> Result<u64> {
    let unaligned = name_len
        .checked_add(sector_size)
        .and_then(|value| value.checked_add(0x3D))
        .ok_or_else(|| {
            Error::Io(io::Error::new(
                io::ErrorKind::InvalidInput,
                "v1 local file record size overflow",
            ))
        })?;
    align_up(unaligned, sector_size)
}

fn write_zeroes<W: Write>(writer: &mut W, mut count: u64) -> Result<()> {
    const ZEROES: [u8; 8192] = [0; 8192];
    while count > 0 {
        let n = count.min(ZEROES.len() as u64) as usize;
        writer.write_all(&ZEROES[..n])?;
        count -= n as u64;
    }
    Ok(())
}

fn sha256_array(bytes: &[u8]) -> [u8; 32] {
    let digest = Sha256::digest(bytes);
    sha256_digest_to_array(digest)
}

fn finish_sha256(hasher: Sha256) -> [u8; 32] {
    sha256_digest_to_array(hasher.finalize())
}

fn sha256_digest_to_array(digest: impl AsRef<[u8]>) -> [u8; 32] {
    let mut out = [0; 32];
    out.copy_from_slice(digest.as_ref());
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::P4kRawEntryRef;

    #[test]
    fn default_compression_levels_match_dump() {
        let options = P4kWriterOptions::default();
        assert_eq!(options.compression, CompressionMethod::Zstd);
        assert_eq!(options.zstd_level, 1);
        assert_eq!(options.deflate_level, 9);
    }

    #[test]
    fn max_v1_payload_end_ignores_empty_entries_and_checks_overflow() {
        let entries = vec![
            test_raw_entry("empty.bin", 900, 0),
            test_raw_entry("small.bin", 100, 20),
            test_raw_entry("large.bin", 4096, 128),
        ];
        assert_eq!(max_v1_payload_end(&entries).unwrap(), 4224);

        let overflowing = vec![test_raw_entry("overflow.bin", u64::MAX, 1)];
        let err = max_v1_payload_end(&overflowing).unwrap_err();
        assert!(err.to_string().contains("source v1 payload range overflow"));
    }

    #[test]
    fn fresh_writer_orders_equal_size_entries_by_sorted_filename_handle() {
        let options = P4kWriterOptions {
            compression: CompressionMethod::Store,
            sector_size: 256,
            ..Default::default()
        };

        let write_names = |names: &[&str]| {
            let mut builder = P4kBuilder::with_options(options.clone());
            for name in names {
                builder.add_bytes(*name, b"same").unwrap();
            }
            let ordered_names: Vec<_> = builder
                .entries_for_fresh_write()
                .into_iter()
                .map(|entry| entry.name.clone())
                .collect();
            let mut output = std::io::Cursor::new(Vec::new());
            builder.write_to(&mut output).unwrap();
            (ordered_names, output.into_inner())
        };

        let (forward_names, forward_bytes) = write_names(&["z.bin", "A.bin", "m.bin"]);
        let (reverse_names, reverse_bytes) = write_names(&["m.bin", "z.bin", "A.bin"]);

        assert_eq!(forward_names, ["A.bin", "m.bin", "z.bin"]);
        assert_eq!(reverse_names, forward_names);
        assert_eq!(reverse_bytes, forward_bytes);
    }

    #[test]
    fn fresh_filename_handles_use_sorted_lowercase_normalized_names() {
        let mut builder = P4kBuilder::new();
        builder.add_bytes("Z.bin", b"same").unwrap();
        builder.add_bytes("a.bin", b"same").unwrap();
        builder.add_bytes("Data\\File.bin", b"same").unwrap();
        builder.add_bytes("data/file.bin", b"same").unwrap();

        let handles = builder.fresh_filename_handles();
        assert!(handles[1] < handles[2]);
        assert!(handles[2] < handles[0]);
        assert_eq!(handles[2], handles[3]);
    }

    #[test]
    fn path_normalization_matches_cig_inplace_rules() {
        assert_eq!(
            normalize_p4k_path("Dir\\./Sub//Leaf.txt   ", false),
            "Dir/Sub/Leaf.txt"
        );
        assert_eq!(
            normalize_p4k_path("Dir/Sub/../Leaf.txt", false),
            "Dir/Leaf.txt"
        );
        assert_eq!(
            normalize_p4k_path("Dir\\Mixed/Case.txt", true),
            "dir/mixed/case.txt"
        );
        assert_eq!(normalize_p4k_path("../escape.bin", false), ".escape.bin");
        assert_eq!(
            normalize_archive_name("../escape.bin").unwrap(),
            ".escape.bin"
        );
    }

    #[test]
    fn writer_uses_cig_crc32c_for_p4k_metadata() {
        let mut builder = P4kBuilder::new();
        builder.add_bytes("crc.bin", b"123456789").unwrap();

        assert_eq!(svarog_common::crc::hash_bytes(b"123456789"), 0xE306_9283);
        assert_eq!(builder.entries[0].crc32, Some(0xE306_9283));
        assert_ne!(builder.entries[0].crc32, Some(0xCBF4_3926));
    }

    #[test]
    fn add_bytes_with_entry_metadata_preserves_signature_and_dos_time() {
        let v2 = temp_writer_path("svarog_p4k_signed_bytes_v2.p4k");
        let v1 = temp_writer_path("svarog_p4k_signed_bytes_v1.p4k");
        let payload = b"signed payload";
        let crc32 = svarog_common::crc::hash_bytes(payload);
        let digest = crypto::signature_metadata_sha256(
            "Data\\Signed.bin",
            crc32,
            payload.len() as u64,
            payload.len() as u64,
            true,
        );
        let mut signature = [0xA5; 128];
        signature[..digest.len()].copy_from_slice(&digest);
        let metadata = P4kEntryMetadata {
            last_mod_file_time: 0x1234,
            last_mod_file_date: 0x5678,
            signature,
        };

        let options = P4kWriterOptions {
            compression: CompressionMethod::Store,
            ..Default::default()
        };
        let mut builder = P4kBuilder::with_options(options);
        builder
            .add_bytes_with_entry_metadata(
                "Data\\Signed.bin",
                payload,
                CompressionMethod::Store,
                false,
                metadata,
            )
            .unwrap();

        builder.write_to_file(&v2).unwrap();
        let archive = P4kArchive::open(&v2).unwrap();
        let entry = archive.find("Data/Signed.bin").unwrap();
        assert_eq!(entry.last_mod_file_time, metadata.last_mod_file_time);
        assert_eq!(entry.last_mod_file_date, metadata.last_mod_file_date);
        assert_eq!(entry.signature, metadata.signature);
        assert_eq!(archive.read(&entry).unwrap(), payload);

        builder.write_v1_to_file(&v1).unwrap();
        let archive = P4kArchive::open(&v1).unwrap();
        let entry = archive.find("Data/Signed.bin").unwrap();
        assert_eq!(entry.last_mod_file_time, metadata.last_mod_file_time);
        assert_eq!(entry.last_mod_file_date, metadata.last_mod_file_date);
        assert_eq!(entry.signature, metadata.signature);
        assert_eq!(archive.read(&entry).unwrap(), payload);

        let _ = std::fs::remove_file(&v2);
        let _ = std::fs::remove_file(&v1);
    }

    #[test]
    fn add_file_with_entry_metadata_preserves_signature_and_dos_time() {
        let input = temp_writer_path("svarog_p4k_signed_file_input.bin");
        let v2 = temp_writer_path("svarog_p4k_signed_file_v2.p4k");
        let payload = b"filesystem signed payload";
        std::fs::write(&input, payload).unwrap();

        let metadata = P4kEntryMetadata {
            last_mod_file_time: 0x2468,
            last_mod_file_date: 0x1357,
            signature: [0x5A; 128],
        };
        let options = P4kWriterOptions {
            compression: CompressionMethod::Store,
            ..Default::default()
        };
        let mut builder = P4kBuilder::with_options(options);
        builder
            .add_file_with_entry_metadata(&input, "signed-file.bin", false, metadata)
            .unwrap();
        builder.write_to_file(&v2).unwrap();

        let archive = P4kArchive::open(&v2).unwrap();
        let entry = archive.find("signed-file.bin").unwrap();
        assert_eq!(entry.last_mod_file_time, metadata.last_mod_file_time);
        assert_eq!(entry.last_mod_file_date, metadata.last_mod_file_date);
        assert_eq!(entry.signature, metadata.signature);
        assert_eq!(archive.read(&entry).unwrap(), payload);

        let _ = std::fs::remove_file(&input);
        let _ = std::fs::remove_file(&v2);
    }

    #[test]
    fn add_file_store_uses_file_backed_payload_and_streamed_hashes() {
        let input = temp_writer_path("svarog_p4k_file_backed_store_input.bin");
        let v2 = temp_writer_path("svarog_p4k_file_backed_store_v2.p4k");
        let v1 = temp_writer_path("svarog_p4k_file_backed_store_v1.p4k");
        let payload = b"file backed store";
        std::fs::write(&input, payload).unwrap();

        let options = P4kWriterOptions {
            compression: CompressionMethod::Store,
            sector_size: 256,
            ..Default::default()
        };
        let mut builder = P4kBuilder::with_options(options);
        builder.add_file(&input, "streamed.bin").unwrap();

        match &builder.entries[0].payload {
            PayloadSource::File {
                path,
                len,
                cleanup,
                validation: FileValidation::SizeOnly,
            } => {
                assert_eq!(path, &input);
                assert_eq!(*len, payload.len() as u64);
                assert!(!cleanup);
            }
            other => panic!("stored filesystem payload should be lazy file-backed: {other:?}"),
        }
        assert_eq!(builder.entries[0].crc32, None);
        assert_eq!(builder.entries[0].sha256, None);

        builder.write_to_file(&v2).unwrap();
        let archive = P4kArchive::open(&v2).unwrap();
        assert_eq!(archive.read(&archive.get(0).unwrap()).unwrap(), payload);

        builder.write_v1_to_file(&v1).unwrap();
        let archive = P4kArchive::open(&v1).unwrap();
        assert_eq!(archive.read(&archive.get(0).unwrap()).unwrap(), payload);

        let _ = std::fs::remove_file(&input);
        let _ = std::fs::remove_file(&v1);
        let _ = std::fs::remove_file(&v2);
    }

    #[test]
    fn staged_file_entries_append_without_intermediate_builders() {
        let input_a = temp_writer_path("svarog_p4k_staged_file_a.bin");
        let input_b = temp_writer_path("svarog_p4k_staged_file_b.bin");
        let output = temp_writer_path("svarog_p4k_staged_file_entries.p4k");
        std::fs::write(&input_a, b"alpha staged").unwrap();
        std::fs::write(&input_b, b"beta staged").unwrap();

        let options = P4kWriterOptions {
            compression: CompressionMethod::Store,
            sector_size: 256,
            ..Default::default()
        };
        let staged = vec![
            P4kBuilder::stage_file_with_options(&options, &input_a, "Data/A.bin", false).unwrap(),
            P4kBuilder::stage_file_with_options(&options, &input_b, "Data/B.bin", false).unwrap(),
        ];
        let mut builder = P4kBuilder::with_options(options);
        builder.append_staged(staged);

        assert_eq!(builder.len(), 2);
        builder.write_to_file(&output).unwrap();
        let archive = P4kArchive::open(&output).unwrap();
        assert_eq!(
            archive.read(&archive.find("Data/A.bin").unwrap()).unwrap(),
            b"alpha staged"
        );
        assert_eq!(
            archive.read(&archive.find("Data/B.bin").unwrap()).unwrap(),
            b"beta staged"
        );

        let _ = std::fs::remove_file(&input_a);
        let _ = std::fs::remove_file(&input_b);
        let _ = std::fs::remove_file(&output);
    }

    #[test]
    fn add_file_compressed_uses_streamed_temporary_payload() {
        let input = temp_writer_path("svarog_p4k_file_backed_compressed_input.bin");
        let v2 = temp_writer_path("svarog_p4k_file_backed_compressed_v2.p4k");
        let payload = b"streamed compressed filesystem payload ".repeat(1024);
        std::fs::write(&input, &payload).unwrap();

        let options = P4kWriterOptions {
            compression: CompressionMethod::Zstd,
            sector_size: 256,
            ..Default::default()
        };
        let mut builder = P4kBuilder::with_options(options);
        builder.add_file(&input, "streamed-zstd.bin").unwrap();

        let staged_path = match &builder.entries[0].payload {
            PayloadSource::File {
                path,
                len,
                cleanup,
                validation: FileValidation::Sha256(sha256),
            } => {
                assert_ne!(path, &input);
                assert!(*cleanup);
                assert!(path.exists());
                assert_eq!(*len, std::fs::metadata(path).unwrap().len());
                assert_eq!(Some(*sha256), builder.entries[0].sha256);
                path.clone()
            }
            other => panic!("compressed filesystem payload should be staged to a file: {other:?}"),
        };
        assert_eq!(
            builder.entries[0].crc32.unwrap(),
            svarog_common::crc::hash_bytes(&payload)
        );

        builder.write_to_file(&v2).unwrap();
        let archive = P4kArchive::open(&v2).unwrap();
        assert_eq!(archive.read(&archive.get(0).unwrap()).unwrap(), payload);

        drop(builder);
        assert!(
            !staged_path.exists(),
            "temporary compressed payload should be removed with the builder"
        );

        let _ = std::fs::remove_file(&input);
        let _ = std::fs::remove_file(&v2);
    }

    #[test]
    fn staged_temporary_payload_is_sha256_checked_while_copying() {
        let input = temp_writer_path("svarog_p4k_file_backed_compressed_corrupt_input.bin");
        let payload = b"temporary payload validation ".repeat(1024);
        std::fs::write(&input, &payload).unwrap();

        let options = P4kWriterOptions {
            compression: CompressionMethod::Zstd,
            sector_size: 256,
            ..Default::default()
        };
        let mut builder = P4kBuilder::with_options(options);
        builder.add_file(&input, "corrupt-temp.bin").unwrap();

        let staged_path = match &builder.entries[0].payload {
            PayloadSource::File {
                path,
                len,
                validation: FileValidation::Sha256(_),
                ..
            } => {
                std::fs::write(path, vec![0xA5; *len as usize]).unwrap();
                path.clone()
            }
            other => panic!("compressed filesystem payload should be staged to a file: {other:?}"),
        };

        let mut output = std::io::Cursor::new(Vec::new());
        let err = builder.write_to(&mut output).unwrap_err();
        assert!(
            err.to_string().contains("staged file changed"),
            "expected staged temporary payload mutation rejection, got {err}"
        );

        drop(builder);
        assert!(
            !staged_path.exists(),
            "temporary payload should still be removed with the builder"
        );
        let _ = std::fs::remove_file(&input);
    }

    #[test]
    fn add_file_encrypted_store_uses_streamed_temporary_payload() {
        let input = temp_writer_path("svarog_p4k_file_backed_encrypted_store_input.bin");
        let v2 = temp_writer_path("svarog_p4k_file_backed_encrypted_store_v2.p4k");
        let payload = b"streamed encrypted store payload with real zeroes\0\0";
        std::fs::write(&input, payload).unwrap();

        let options = P4kWriterOptions {
            compression: CompressionMethod::Store,
            sector_size: 256,
            ..Default::default()
        };
        let mut builder = P4kBuilder::with_options(options);
        builder
            .add_file_encrypted(&input, "encrypted-store.bin")
            .unwrap();

        let staged_path = match &builder.entries[0].payload {
            PayloadSource::File {
                path,
                len,
                cleanup,
                validation: FileValidation::Sha256(sha256),
            } => {
                assert_ne!(path, &input);
                assert!(*cleanup);
                assert!(path.exists());
                assert_eq!(*len, std::fs::metadata(path).unwrap().len());
                assert_eq!(*len % 16, 0);
                assert_eq!(Some(*sha256), builder.entries[0].sha256);
                path.clone()
            }
            other => panic!("encrypted filesystem payload should be staged to a file: {other:?}"),
        };
        assert!(builder.entries[0].encrypted);
        assert_eq!(
            builder.entries[0].crc32.unwrap(),
            svarog_common::crc::hash_bytes(payload)
        );

        builder.write_to_file(&v2).unwrap();
        let archive = P4kArchive::open(&v2).unwrap();
        assert_eq!(archive.read(&archive.get(0).unwrap()).unwrap(), payload);

        drop(builder);
        assert!(
            !staged_path.exists(),
            "temporary encrypted payload should be removed with the builder"
        );

        let _ = std::fs::remove_file(&input);
        let _ = std::fs::remove_file(&v2);
    }

    #[test]
    fn encrypted_stored_file_payload_hashes_plaintext_during_encryption() {
        let input = temp_writer_path("svarog_p4k_encrypted_store_one_pass_input.bin");
        let payload = b"encrypted stored payload crc source\0";
        std::fs::write(&input, payload).unwrap();

        let (source, uncompressed_len, crc32, sha256) =
            encrypt_stored_file_payload(&input).unwrap();
        let staged_path = match &source {
            PayloadSource::File {
                path,
                len,
                cleanup,
                validation: FileValidation::Sha256(validation_sha256),
            } => {
                assert!(*cleanup);
                assert_eq!(*validation_sha256, sha256);
                assert_eq!(*len, std::fs::metadata(path).unwrap().len());
                path.clone()
            }
            other => panic!("encrypted stored file should stage to a temp file: {other:?}"),
        };

        let expected_encrypted = crypto::encrypt(payload).unwrap();
        let actual_encrypted = std::fs::read(&staged_path).unwrap();
        assert_eq!(uncompressed_len, payload.len() as u64);
        assert_eq!(crc32, svarog_common::crc::hash_bytes(payload));
        assert_eq!(actual_encrypted, expected_encrypted);
        assert_eq!(sha256, sha256_array(&expected_encrypted));

        let _ = std::fs::remove_file(&input);
        drop(source);
        assert!(
            !staged_path.exists(),
            "temporary encrypted payload should be removed with the payload source"
        );
    }

    #[test]
    fn add_file_encrypted_compressed_uses_streamed_temporary_payload() {
        let input = temp_writer_path("svarog_p4k_file_backed_encrypted_compressed_input.bin");
        let v2 = temp_writer_path("svarog_p4k_file_backed_encrypted_compressed_v2.p4k");
        let payload = b"streamed encrypted compressed filesystem payload ".repeat(1024);
        std::fs::write(&input, &payload).unwrap();

        let options = P4kWriterOptions {
            compression: CompressionMethod::Zstd,
            sector_size: 256,
            ..Default::default()
        };
        let mut builder = P4kBuilder::with_options(options);
        builder
            .add_file_encrypted(&input, "encrypted-zstd.bin")
            .unwrap();

        let staged_path = match &builder.entries[0].payload {
            PayloadSource::File {
                path,
                len,
                cleanup,
                validation: FileValidation::Sha256(sha256),
            } => {
                assert_ne!(path, &input);
                assert!(*cleanup);
                assert!(path.exists());
                assert_eq!(*len, std::fs::metadata(path).unwrap().len());
                assert_eq!(*len % 16, 0);
                assert_eq!(Some(*sha256), builder.entries[0].sha256);
                path.clone()
            }
            other => panic!("encrypted filesystem payload should be staged to a file: {other:?}"),
        };
        assert!(builder.entries[0].encrypted);
        assert_eq!(
            builder.entries[0].compression_method,
            CompressionMethod::Zstd
        );
        assert_eq!(
            builder.entries[0].crc32.unwrap(),
            svarog_common::crc::hash_bytes(&payload)
        );

        builder.write_to_file(&v2).unwrap();
        let archive = P4kArchive::open(&v2).unwrap();
        assert_eq!(archive.read(&archive.get(0).unwrap()).unwrap(), payload);

        drop(builder);
        assert!(
            !staged_path.exists(),
            "temporary encrypted payload should be removed with the builder"
        );

        let _ = std::fs::remove_file(&input);
        let _ = std::fs::remove_file(&v2);
    }

    #[test]
    fn v2_file_backed_store_hashes_content_during_copy() {
        let input = temp_writer_path("svarog_p4k_file_backed_changed_input.bin");
        let output = temp_writer_path("svarog_p4k_file_backed_changed_v2.p4k");
        std::fs::write(&input, b"abcdef").unwrap();

        let options = P4kWriterOptions {
            compression: CompressionMethod::Store,
            sector_size: 256,
            ..Default::default()
        };
        let mut builder = P4kBuilder::with_options(options);
        builder.add_file(&input, "changed.bin").unwrap();
        std::fs::write(&input, b"ghijkl").unwrap();

        builder.write_to_file(&output).unwrap();
        let archive = P4kArchive::open(&output).unwrap();
        let entry = archive.find("changed.bin").unwrap();
        assert_eq!(archive.read(&entry).unwrap(), b"ghijkl");
        assert_eq!(entry.crc32, svarog_common::crc::hash_bytes(b"ghijkl"));
        assert_eq!(entry.sha256, sha256_array(b"ghijkl"));

        let _ = std::fs::remove_file(&input);
        let _ = std::fs::remove_file(&output);
    }

    #[test]
    fn v1_file_backed_store_hashes_content_during_copy_then_fills_local_header() {
        let input = temp_writer_path("svarog_p4k_file_backed_changed_input_v1.bin");
        let output = temp_writer_path("svarog_p4k_file_backed_changed_v1.p4k");
        std::fs::write(&input, b"abcdef").unwrap();

        let options = P4kWriterOptions {
            compression: CompressionMethod::Store,
            sector_size: 256,
            ..Default::default()
        };
        let mut builder = P4kBuilder::with_options(options);
        builder.add_file(&input, "changed.bin").unwrap();
        std::fs::write(&input, b"ghijkl").unwrap();

        builder.write_v1_to_file(&output).unwrap();
        let archive = P4kArchive::open(&output).unwrap();
        let entry = archive.find("changed.bin").unwrap();
        assert_eq!(archive.read(&entry).unwrap(), b"ghijkl");
        assert_eq!(entry.crc32, svarog_common::crc::hash_bytes(b"ghijkl"));
        assert_eq!(entry.sha256, sha256_array(b"ghijkl"));

        let _ = std::fs::remove_file(&input);
        let _ = std::fs::remove_file(&output);
    }

    #[test]
    fn write_to_file_preserves_existing_output_when_lazy_file_length_changes() {
        let input = temp_writer_path("svarog_p4k_file_create_changed_input.bin");
        let output = temp_writer_path("svarog_p4k_file_create_existing_v2.p4k");
        std::fs::write(&input, b"abcdef").unwrap();
        std::fs::write(&output, b"existing archive").unwrap();

        let options = P4kWriterOptions {
            compression: CompressionMethod::Store,
            sector_size: 256,
            ..Default::default()
        };
        let mut builder = P4kBuilder::with_options(options);
        builder.add_file(&input, "changed.bin").unwrap();
        std::fs::write(&input, b"ghijklm").unwrap();

        let err = builder.write_to_file(&output).unwrap_err();
        assert!(
            err.to_string().contains("staged file changed"),
            "expected staged file mutation rejection, got {err}"
        );
        assert_eq!(std::fs::read(&output).unwrap(), b"existing archive");
        assert_no_temp_output_leak(&output);

        let _ = std::fs::remove_file(&input);
        let _ = std::fs::remove_file(&output);
    }

    #[test]
    fn write_v1_to_file_preserves_existing_output_when_lazy_file_length_changes() {
        let input = temp_writer_path("svarog_p4k_file_create_changed_input_v1.bin");
        let output = temp_writer_path("svarog_p4k_file_create_existing_v1.p4k");
        std::fs::write(&input, b"abcdef").unwrap();
        std::fs::write(&output, b"existing archive").unwrap();

        let options = P4kWriterOptions {
            compression: CompressionMethod::Store,
            sector_size: 256,
            ..Default::default()
        };
        let mut builder = P4kBuilder::with_options(options);
        builder.add_file(&input, "changed.bin").unwrap();
        std::fs::write(&input, b"ghijklm").unwrap();

        let err = builder.write_v1_to_file(&output).unwrap_err();
        assert!(
            err.to_string().contains("staged file changed"),
            "expected staged file mutation rejection, got {err}"
        );
        assert_eq!(std::fs::read(&output).unwrap(), b"existing archive");
        assert_no_temp_output_leak(&output);

        let _ = std::fs::remove_file(&input);
        let _ = std::fs::remove_file(&output);
    }

    #[test]
    fn precompressed_payload_rejects_sha256_metadata_mismatch() {
        let mut builder = P4kBuilder::new();
        let mut wrong_sha256 = sha256_array(b"payload");
        wrong_sha256[0] ^= 0xFF;

        let err = builder
            .add_precompressed_with_metadata(
                "bad-sha.bin",
                b"payload".to_vec(),
                7,
                CompressionMethod::Store,
                svarog_common::crc::hash_bytes(b"payload"),
                0,
                0,
                false,
                [0; 128],
                wrong_sha256,
            )
            .unwrap_err();

        assert!(
            err.to_string().contains("payload SHA-256 metadata"),
            "expected SHA-256 metadata mismatch rejection, got {err}"
        );
        assert!(builder.is_empty());
    }

    #[test]
    fn precompressed_stored_payload_rejects_crc32_metadata_mismatch() {
        let mut builder = P4kBuilder::new();
        let payload = b"payload".to_vec();
        let sha256 = sha256_array(&payload);

        let err = builder
            .add_precompressed_with_metadata(
                "bad-crc.bin",
                payload,
                7,
                CompressionMethod::Store,
                0xDEAD_BEEF,
                0,
                0,
                false,
                [0; 128],
                sha256,
            )
            .unwrap_err();

        assert!(
            err.to_string().contains("CRC32C metadata"),
            "expected CRC32C metadata mismatch rejection, got {err}"
        );
        assert!(builder.is_empty());
    }

    #[test]
    fn precompressed_encrypted_stored_payload_rejects_crc32_metadata_mismatch() {
        let mut builder = P4kBuilder::new();
        let payload = crypto::encrypt(b"payload").unwrap();
        let sha256 = sha256_array(&payload);

        let err = builder
            .add_precompressed_with_metadata(
                "bad-encrypted-crc.bin",
                payload,
                7,
                CompressionMethod::Store,
                0xDEAD_BEEF,
                0,
                0,
                true,
                [0; 128],
                sha256,
            )
            .unwrap_err();

        assert!(
            err.to_string().contains("CRC32C metadata"),
            "expected encrypted CRC32C metadata mismatch rejection, got {err}"
        );
        assert!(builder.is_empty());
    }

    #[test]
    fn precompressed_compressed_payload_rejects_crc32_metadata_mismatch() {
        let mut builder = P4kBuilder::new();
        let plaintext = b"payload that should be deflated before storage";
        let payload =
            compress_bytes(plaintext, CompressionMethod::Deflate, &Default::default()).unwrap();
        let sha256 = sha256_array(&payload);

        let err = builder
            .add_precompressed_with_metadata(
                "bad-compressed-crc.bin",
                payload,
                plaintext.len() as u64,
                CompressionMethod::Deflate,
                0xDEAD_BEEF,
                0,
                0,
                false,
                [0; 128],
                sha256,
            )
            .unwrap_err();

        assert!(
            err.to_string().contains("CRC32C metadata"),
            "expected compressed CRC32C metadata mismatch rejection, got {err}"
        );
        assert!(builder.is_empty());
    }

    #[test]
    fn precompressed_encrypted_compressed_payload_rejects_crc32_metadata_mismatch() {
        let mut builder = P4kBuilder::new();
        let plaintext = b"payload that should be compressed and encrypted";
        let compressed =
            compress_bytes(plaintext, CompressionMethod::Zstd, &Default::default()).unwrap();
        let payload = crypto::encrypt(&compressed).unwrap();
        let sha256 = sha256_array(&payload);

        let err = builder
            .add_precompressed_with_metadata(
                "bad-encrypted-compressed-crc.bin",
                payload,
                plaintext.len() as u64,
                CompressionMethod::Zstd,
                0xDEAD_BEEF,
                0,
                0,
                true,
                [0; 128],
                sha256,
            )
            .unwrap_err();

        assert!(
            err.to_string().contains("CRC32C metadata"),
            "expected encrypted compressed CRC32C metadata mismatch rejection, got {err}"
        );
        assert!(builder.is_empty());
    }

    #[test]
    fn copy_exact_prefix_rejects_short_source() {
        let mut output = Vec::new();
        let err = copy_exact_prefix(&b"short"[..], &mut output, 6).unwrap_err();
        assert_eq!(
            err.to_string(),
            "I/O error: source ended after 5 bytes, expected 6 bytes"
        );
        assert_eq!(output, b"short");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn copy_file_range_prefix_copies_or_cleanly_falls_back() {
        let source_path = temp_writer_path("svarog_p4k_copy_file_range_source");
        let destination_path = temp_writer_path("svarog_p4k_copy_file_range_destination");
        std::fs::write(&source_path, b"0123456789abcdef").unwrap();

        let source = File::open(&source_path).unwrap();
        let mut destination = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&destination_path)
            .unwrap();
        let copied = try_copy_file_range_prefix(&source, &mut destination, 10).unwrap();

        if copied {
            assert_eq!(destination.stream_position().unwrap(), 10);
            drop(destination);
            assert_eq!(std::fs::read(&destination_path).unwrap(), b"0123456789");
        } else {
            assert_eq!(destination.metadata().unwrap().len(), 0);
        }

        let _ = std::fs::remove_file(source_path);
        let _ = std::fs::remove_file(destination_path);
    }

    #[test]
    fn safe_output_path_rejects_windows_drive_and_stream_components() {
        let root = Path::new("/tmp/svarog-p4k-output");
        for name in ["C:/escape.bin", "dir/file:stream.bin"] {
            let err = safe_output_path(root, name).unwrap_err();
            assert!(
                err.to_string().contains("unsafe archive path"),
                "expected unsafe path rejection for {name}, got {err}"
            );
        }
    }

    #[test]
    fn v1_local_file_record_size_uses_dump_formula_with_checked_arithmetic() {
        assert_eq!(v1_local_file_record_size(12, 256).unwrap(), 512);

        let err = v1_local_file_record_size(u64::MAX, 4096).unwrap_err();
        assert!(
            err.to_string()
                .contains("v1 local file record size overflow"),
            "expected checked size overflow, got {err}"
        );
    }

    #[test]
    fn unix_time_to_dos_datetime_matches_known_values() {
        assert_eq!(dos_datetime_from_unix_secs(0), (0, 0x0021));

        // 2026-05-28 12:34:56 UTC
        let unix = 1_779_971_696;
        let (time, date) = dos_datetime_from_unix_secs(unix);
        assert_eq!(time, (12 << 11) | (34 << 5) | 28);
        assert_eq!(date, ((2026 - 1980) << 9) | (5 << 5) | 28);
    }

    #[test]
    fn freelist_merge_matches_dump_adjacent_only_then_trims_tail() {
        let mut blocks = vec![
            FreelistBlock {
                offset: 100,
                size: 20,
            },
            FreelistBlock {
                offset: 64,
                size: 8,
            },
            FreelistBlock {
                offset: 72,
                size: 8,
            },
            FreelistBlock {
                offset: 110,
                size: 5,
            },
        ];
        merge_freelist_blocks(&mut blocks).unwrap();
        assert_eq!(
            blocks,
            vec![
                FreelistBlock {
                    offset: 64,
                    size: 16
                },
                FreelistBlock {
                    offset: 100,
                    size: 20
                },
                FreelistBlock {
                    offset: 110,
                    size: 5
                },
            ]
        );

        blocks.push(FreelistBlock {
            offset: 112,
            size: 16,
        });
        let mut payload_end = 113;
        trim_trailing_freelist_blocks(&mut blocks, &mut payload_end, 16).unwrap();
        assert_eq!(payload_end, 112);
        assert_eq!(
            blocks,
            vec![
                FreelistBlock {
                    offset: 64,
                    size: 16
                },
                FreelistBlock {
                    offset: 100,
                    size: 20
                },
                FreelistBlock {
                    offset: 110,
                    size: 5
                },
            ]
        );
    }

    #[test]
    fn freelist_merge_rejects_merged_size_overflow() {
        let mut blocks = vec![
            FreelistBlock {
                offset: 0,
                size: u64::MAX,
            },
            FreelistBlock {
                offset: u64::MAX,
                size: 1,
            },
        ];

        let err = merge_freelist_blocks(&mut blocks).unwrap_err();
        assert!(
            err.to_string()
                .contains("merged freelist block size overflow"),
            "expected merged freelist overflow rejection, got {err}"
        );
    }

    #[test]
    fn write_v2_tail_rejects_cdr_offset_overflow() {
        let mut writer = PositionOnlyWriter::new(u64::MAX - 10).ignore_position_after_writes(1);
        let options = P4kWriterOptions {
            sector_size: 4096,
            ..Default::default()
        };
        let entries = [V2EntryMeta {
            name: "overflow.bin",
            offset_to_file_data: 0,
            compressed_size: 0,
            uncompressed_size: 0,
            compression_method: CompressionMethod::Store,
            crc32: 0,
            last_mod_file_time: 0,
            last_mod_file_date: 0,
            encrypted: false,
            signature: [0; 128],
            sha256: [0; 32],
            bytes_already_written: 0,
        }];

        let err =
            write_v2_tail(&mut writer, &entries, &[], u64::MAX - 10, &options, false).unwrap_err();
        assert!(
            err.to_string().contains("v2 CDR offset overflow"),
            "expected CDR offset overflow rejection, got {err}"
        );
    }

    struct PositionOnlyWriter {
        position: u64,
        writes: usize,
        ignore_position_after_writes: Option<usize>,
    }

    impl PositionOnlyWriter {
        fn new(position: u64) -> Self {
            Self {
                position,
                writes: 0,
                ignore_position_after_writes: None,
            }
        }

        fn ignore_position_after_writes(mut self, count: usize) -> Self {
            self.ignore_position_after_writes = Some(count);
            self
        }
    }

    impl Write for PositionOnlyWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.writes += 1;
            if self
                .ignore_position_after_writes
                .map_or(true, |limit| self.writes <= limit)
            {
                self.position = self.position.checked_add(buf.len() as u64).ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidInput, "position overflow")
                })?;
            }
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl Seek for PositionOnlyWriter {
        fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
            self.position = match pos {
                SeekFrom::Start(position) => position,
                SeekFrom::Current(offset) => self
                    .position
                    .checked_add_signed(offset)
                    .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "seek overflow"))?,
                SeekFrom::End(_) => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "end-relative seek is unsupported",
                    ));
                }
            };
            Ok(self.position)
        }

        fn stream_position(&mut self) -> io::Result<u64> {
            Ok(self.position)
        }
    }

    fn temp_writer_path(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("{name}-{}-{nanos}", std::process::id()))
    }

    fn assert_no_temp_output_leak(output: &Path) {
        let parent = output
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let file_name = output.file_name().unwrap().to_string_lossy();
        let temp_prefix = format!(".{file_name}.svarog-p4k-output-");
        let leaked = std::fs::read_dir(parent)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .any(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(&temp_prefix)
            });
        assert!(!leaked, "temporary archive output should be removed");
    }

    fn test_raw_entry(name: &str, payload_offset: u64, payload_len: u64) -> P4kRawEntryRef<'_> {
        P4kRawEntryRef {
            name,
            payload_len,
            local_header_offset: payload_offset.saturating_sub(DEFAULT_SECTOR_SIZE),
            payload_offset,
            local_record_size: DEFAULT_SECTOR_SIZE,
            uncompressed_size: payload_len,
            compression_method: CompressionMethod::Store,
            is_encrypted: false,
            crc32: 0,
            last_mod_file_time: 0,
            last_mod_file_date: 0,
            signature: [0; 128],
            sha256: [0; 32],
            bytes_already_written: payload_len,
        }
    }
}
