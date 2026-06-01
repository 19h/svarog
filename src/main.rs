//! Svarog CLI - Command-line tool for Star Citizen game file extraction.
//!
//! This is the main entry point for the Svarog command-line application.

use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{BufWriter, Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use indicatif::{ProgressBar, ProgressStyle};
use serde_json::json;
use walkdir::WalkDir;

use svarog::cryxml::{CryXmlAttribute, CryXmlHeader, CryXmlNode};
use svarog::prelude::*;

const CRYXML_MAGIC: &[u8; 8] = b"CryXmlB\0";
const CRYXML_HEADER_SIZE: usize = std::mem::size_of::<CryXmlHeader>();

#[derive(Debug, Clone, Copy, ValueEnum)]
enum CliP4kCompression {
    Store,
    Deflate,
    DeflateZlib,
    ZstdDeprecated,
    Zstd,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum CliP4kVersion {
    V1,
    V2,
}

impl From<CliP4kCompression> for svarog::p4k::zip::CompressionMethod {
    fn from(value: CliP4kCompression) -> Self {
        match value {
            CliP4kCompression::Store => Self::Store,
            CliP4kCompression::Deflate => Self::Deflate,
            CliP4kCompression::DeflateZlib => Self::DeflateZlib,
            CliP4kCompression::ZstdDeprecated => Self::ZstdDeprecated,
            CliP4kCompression::Zstd => Self::Zstd,
        }
    }
}

/// Progress stage for detailed visualization
#[derive(Clone, Copy)]
enum Stage {
    P4kExtract,
    P4kVerify,
    P4kConvert,
    SocpakExpand,
    CryXmlDecode,
    DcbExport,
}

impl Stage {
    fn prefix(self) -> &'static str {
        match self {
            Stage::P4kExtract => "P4K",
            Stage::P4kVerify => "VERIFY",
            Stage::P4kConvert => "CONVERT",
            Stage::SocpakExpand => "SOCPAK",
            Stage::CryXmlDecode => "CryXML",
            Stage::DcbExport => "DCB",
        }
    }

    fn color(self) -> &'static str {
        match self {
            Stage::P4kExtract => "cyan",
            Stage::P4kVerify => "red",
            Stage::P4kConvert => "green",
            Stage::SocpakExpand => "yellow",
            Stage::CryXmlDecode => "magenta",
            Stage::DcbExport => "green",
        }
    }
}

/// Create a progress bar with stage-aware template
fn create_progress_bar(len: u64, stage: Stage) -> ProgressBar {
    let pb = ProgressBar::new(len);
    let template = format!(
        "{{spinner:.{}}} [{{elapsed_precise}}] [{{bar:40.{}/blue}}] {{pos}}/{{len}} ({{per_sec}}) {{msg}}",
        stage.color(),
        stage.color()
    );
    pb.set_style(
        ProgressStyle::default_bar()
            .template(&template)
            .unwrap()
            .progress_chars("#>-"),
    );
    pb
}

fn create_spinner(stage: Stage, message: impl Into<String>) -> ProgressBar {
    let pb = ProgressBar::new_spinner();
    let template = format!(
        "{{spinner:.{}}} [{{elapsed_precise}}] {{msg}}",
        stage.color()
    );
    pb.set_style(
        ProgressStyle::default_spinner()
            .template(&template)
            .unwrap(),
    );
    pb.set_message(format!("[{}] {}", stage.prefix(), message.into()));
    pb.enable_steady_tick(Duration::from_millis(100));
    pb
}

fn create_bytes_progress_bar(len: u64, stage: Stage, message: impl Into<String>) -> ProgressBar {
    let pb = ProgressBar::new(len);
    let template = format!(
        "{{spinner:.{}}} [{{elapsed_precise}}] [{{bar:40.{}/blue}}] {{bytes}}/{{total_bytes}} ({{bytes_per_sec}}, eta {{eta}}) {{msg}}",
        stage.color(),
        stage.color()
    );
    pb.set_style(
        ProgressStyle::default_bar()
            .template(&template)
            .unwrap()
            .progress_chars("#>-"),
    );
    pb.set_message(format!("[{}] {}", stage.prefix(), message.into()));
    pb
}

/// Format a file path for display (truncate if too long)
fn format_path_for_display(path: &str, max_len: usize) -> String {
    if path.len() <= max_len {
        path.to_string()
    } else {
        let start = path.len() - max_len + 3;
        format!("...{}", &path[start..])
    }
}

/// Set progress message with stage prefix and file name
fn set_progress_message(pb: &ProgressBar, stage: Stage, file: &str) {
    let display_path = format_path_for_display(file, 50);
    pb.set_message(format!("[{}] {}", stage.prefix(), display_path));
}

/// Try to decode a CryXML file in-place, returning true if converted
fn try_decode_cryxml_inplace(path: &Path) -> Result<bool> {
    let data = fs::read(path)?;

    if !CryXml::is_cryxml(&data) {
        return Ok(false);
    }

    let cryxml = CryXml::parse(&data).context("Failed to parse CryXmlB")?;
    let xml = cryxml.to_xml_string().context("Failed to convert to XML")?;
    fs::write(path, xml)?;

    Ok(true)
}

/// Svarog - Star Citizen game file extraction tool
#[derive(Parser)]
#[command(name = "svarog")]
#[command(author, version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Extract files from a P4K archive with advanced options
    P4kExtract {
        /// Path to the P4K file
        #[arg(short, long, env = "INPUT_P4K")]
        p4k: PathBuf,

        /// Output directory
        #[arg(short, long, env = "OUTPUT_FOLDER")]
        output: PathBuf,

        /// Filter pattern (glob-style, or regex if --regex is set)
        #[arg(short, long)]
        filter: Option<String>,

        /// Treat filter as regex instead of glob
        #[arg(long)]
        regex: bool,

        /// Incremental extraction: skip files that already exist with matching size
        #[arg(long, default_value = "true")]
        incremental: bool,

        /// Extract and process DataCore (Game.dcb or Game2.dcb) to XML
        #[arg(long, default_value = "true")]
        extract_dcb: bool,

        /// Extract and expand SOCPAK files inline
        #[arg(long, default_value = "true")]
        expand_socpak: bool,

        /// Number of parallel workers (0 = auto)
        #[arg(long, short = 'j', default_value = "0")]
        parallel: usize,
    },

    /// List contents of a P4K archive
    P4kList {
        /// Path to the P4K file
        #[arg(short, long, env = "INPUT_P4K")]
        p4k: PathBuf,

        /// Filter pattern (glob-style)
        #[arg(short, long)]
        filter: Option<String>,

        /// Show detailed information
        #[arg(short, long)]
        detailed: bool,

        /// Emit archive and entry metadata as JSON
        #[arg(long)]
        json: bool,
    },

    /// Dump every file from a P4K archive without extra post-processing
    P4kDump {
        /// Path to the P4K file
        #[arg(short, long, env = "INPUT_P4K")]
        p4k: PathBuf,

        /// Output directory
        #[arg(short, long, env = "OUTPUT_FOLDER")]
        output: PathBuf,

        /// Number of parallel workers (0 = auto)
        #[arg(long, short = 'j', default_value = "0")]
        parallel: usize,

        /// Dump raw compressed/encrypted payload bytes instead of decoded files
        #[arg(long)]
        raw_payloads: bool,
    },

    /// Verify P4K raw payload SHA-256 and decoded CRC32C metadata
    P4kVerify {
        /// Path to the P4K file
        #[arg(short, long, env = "INPUT_P4K")]
        p4k: PathBuf,

        /// Verify only raw compressed/encrypted payload SHA-256 metadata
        #[arg(long)]
        raw_sha_only: bool,
    },

    /// Create a P4K archive from a directory
    P4kCreate {
        /// Input directory to pack
        #[arg(short, long)]
        input: PathBuf,

        /// Output P4K file
        #[arg(short, long)]
        output: PathBuf,

        /// Compression method for file payloads
        #[arg(long, value_enum, default_value = "zstd")]
        compression: CliP4kCompression,

        /// P4K format version to write
        #[arg(long, value_enum, default_value = "v2")]
        version: CliP4kVersion,

        /// Physical sector size recorded in the archive
        #[arg(long, default_value = "4096")]
        sector_size: u64,

        /// Zstandard level when --compression=zstd
        #[arg(long, default_value = "1")]
        zstd_level: i32,

        /// Encrypt payloads with the P4K AES-CBC scheme
        #[arg(long)]
        encrypt: bool,

        /// Hex manifest digest(s) for the v2 EOCDR: 64 hex chars for the first SHA-256, or 128 for both stored digests
        #[arg(long)]
        manifest_sha256: Option<String>,

        /// Number of parallel workers for file hashing/compression (0 = auto)
        #[arg(long, short = 'j', default_value = "0")]
        parallel: usize,
    },

    /// Convert a P4K v1 archive to P4K v2
    P4kConvertV2 {
        /// Input P4K v1 file
        #[arg(short, long)]
        input: PathBuf,

        /// Output P4K v2 file
        #[arg(short, long)]
        output: PathBuf,

        /// Physical sector size recorded in the v2 EOCDR
        #[arg(long, default_value = "4096")]
        sector_size: u64,

        /// Hex manifest digest(s) for the v2 EOCDR: 64 hex chars for the first SHA-256, or 128 for both stored digests
        #[arg(long)]
        manifest_sha256: Option<String>,

        /// Convert in place by rewriting only the metadata tail (seconds, no
        /// payload copy). CONSUMES the source: it is renamed to the output path,
        /// which must be on the same filesystem as the input.
        #[arg(long)]
        in_place: bool,
    },

    /// Rebuild only the metadata tail of an existing P4K v2 archive
    P4kRewriteV2Tail {
        /// Path to the P4K v2 file to update in place
        #[arg(short, long, env = "INPUT_P4K")]
        p4k: PathBuf,
    },

    /// Convert a CryXmlB file to XML
    CryxmlConvert {
        /// Input CryXmlB file
        #[arg(short, long)]
        input: PathBuf,

        /// Output XML file
        #[arg(short, long)]
        output: PathBuf,
    },

    /// Convert an XML file to CryXmlB binary format
    CryxmlCreate {
        /// Input XML file
        #[arg(short, long)]
        input: PathBuf,

        /// Output CryXmlB file
        #[arg(short, long)]
        output: PathBuf,
    },

    /// Extract DataCore database to XML/JSON files
    DcbExtract {
        /// Path to the DCB file or P4K containing it
        #[arg(short, long)]
        input: PathBuf,

        /// Output directory
        #[arg(short, long)]
        output: PathBuf,

        /// Filter pattern for record file names
        #[arg(short, long)]
        filter: Option<String>,
    },

    /// Process a CHF character file
    ChfProcess {
        /// Input CHF file
        #[arg(short, long)]
        input: PathBuf,

        /// Output file (CHF or BIN)
        #[arg(short, long)]
        output: PathBuf,
    },

    /// Merge split DDS files
    DdsMerge {
        /// Input DDS file (base file without .N suffix)
        #[arg(short, long)]
        input: PathBuf,

        /// Output DDS file
        #[arg(short, long)]
        output: PathBuf,
    },

    /// Export DataCore schema as a C header file
    DcbSchema {
        /// Path to the DCB file
        #[arg(short, long)]
        input: PathBuf,

        /// Output C header file
        #[arg(short, long)]
        output: PathBuf,
    },

    /// Compare two P4K archives
    P4kCompare {
        /// Path to the first (old) P4K file
        #[arg(long, alias = "old")]
        p4k1: PathBuf,

        /// Path to the second (new) P4K file
        #[arg(long, alias = "new")]
        p4k2: PathBuf,

        /// Show only added files
        #[arg(long)]
        added_only: bool,

        /// Show only removed files
        #[arg(long)]
        removed_only: bool,

        /// Show only modified files
        #[arg(long)]
        modified_only: bool,

        /// Show full diff for XML/text files
        #[arg(long, short)]
        diff: bool,

        /// Filter pattern (glob-style)
        #[arg(short, long)]
        filter: Option<String>,
    },

    /// Compare two DCB databases
    DcbCompare {
        /// Path to the first (old) DCB or P4K file (DCB extracted from P4K automatically)
        #[arg(long, alias = "old")]
        dcb1: PathBuf,

        /// Path to the second (new) DCB or P4K file (DCB extracted from P4K automatically)
        #[arg(long, alias = "new")]
        dcb2: PathBuf,

        /// What to compare: records, structs, enums, or all
        #[arg(long, default_value = "all")]
        scope: String,

        /// Show only added items
        #[arg(long)]
        added_only: bool,

        /// Show only removed items
        #[arg(long)]
        removed_only: bool,

        /// Show only modified items
        #[arg(long)]
        modified_only: bool,

        /// Show full diff for modified items
        #[arg(long, short)]
        diff: bool,

        /// Filter pattern (name filter)
        #[arg(short, long)]
        filter: Option<String>,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::P4kExtract {
            p4k,
            output,
            filter,
            regex,
            incremental,
            extract_dcb,
            expand_socpak,
            parallel,
        } => {
            cmd_p4k_extract(
                &p4k,
                &output,
                filter.as_deref(),
                regex,
                incremental,
                extract_dcb,
                expand_socpak,
                parallel,
            )?;
        }
        Commands::P4kList {
            p4k,
            filter,
            detailed,
            json,
        } => {
            cmd_p4k_list(&p4k, filter.as_deref(), detailed, json)?;
        }
        Commands::P4kDump {
            p4k,
            output,
            parallel,
            raw_payloads,
        } => {
            cmd_p4k_dump(&p4k, &output, parallel, raw_payloads)?;
        }
        Commands::P4kVerify { p4k, raw_sha_only } => {
            cmd_p4k_verify(&p4k, raw_sha_only)?;
        }
        Commands::P4kCreate {
            input,
            output,
            compression,
            version,
            sector_size,
            zstd_level,
            encrypt,
            manifest_sha256,
            parallel,
        } => {
            cmd_p4k_create(
                &input,
                &output,
                compression,
                version,
                sector_size,
                zstd_level,
                encrypt,
                manifest_sha256.as_deref(),
                parallel,
            )?;
        }
        Commands::P4kConvertV2 {
            input,
            output,
            sector_size,
            manifest_sha256,
            in_place,
        } => {
            cmd_p4k_convert_v2(
                &input,
                &output,
                sector_size,
                manifest_sha256.as_deref(),
                in_place,
            )?;
        }
        Commands::P4kRewriteV2Tail { p4k } => {
            cmd_p4k_rewrite_v2_tail(&p4k)?;
        }
        Commands::CryxmlConvert { input, output } => {
            cmd_cryxml_convert(&input, &output)?;
        }
        Commands::CryxmlCreate { input, output } => {
            cmd_cryxml_create(&input, &output)?;
        }
        Commands::DcbExtract {
            input,
            output,
            filter,
        } => {
            cmd_dcb_extract(&input, &output, filter.as_deref())?;
        }
        Commands::ChfProcess { input, output } => {
            cmd_chf_process(&input, &output)?;
        }
        Commands::DdsMerge { input, output } => {
            cmd_dds_merge(&input, &output)?;
        }
        Commands::DcbSchema { input, output } => {
            cmd_dcb_schema(&input, &output)?;
        }
        Commands::P4kCompare {
            p4k1,
            p4k2,
            added_only,
            removed_only,
            modified_only,
            diff,
            filter,
        } => {
            cmd_p4k_compare(
                &p4k1,
                &p4k2,
                added_only,
                removed_only,
                modified_only,
                diff,
                filter.as_deref(),
            )?;
        }
        Commands::DcbCompare {
            dcb1,
            dcb2,
            scope,
            added_only,
            removed_only,
            modified_only,
            diff,
            filter,
        } => {
            cmd_dcb_compare(
                &dcb1,
                &dcb2,
                &scope,
                added_only,
                removed_only,
                modified_only,
                diff,
                filter.as_deref(),
            )?;
        }
    }

    Ok(())
}

/// Case-insensitive path mapper for merging DCB output with existing folders.
///
/// The DCB exports files with lowercase paths, but the P4K archive has mixed case.
/// This mapper ensures we use the existing case when a folder already exists.
struct CaseInsensitivePathMapper {
    /// Maps lowercase path components to their actual case on disk
    component_cache: Mutex<HashMap<String, String>>,
}

impl CaseInsensitivePathMapper {
    fn new() -> Self {
        Self {
            component_cache: Mutex::new(HashMap::new()),
        }
    }

    /// Resolve a path using existing case from the filesystem.
    ///
    /// For each component in the path, checks if a case-insensitive match exists
    /// on disk and uses that case instead. This allows DCB exports (lowercase)
    /// to merge with existing P4K extracts (mixed case).
    fn resolve(&self, base: &Path, relative: &str) -> PathBuf {
        let mut result = base.to_path_buf();
        let components: Vec<&str> = relative
            .split(['/', '\\'])
            .filter(|s| !s.is_empty())
            .collect();

        for (i, component) in components.iter().enumerate() {
            let is_last = i == components.len() - 1;

            // Build the cache key (lowercase path so far)
            let cache_key = if i == 0 {
                component.to_lowercase()
            } else {
                let prefix: String = components[..i]
                    .iter()
                    .map(|c| c.to_lowercase())
                    .collect::<Vec<_>>()
                    .join("/");
                format!("{}/{}", prefix, component.to_lowercase())
            };

            // Check cache first
            {
                let cache = self.component_cache.lock().unwrap();
                if let Some(cached) = cache.get(&cache_key) {
                    result.push(cached);
                    continue;
                }
            }

            // Try to find existing entry with matching case
            let matched_name = if result.exists() {
                self.find_case_insensitive_match(&result, component)
            } else {
                None
            };

            let actual_name = matched_name.unwrap_or_else(|| component.to_string());

            // Cache the result (only for directories, not the final file)
            if !is_last {
                let mut cache = self.component_cache.lock().unwrap();
                cache.insert(cache_key, actual_name.clone());
            }

            result.push(&actual_name);
        }

        result
    }

    /// Find a case-insensitive match in a directory.
    fn find_case_insensitive_match(&self, dir: &Path, target: &str) -> Option<String> {
        let target_lower = target.to_lowercase();

        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                if let Some(name_str) = name.to_str() {
                    if name_str.to_lowercase() == target_lower {
                        return Some(name_str.to_string());
                    }
                }
            }
        }

        None
    }
}

/// Result of SOCPAK extraction
struct SocpakExtractionResult {
    files_extracted: usize,
    cryxml_decoded: usize,
}

struct P4kExtractionTask {
    index: usize,
    entry_index: usize,
    name: String,
    output_path: PathBuf,
    kind: P4kExtractionKind,
}

enum P4kExtractionKind {
    File,
    Socpak { dir: PathBuf },
}

struct PrefixCaptureWriter<W> {
    inner: W,
    prefix: [u8; 8],
    prefix_len: usize,
}

impl<W> PrefixCaptureWriter<W> {
    fn new(inner: W) -> Self {
        Self {
            inner,
            prefix: [0; 8],
            prefix_len: 0,
        }
    }

    fn captured_prefix(&self) -> &[u8] {
        &self.prefix[..self.prefix_len]
    }
}

impl<W: Write> Write for PrefixCaptureWriter<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let written = self.inner.write(buf)?;
        if self.prefix_len < self.prefix.len() {
            let capture_len = (self.prefix.len() - self.prefix_len).min(written);
            let start = self.prefix_len;
            let end = start + capture_len;
            self.prefix[start..end].copy_from_slice(&buf[..capture_len]);
            self.prefix_len = end;
        }
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

fn cryxml_declared_min_len(header: &[u8; CRYXML_HEADER_SIZE]) -> Option<usize> {
    let read_u32 = |offset: usize| {
        let bytes = header.get(offset..offset + 4)?;
        Some(u32::from_le_bytes(bytes.try_into().ok()?))
    };

    let node_table_position = read_u32(4)? as usize;
    let node_count = read_u32(8)? as usize;
    let attribute_table_position = read_u32(12)? as usize;
    let attribute_count = read_u32(16)? as usize;
    let child_table_position = read_u32(20)? as usize;
    let child_count = read_u32(24)? as usize;
    let string_data_position = read_u32(28)? as usize;
    let string_data_size = read_u32(32)? as usize;

    let node_end = node_count
        .checked_mul(std::mem::size_of::<CryXmlNode>())?
        .checked_add(node_table_position)?;
    let attribute_end = attribute_count
        .checked_mul(std::mem::size_of::<CryXmlAttribute>())?
        .checked_add(attribute_table_position)?;
    let child_end = child_count
        .checked_mul(4)?
        .checked_add(child_table_position)?;
    let string_end = string_data_size.checked_add(string_data_position)?;

    Some(
        CRYXML_MAGIC
            .len()
            .checked_add(CRYXML_HEADER_SIZE)?
            .max(node_end)
            .max(attribute_end)
            .max(child_end)
            .max(string_end),
    )
}

fn cryxml_header_fits_member(header: &[u8; CRYXML_HEADER_SIZE], member_size: u64) -> bool {
    let Ok(member_size) = usize::try_from(member_size) else {
        return false;
    };
    cryxml_declared_min_len(header).is_some_and(|declared_min| declared_min <= member_size)
}

/// Extract a SOCPAK (which is just a ZIP file) to a directory.
/// Also decodes any CryXML files found inside.
fn extract_socpak<R>(
    reader: R,
    output_dir: &Path,
    pb: Option<&ProgressBar>,
) -> Result<SocpakExtractionResult>
where
    R: Read + Seek,
{
    let mut archive =
        zip::ZipArchive::new(reader).context("Failed to open SOCPAK as ZIP archive")?;

    let mut extracted = 0;
    let mut cryxml_decoded = 0;

    for i in 0..archive.len() {
        let mut file = archive.by_index(i)?;
        let name = file.name().to_string();

        // Skip directories
        if name.ends_with('/') {
            continue;
        }

        let output_path = output_dir.join(name.replace('\\', "/"));

        if let Some(parent) = output_path.parent() {
            fs::create_dir_all(parent)?;
        }

        let mut prefix = [0u8; 8];
        let mut prefix_len = 0usize;
        while prefix_len < prefix.len() {
            let read = file.read(&mut prefix[prefix_len..])?;
            if read == 0 {
                break;
            }
            prefix_len += read;
        }

        // Check if this is a CryXML file and decode it. Non-CryXML
        // SOCPAK members stream directly to disk instead of first
        // materializing the full member in memory.
        if prefix_len == CRYXML_MAGIC.len() && prefix == *CRYXML_MAGIC {
            let mut header = [0u8; CRYXML_HEADER_SIZE];
            let mut header_len = 0usize;
            while header_len < header.len() {
                let read = file.read(&mut header[header_len..])?;
                if read == 0 {
                    break;
                }
                header_len += read;
            }

            if header_len != CRYXML_HEADER_SIZE || !cryxml_header_fits_member(&header, file.size())
            {
                let output = File::create(&output_path)?;
                let mut output = BufWriter::with_capacity(1024 * 1024, output);
                output.write_all(&prefix)?;
                output.write_all(&header[..header_len])?;
                std::io::copy(&mut file, &mut output)?;
                output.flush()?;
                extracted += 1;
                continue;
            }

            let capacity = usize::try_from(file.size()).unwrap_or(
                CRYXML_MAGIC
                    .len()
                    .saturating_add(CRYXML_HEADER_SIZE)
                    .saturating_add(file.size().min(usize::MAX as u64) as usize),
            );
            let mut contents = Vec::with_capacity(capacity);
            contents.extend_from_slice(&prefix[..prefix_len]);
            contents.extend_from_slice(&header);
            file.read_to_end(&mut contents)?;
            if let Some(pb) = pb {
                set_progress_message(pb, Stage::CryXmlDecode, &name);
            }

            match CryXml::parse(&contents) {
                Ok(cryxml) => {
                    if let Ok(xml) = cryxml.to_xml_string() {
                        fs::write(&output_path, xml)?;
                        cryxml_decoded += 1;
                        extracted += 1;
                        continue;
                    }
                }
                Err(_) => {
                    // Fall through to write raw contents
                }
            }

            fs::write(&output_path, &contents)?;
        } else {
            let output = File::create(&output_path)?;
            let mut output = BufWriter::with_capacity(1024 * 1024, output);
            output.write_all(&prefix[..prefix_len])?;
            std::io::copy(&mut file, &mut output)?;
            output.flush()?;
        }
        extracted += 1;
    }

    Ok(SocpakExtractionResult {
        files_extracted: extracted,
        cryxml_decoded,
    })
}

fn process_p4k_extraction_task(
    archive: &P4kArchive,
    task: &P4kExtractionTask,
    pb: &ProgressBar,
    extracted: &AtomicU64,
    socpak_expanded: &AtomicU64,
    cryxml_decoded: &AtomicU64,
    errors: &AtomicU64,
) {
    if task.index % 1024 == 0 {
        set_progress_message(pb, Stage::P4kExtract, &task.name);
    }

    match &task.kind {
        P4kExtractionKind::Socpak { dir } => {
            let write_result = (|| -> Result<()> {
                let file = File::create(&task.output_path)?;
                let mut output = BufWriter::with_capacity(1024 * 1024, file);
                archive.extract_index_to_writer(task.entry_index, &mut output)?;
                output.flush()?;
                Ok(())
            })();
            if let Err(e) = write_result {
                eprintln!("Failed to read {}: {}", task.name, e);
                errors.fetch_add(1, Ordering::Relaxed);
                pb.inc(1);
                return;
            }

            let socpak = match File::open(&task.output_path) {
                Ok(file) => file,
                Err(e) => {
                    eprintln!("Failed to open extracted SOCPAK {}: {}", task.name, e);
                    errors.fetch_add(1, Ordering::Relaxed);
                    pb.inc(1);
                    return;
                }
            };

            if task.index % 1024 == 0 {
                set_progress_message(pb, Stage::SocpakExpand, &task.name);
            }

            match extract_socpak(socpak, dir, Some(pb)) {
                Ok(result) => {
                    let _ = fs::remove_file(&task.output_path);
                    socpak_expanded.fetch_add(result.files_extracted as u64, Ordering::Relaxed);
                    cryxml_decoded.fetch_add(result.cryxml_decoded as u64, Ordering::Relaxed);
                    extracted.fetch_add(1, Ordering::Relaxed);
                }
                Err(e) => {
                    eprintln!("Failed to extract SOCPAK {}: {}", task.name, e);
                    extracted.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
        P4kExtractionKind::File => {
            let write_result = (|| -> Result<bool> {
                let file = File::create(&task.output_path)?;
                let writer = BufWriter::with_capacity(1024 * 1024, file);
                let mut output = PrefixCaptureWriter::new(writer);
                archive.extract_index_to_writer(task.entry_index, &mut output)?;
                output.flush()?;
                Ok(output.captured_prefix() == CRYXML_MAGIC)
            })();

            match write_result {
                Ok(true) => {
                    if try_decode_cryxml_inplace(&task.output_path).unwrap_or(false) {
                        cryxml_decoded.fetch_add(1, Ordering::Relaxed);
                    }
                    extracted.fetch_add(1, Ordering::Relaxed);
                }
                Ok(false) => {
                    extracted.fetch_add(1, Ordering::Relaxed);
                }
                Err(e) => {
                    eprintln!("Failed to extract {}: {}", task.name, e);
                    errors.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
    }

    pb.inc(1);
}

fn prepare_p4k_extraction_parent_dirs(tasks: &[P4kExtractionTask]) -> Result<()> {
    let mut parent_dirs = HashSet::with_capacity(tasks.len());
    for task in tasks {
        if let Some(parent) = task.output_path.parent() {
            parent_dirs.insert(parent.to_path_buf());
        }
    }
    for dir in parent_dirs {
        fs::create_dir_all(&dir)
            .with_context(|| format!("Failed to create directory {}", dir.display()))?;
    }
    Ok(())
}

fn run_with_rayon_threads<T, F>(workers: usize, f: F) -> Result<T>
where
    T: Send,
    F: FnOnce() -> Result<T> + Send,
{
    if workers == 0 {
        return f();
    }

    rayon::ThreadPoolBuilder::new()
        .num_threads(workers)
        .build()
        .context("Failed to build Rayon worker pool")?
        .install(f)
}

fn has_parallel_output_conflict(tasks: &[P4kExtractionTask]) -> bool {
    let socpak_dirs: Vec<_> = tasks
        .iter()
        .filter_map(|task| match &task.kind {
            P4kExtractionKind::Socpak { dir } => Some(dir),
            P4kExtractionKind::File => None,
        })
        .collect();

    for task in tasks {
        for dir in &socpak_dirs {
            if matches!(&task.kind, P4kExtractionKind::Socpak { dir: task_dir } if task_dir == *dir)
            {
                continue;
            }
            if task.output_path.starts_with(dir) {
                return true;
            }
        }
    }

    false
}

/// Check if a file is an undecoded CryXML file by reading its magic bytes.
/// If so, decode it in place. Returns true if decoded.
fn check_and_decode_cryxml(path: &Path) -> bool {
    // Read first 8 bytes to check for CryXML magic
    let mut file = match fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return false,
    };

    let mut magic = [0u8; 8];
    if std::io::Read::read_exact(&mut file, &mut magic).is_err() {
        return false;
    }

    if &magic != CRYXML_MAGIC {
        return false;
    }

    // It's a CryXML file - decode it
    try_decode_cryxml_inplace(path).unwrap_or(false)
}

/// Scan an already-extracted SOCPAK directory for undecoded CryXML files.
/// Only checks files with XML-like extensions. Returns count of decoded files.
fn decode_cryxml_in_directory(dir: &Path, pb: Option<&ProgressBar>) -> Result<usize> {
    let mut decoded = 0;

    if !dir.exists() || !dir.is_dir() {
        return Ok(0);
    }

    // Iterate lazily - don't collect into a Vec
    for entry in walkdir::WalkDir::new(dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
    {
        let path = entry.path();

        // Only check files with XML-like extensions that might be CryXML
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        if !matches!(
            ext.to_lowercase().as_str(),
            "xml" | "mtl" | "cdf" | "chrparams" | "adb" | "rmxml"
        ) {
            continue;
        }

        if let Some(pb) = pb {
            let rel_path = path.strip_prefix(dir).unwrap_or(path);
            set_progress_message(pb, Stage::CryXmlDecode, &rel_path.display().to_string());
        }

        if check_and_decode_cryxml(path) {
            decoded += 1;
        }
    }

    Ok(decoded)
}

/// Check if a file should be skipped during incremental extraction.
fn should_skip_file(output_path: &Path, expected_size: u64) -> bool {
    if let Ok(metadata) = fs::metadata(output_path) {
        metadata.len() == expected_size
    } else {
        false
    }
}

fn ends_with_ascii_ignore_case(value: &str, suffix: &str) -> bool {
    value
        .get(value.len().saturating_sub(suffix.len())..)
        .is_some_and(|tail| tail.eq_ignore_ascii_case(suffix))
}

/// Check if a directory contains any files (recursively).
/// Returns false for empty directories or directories containing only empty subdirectories.
fn has_any_files(dir: &Path) -> bool {
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() || (path.is_dir() && has_any_files(&path)) {
                return true;
            }
        }
    }
    false
}

#[allow(clippy::too_many_arguments)]
fn cmd_p4k_extract(
    p4k_path: &PathBuf,
    output: &PathBuf,
    filter: Option<&str>,
    use_regex: bool,
    incremental: bool,
    extract_dcb: bool,
    expand_socpak: bool,
    parallel: usize,
) -> Result<()> {
    println!("Opening P4K archive: {}", p4k_path.display());

    let start = Instant::now();
    let archive = P4kArchive::open(p4k_path).context("Failed to open P4K archive")?;

    println!(
        "Loaded {} entries in {:?}",
        archive.entry_count(),
        start.elapsed()
    );

    // Compile regex if using regex mode
    let regex_filter = if use_regex {
        filter
            .map(|p| regex::Regex::new(p).context("Invalid regex pattern"))
            .transpose()?
    } else {
        None
    };

    // Track ALL SOCPAK directories for CryXML post-processing check
    let mut all_socpak_dirs: Vec<PathBuf> = Vec::new();

    fs::create_dir_all(output)?;

    // Path mapper for case-insensitive merging
    let path_mapper = CaseInsensitivePathMapper::new();

    let mut tasks = Vec::new();
    let mut target_paths = HashSet::new();
    let mut dcb_entries = Vec::new();
    let mut has_duplicate_output = false;
    let mut matched_entries = 0usize;
    let mut planned_skipped = 0u64;
    let mut planned_cryxml_decoded = 0u64;

    for (idx, entry) in archive.iter().enumerate() {
        if extract_dcb && ends_with_ascii_ignore_case(entry.name, ".dcb") {
            dcb_entries.push((idx, entry.name.to_string()));
        }

        let matches_filter = if let Some(ref re) = regex_filter {
            re.is_match(entry.name)
        } else if let Some(pattern) = filter {
            glob_match(pattern, entry.name)
        } else {
            true
        };
        if !matches_filter {
            continue;
        }

        let task_index = matched_entries;
        matched_entries += 1;

        let name_normalized = entry.name.replace('\\', "/");
        let output_path = path_mapper.resolve(output, &name_normalized);

        // Check if this is a SOCPAK file
        let is_socpak = expand_socpak && ends_with_ascii_ignore_case(&name_normalized, ".socpak");

        // For SOCPAK files, we check if the extracted directory exists
        let socpak_dir = if is_socpak {
            Some(output_path.with_extension(""))
        } else {
            None
        };

        let should_extract = if let Some(ref dir) = socpak_dir {
            // Track all SOCPAK dirs for CryXML post-processing
            all_socpak_dirs.push(dir.clone());
            // For SOCPAK, check if directory exists and contains files
            if dir.exists() {
                // Check if directory has any files (recursively)
                // Empty dirs or dirs with only empty subdirs should be re-extracted
                let has_files = has_any_files(dir);
                if has_files {
                    // Directory has actual files - skip extraction, delete .socpak if present
                    if output_path.exists() {
                        let _ = fs::remove_file(&output_path);
                    }
                    false
                } else {
                    // No files found - remove empty tree and re-extract
                    let _ = fs::remove_dir_all(dir);
                    true
                }
            } else {
                true
            }
        } else if incremental {
            let dominated = should_skip_file(&output_path, entry.uncompressed_size);
            if dominated {
                // File exists with matching size - but check if it's undecoded CryXML
                if check_and_decode_cryxml(&output_path) {
                    planned_cryxml_decoded += 1;
                }
            }
            !dominated
        } else {
            true
        };

        if !should_extract {
            planned_skipped += 1;
            continue;
        }

        if !target_paths.insert(output_path.clone()) {
            has_duplicate_output = true;
        }

        let kind = if let Some(socpak_dir) = socpak_dir {
            P4kExtractionKind::Socpak { dir: socpak_dir }
        } else {
            P4kExtractionKind::File
        };
        tasks.push(P4kExtractionTask {
            index: task_index,
            entry_index: idx,
            name: name_normalized,
            output_path,
            kind,
        });
    }

    println!("Extracting {} entries from P4K...", matched_entries);

    if !dcb_entries.is_empty() {
        println!(
            "Found {} DCB file(s) - will extract and process DataCore",
            dcb_entries.len()
        );
    }

    let pb = create_progress_bar(matched_entries as u64, Stage::P4kExtract);
    if planned_skipped > 0 {
        pb.inc(planned_skipped);
    }

    // Statistics
    let extracted = AtomicU64::new(0);
    let skipped = AtomicU64::new(planned_skipped);
    let socpak_expanded = AtomicU64::new(0);
    let cryxml_decoded = AtomicU64::new(planned_cryxml_decoded);
    let errors = AtomicU64::new(0);

    let start = Instant::now();

    let has_output_conflict = has_duplicate_output || has_parallel_output_conflict(&tasks);
    prepare_p4k_extraction_parent_dirs(&tasks)?;
    if has_output_conflict || parallel == 1 {
        if has_duplicate_output && parallel != 1 {
            eprintln!("Duplicate output paths detected; preserving archive order for extraction");
        } else if has_output_conflict && parallel != 1 {
            eprintln!("Overlapping output paths detected; preserving archive order for extraction");
        }
        for task in &tasks {
            process_p4k_extraction_task(
                &archive,
                task,
                &pb,
                &extracted,
                &socpak_expanded,
                &cryxml_decoded,
                &errors,
            );
        }
    } else {
        use rayon::prelude::*;

        run_with_rayon_threads(parallel, || {
            tasks.par_iter().for_each(|task| {
                process_p4k_extraction_task(
                    &archive,
                    task,
                    &pb,
                    &extracted,
                    &socpak_expanded,
                    &cryxml_decoded,
                    &errors,
                );
            });
            Ok(())
        })?;
    }

    pb.finish_with_message("P4K extraction complete");

    let extracted_count = extracted.load(Ordering::Relaxed);
    let skipped_count = skipped.load(Ordering::Relaxed);
    let error_count = errors.load(Ordering::Relaxed);

    println!(
        "\nExtracted {} files, skipped {} (unchanged), {} errors in {:?}",
        extracted_count,
        skipped_count,
        error_count,
        start.elapsed()
    );

    // Debug: warn if incremental mode isn't working as expected
    if incremental && skipped_count == 0 && extracted_count > 0 {
        eprintln!("Warning: incremental mode enabled but no files were skipped - this may indicate a path mismatch");
    }

    let socpak_count = socpak_expanded.load(Ordering::Relaxed);
    let cryxml_count = cryxml_decoded.load(Ordering::Relaxed);

    if socpak_count > 0 || cryxml_count > 0 {
        let mut parts = Vec::new();
        if socpak_count > 0 {
            parts.push(format!("{} files from SOCPAK archives", socpak_count));
        }
        if cryxml_count > 0 {
            parts.push(format!("{} CryXML decoded", cryxml_count));
        }
        println!("{}", parts.join(", "));
    }

    // Process ALL SOCPAK directories for any undecoded CryXML files
    if !all_socpak_dirs.is_empty() {
        println!(
            "\nVerifying CryXML decoding in {} SOCPAK directories...",
            all_socpak_dirs.len()
        );

        let cryxml_pb = create_progress_bar(all_socpak_dirs.len() as u64, Stage::CryXmlDecode);
        let mut total_decoded = 0u64;

        for dir in &all_socpak_dirs {
            let dir_name = dir.file_name().unwrap_or_default().to_string_lossy();
            set_progress_message(&cryxml_pb, Stage::CryXmlDecode, &dir_name);

            match decode_cryxml_in_directory(dir, Some(&cryxml_pb)) {
                Ok(count) => {
                    total_decoded += count as u64;
                }
                Err(e) => {
                    eprintln!("Error processing {}: {}", dir.display(), e);
                }
            }

            cryxml_pb.inc(1);
        }

        cryxml_pb.finish_with_message("CryXML verification complete");

        if total_decoded > 0 {
            println!("Decoded {} additional CryXML files", total_decoded);
        }
    }

    // Extract and process all DCB files
    for (dcb_idx, dcb_name) in &dcb_entries {
        println!("\nProcessing DataCore: {}", dcb_name);

        let dcb_start = Instant::now();
        let dcb_data = match archive.read_index(*dcb_idx) {
            Ok(data) => data,
            Err(e) => {
                eprintln!("Failed to read {}: {}", dcb_name, e);
                continue;
            }
        };

        let database = match DataCoreDatabase::parse(&dcb_data) {
            Ok(db) => db,
            Err(e) => {
                eprintln!("Failed to parse {}: {}", dcb_name, e);
                continue;
            }
        };

        println!(
            "Loaded DataCore in {:?}: {} structs, {} enums, {} records",
            dcb_start.elapsed(),
            database.struct_definitions().len(),
            database.enum_definitions().len(),
            database.records().len()
        );

        // Export to XML (with incremental support)
        let main_records: Vec<_> = database.main_records().collect();

        // In incremental mode, filter out records that already have XML files
        let records_to_export: Vec<_> = if incremental {
            main_records
                .iter()
                .filter(|record| {
                    let file_name = database.record_file_name(record).unwrap_or("unknown.xml");
                    let output_path = path_mapper.resolve(output, file_name);
                    let output_path = output_path.with_extension("xml");
                    !output_path.exists()
                })
                .collect()
        } else {
            main_records.iter().collect()
        };

        let skipped_dcb = main_records.len() - records_to_export.len();
        if skipped_dcb > 0 {
            println!(
                "Exporting {} DataCore records ({} already exist, skipped)...",
                records_to_export.len(),
                skipped_dcb
            );
        } else {
            println!("Exporting {} DataCore records...", records_to_export.len());
        }

        if records_to_export.is_empty() {
            println!("All DataCore records already exported, nothing to do");
        } else {
            let dcb_pb = create_progress_bar(records_to_export.len() as u64, Stage::DcbExport);

            let exporter = svarog::XmlExporter::new(&database);
            let mut dcb_exported = 0;
            let mut dcb_errors = 0;

            for record in &records_to_export {
                let file_name = database.record_file_name(record).unwrap_or("unknown.xml");

                // Update progress with current file
                set_progress_message(&dcb_pb, Stage::DcbExport, file_name);

                // Use path mapper to merge with existing case
                let output_path = path_mapper.resolve(output, file_name);
                let output_path = output_path.with_extension("xml");

                // Create parent directories
                if let Some(parent) = output_path.parent() {
                    let _ = fs::create_dir_all(parent);
                }

                // Export record
                match exporter.export_record(record) {
                    Ok(xml) => {
                        if let Err(e) = fs::write(&output_path, xml) {
                            eprintln!("Failed to write {}: {}", file_name, e);
                            dcb_errors += 1;
                        } else {
                            dcb_exported += 1;
                        }
                    }
                    Err(e) => {
                        eprintln!("Error exporting {}: {}", file_name, e);
                        dcb_errors += 1;
                    }
                }

                dcb_pb.inc(1);
            }

            dcb_pb.finish_with_message("DCB export complete");
            println!(
                "Exported {} DataCore records ({} errors) in {:?}",
                dcb_exported,
                dcb_errors,
                dcb_start.elapsed()
            );
        }
    }

    Ok(())
}

fn cmd_p4k_list(
    p4k_path: &PathBuf,
    filter: Option<&str>,
    detailed: bool,
    json_output: bool,
) -> Result<()> {
    let archive = P4kArchive::open(p4k_path).context("Failed to open P4K archive")?;

    if json_output {
        let layout = archive.layout();
        let entries = archive
            .iter()
            .filter(|entry| filter.map_or(true, |pattern| glob_match(pattern, entry.name)))
            .map(|entry| {
                let payload_offset = archive.payload_offset(&entry).with_context(|| {
                    format!("Failed to resolve payload offset for {}", entry.name)
                })?;
                Ok(json!({
                    "name": entry.name,
                    "compressed_size": entry.compressed_size,
                    "uncompressed_size": entry.uncompressed_size,
                    "compression_method": entry.compression_method as u16,
                    "compression": format!("{:?}", entry.compression_method),
                    "encrypted": entry.is_encrypted,
                    "offset": entry.local_header_offset,
                    "offset_kind": match archive.version() {
                        svarog::p4k::P4kVersion::V1 => "local_header",
                        svarog::p4k::P4kVersion::V2 => "payload",
                    },
                    "payload_offset": payload_offset,
                    "crc32": format!("{:08x}", entry.crc32),
                    "last_mod_file_time": entry.last_mod_file_time,
                    "last_mod_file_date": entry.last_mod_file_date,
                    "signature": bytes_to_hex(&entry.signature),
                    "sha256": bytes_to_hex(&entry.sha256),
                    "bytes_already_written": entry.bytes_already_written,
                }))
            })
            .collect::<Result<Vec<_>>>()?;
        let freelist_blocks = archive
            .freelist_blocks()
            .iter()
            .map(|block| {
                json!({
                    "offset": block.offset,
                    "size": block.size,
                })
            })
            .collect::<Vec<_>>();
        let document = json!({
            "path": p4k_path,
            "version": format!("{:?}", archive.version()),
            "entry_count": archive.entry_count(),
            "matched_entry_count": entries.len(),
            "layout": {
                "file_size": layout.file_size,
                "actual_content_end": layout.actual_content_end,
                "physical_sector_size": layout.physical_sector_size,
                "cdr_offset": layout.cdr_offset,
                "cdr_size": layout.cdr_size,
                "name_table_offset": layout.name_table_offset,
                "name_table_size": layout.name_table_size,
                "end_of_payload": layout.end_of_payload,
                "install_block_offset": layout.install_block_offset,
                "install_block_size": layout.install_block_size,
                "eocd_offset": layout.eocd_offset,
                "manifest_sha256": layout.manifest_sha256.as_ref().map(|bytes| bytes_to_hex(bytes)),
                "v1_payload_placement": archive.v1_payload_placement_kind(),
            },
            "freelist_blocks": freelist_blocks,
            "entries": entries,
        });
        println!("{}", serde_json::to_string_pretty(&document)?);
        return Ok(());
    }

    let mut count = 0;
    for entry in archive.iter() {
        if let Some(pattern) = filter {
            if !glob_match(pattern, entry.name) {
                continue;
            }
        }

        if detailed {
            println!(
                "{:>12} {:>12} {} {}",
                entry.compressed_size,
                entry.uncompressed_size,
                if entry.is_encrypted { "E" } else { " " },
                entry.name
            );
        } else {
            println!("{}", entry.name);
        }
        count += 1;
    }

    println!("\nTotal: {} entries", count);

    Ok(())
}

fn bytes_to_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0F) as usize] as char);
    }
    out
}

fn cmd_p4k_dump(
    p4k_path: &PathBuf,
    output: &PathBuf,
    parallel: usize,
    raw_payloads: bool,
) -> Result<()> {
    println!("Dumping P4K archive: {}", p4k_path.display());
    let archive = P4kArchive::open(p4k_path).context("Failed to open P4K archive")?;
    println!(
        "Version: {:?}, entries: {}",
        archive.version(),
        archive.entry_count()
    );
    run_with_rayon_threads(parallel, || {
        if raw_payloads {
            archive
                .dump_raw_payloads_to_dir(output)
                .context("Failed to dump raw P4K payloads")
        } else {
            archive
                .dump_to_dir(output)
                .context("Failed to dump P4K archive")
        }
    })?;
    println!("Dumped to {}", output.display());
    Ok(())
}

fn cmd_p4k_verify(p4k_path: &PathBuf, raw_sha_only: bool) -> Result<()> {
    println!("Verifying P4K archive: {}", p4k_path.display());
    let archive = P4kArchive::open(p4k_path).context("Failed to open P4K archive")?;
    println!(
        "Version: {:?}, entries: {}",
        archive.version(),
        archive.entry_count()
    );

    let pb = create_progress_bar(archive.entry_count() as u64, Stage::P4kVerify);
    if raw_sha_only {
        archive
            .verify_payloads_sha256_physical_order_parallel_with_progress(|index, name| {
                update_p4k_verify_progress(&pb, index, name);
            })
            .context("P4K raw payload SHA-256 check failed")?;
    } else {
        archive
            .verify_integrity_parallel_with_progress(|index, name| {
                update_p4k_verify_progress(&pb, index, name);
            })
            .context("P4K integrity check failed")?;
    }
    pb.finish_with_message("P4K verification complete");
    println!("Verified {} entries", archive.entry_count());
    Ok(())
}

fn update_p4k_verify_progress(pb: &ProgressBar, index: usize, name: &str) {
    if index % 1024 == 0 {
        set_progress_message(pb, Stage::P4kVerify, name);
    }
    pb.inc(1);
}

#[allow(clippy::too_many_arguments)]
fn cmd_p4k_create(
    input: &PathBuf,
    output: &PathBuf,
    compression: CliP4kCompression,
    version: CliP4kVersion,
    sector_size: u64,
    zstd_level: i32,
    encrypt: bool,
    manifest_sha256: Option<&str>,
    parallel: usize,
) -> Result<()> {
    if !input.is_dir() {
        anyhow::bail!("Input path is not a directory: {}", input.display());
    }
    let options = svarog::p4k::P4kWriterOptions {
        compression: compression.into(),
        sector_size,
        zstd_level,
        manifest_sha256: parse_manifest_sha256_option(manifest_sha256)?,
        ..Default::default()
    };

    let mut files = WalkDir::new(input)
        .into_iter()
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().is_file())
        .map(|entry| {
            let path = entry.into_path();
            let relative = path
                .strip_prefix(input)
                .with_context(|| format!("Failed to relativize {}", path.display()))?;
            let archive_name = relative.to_string_lossy().replace('/', "\\");
            let sort_key = p4k_archive_name_sort_key(&archive_name);
            Ok((path, archive_name, sort_key))
        })
        .collect::<Result<Vec<_>>>()?;
    files.sort_by(|a, b| {
        a.2.cmp(&b.2)
            .then_with(|| a.1.cmp(&b.1))
            .then_with(|| a.0.cmp(&b.0))
    });

    println!(
        "Creating P4K {:?}: {} files, compression={:?}, encrypted={}",
        version,
        files.len(),
        compression,
        encrypt
    );
    let pb = create_progress_bar(files.len() as u64, Stage::P4kExtract);

    let mut builder = svarog::p4k::P4kBuilder::with_options(options.clone());
    if parallel == 1 {
        for (index, (path, archive_name, _)) in files.iter().enumerate() {
            if index % 1024 == 0 {
                set_progress_message(&pb, Stage::P4kExtract, archive_name);
            }
            if encrypt {
                builder
                    .add_file_encrypted(path, archive_name)
                    .with_context(|| format!("Failed to add {}", path.display()))?;
            } else {
                builder
                    .add_file(path, archive_name)
                    .with_context(|| format!("Failed to add {}", path.display()))?;
            }
            pb.inc(1);
        }
    } else {
        use rayon::prelude::*;

        let staged_entries: Vec<_> = run_with_rayon_threads(parallel, || {
            files
                .par_iter()
                .enumerate()
                .map(|(index, (path, archive_name, _))| {
                    if index % 1024 == 0 {
                        set_progress_message(&pb, Stage::P4kExtract, archive_name);
                    }
                    let staged = svarog::p4k::P4kBuilder::stage_file_with_options(
                        &options,
                        path,
                        archive_name,
                        encrypt,
                    )
                    .with_context(|| format!("Failed to add {}", path.display()));
                    pb.inc(1);
                    staged
                })
                .collect::<Result<Vec<_>>>()
        })?;
        builder.append_staged(staged_entries);
    }
    pb.finish_with_message("P4K create complete");

    let stats = match version {
        CliP4kVersion::V1 => builder
            .write_v1_to_file(output)
            .context("Failed to write P4K v1 archive")?,
        CliP4kVersion::V2 => builder
            .write_to_file(output)
            .context("Failed to write P4K v2 archive")?,
    };
    println!(
        "Wrote {} entries to {} ({} bytes, CDR at 0x{:X})",
        stats.entry_count,
        output.display(),
        stats.file_size,
        stats.cdr_offset
    );
    Ok(())
}

fn p4k_archive_name_sort_key(name: &str) -> String {
    let mut out = Vec::with_capacity(name.len());
    for &byte in name.as_bytes() {
        let mut value = if byte == b'\\' { b'/' } else { byte };
        if value.is_ascii_uppercase() {
            value = value.to_ascii_lowercase();
        }
        out.push(value);
    }
    while out.last() == Some(&b' ') {
        out.pop();
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn cmd_p4k_convert_v2(
    input: &PathBuf,
    output: &PathBuf,
    sector_size: u64,
    manifest_sha256: Option<&str>,
    in_place: bool,
) -> Result<()> {
    let options = svarog::p4k::P4kWriterOptions {
        sector_size,
        manifest_sha256: parse_manifest_sha256_option(manifest_sha256)?,
        ..Default::default()
    };
    let mut active_pb: Option<ProgressBar> = None;
    let progress = |event| {
        use svarog::p4k::{P4kConvertCopyMethod, P4kConvertProgress};

        match event {
            P4kConvertProgress::OpeningSource => {
                finish_progress_bar(&mut active_pb, "Starting conversion");
                active_pb = Some(create_spinner(
                    Stage::P4kConvert,
                    format!("Opening {}", input.display()),
                ));
            }
            P4kConvertProgress::SourceOpened {
                entry_count,
                source_file_size,
            } => {
                finish_progress_bar(
                    &mut active_pb,
                    format!("Opened source: {entry_count} entries, {source_file_size} bytes"),
                );
            }
            P4kConvertProgress::ScanningEntries { entry_count } => {
                active_pb = Some(create_progress_bar(entry_count as u64, Stage::P4kConvert));
                if let Some(pb) = &active_pb {
                    pb.set_message("[CONVERT] Scanning source payload metadata");
                }
            }
            P4kConvertProgress::ScanningProgress {
                scanned,
                total,
                name,
            } => {
                if let Some(pb) = &active_pb {
                    pb.set_length(total as u64);
                    pb.set_position(scanned as u64);
                    if let Some(name) = name {
                        set_progress_message(pb, Stage::P4kConvert, &name);
                    }
                }
            }
            P4kConvertProgress::ScanningFinished { entry_count } => {
                finish_progress_bar(
                    &mut active_pb,
                    format!("Scanned {entry_count} source entries"),
                );
            }
            P4kConvertProgress::PlanningEntries { entry_count } => {
                active_pb = Some(create_progress_bar(entry_count as u64, Stage::P4kConvert));
                if let Some(pb) = &active_pb {
                    pb.set_message("[CONVERT] Planning v2 metadata");
                }
            }
            P4kConvertProgress::PlanningProgress { planned, total } => {
                if let Some(pb) = &active_pb {
                    pb.set_length(total as u64);
                    pb.set_position(planned as u64);
                }
            }
            P4kConvertProgress::PlanningFinished {
                entry_count,
                payload_bytes,
                freelist_blocks,
            } => {
                finish_progress_bar(
                    &mut active_pb,
                    format!(
                        "Planned {entry_count} entries, {payload_bytes} payload bytes, {freelist_blocks} free ranges"
                    ),
                );
            }
            P4kConvertProgress::CopyStarted { bytes } => {
                active_pb = Some(create_bytes_progress_bar(
                    bytes,
                    Stage::P4kConvert,
                    "Preserving payload prefix",
                ));
            }
            P4kConvertProgress::CopyProgress { copied, total } => {
                if let Some(pb) = &active_pb {
                    pb.set_length(total);
                    pb.set_position(copied);
                }
            }
            P4kConvertProgress::CopyFinished { bytes, method } => {
                let method = match method {
                    P4kConvertCopyMethod::Reflink => "reflink",
                    P4kConvertCopyMethod::DirectIo => "parallel O_DIRECT",
                    P4kConvertCopyMethod::CopyFileRange => "copy_file_range",
                    P4kConvertCopyMethod::Buffered => "buffered copy",
                    P4kConvertCopyMethod::InPlace => "in-place (no copy)",
                    P4kConvertCopyMethod::Empty => "no payload",
                };
                finish_progress_bar(
                    &mut active_pb,
                    format!("Preserved {bytes} payload bytes via {method}"),
                );
            }
            P4kConvertProgress::TailStarted {
                entry_count,
                freelist_blocks,
            } => {
                active_pb = Some(create_spinner(
                    Stage::P4kConvert,
                    format!(
                        "Writing v2 tail for {entry_count} entries and {freelist_blocks} free ranges"
                    ),
                ));
            }
            P4kConvertProgress::TailFinished {
                file_size,
                cdr_offset,
            } => {
                finish_progress_bar(
                    &mut active_pb,
                    format!("Wrote v2 tail: {file_size} bytes, CDR at 0x{cdr_offset:X}"),
                );
            }
            P4kConvertProgress::Publishing => {
                active_pb = Some(create_spinner(
                    Stage::P4kConvert,
                    format!("Publishing {}", output.display()),
                ));
            }
            P4kConvertProgress::Finished { stats } => {
                finish_progress_bar(
                    &mut active_pb,
                    format!("Converted {} entries", stats.entry_count),
                );
            }
        }
    };

    let stats = if in_place {
        svarog::p4k::convert_v1_to_v2_in_place_with_progress(input, output, options, progress)
    } else {
        svarog::p4k::convert_v1_to_v2_with_progress(input, output, options, progress)
    }
    .context("Failed to convert P4K v1 to v2")?;
    println!(
        "Converted {} entries to {} ({} bytes, CDR at 0x{:X})",
        stats.entry_count,
        output.display(),
        stats.file_size,
        stats.cdr_offset
    );
    Ok(())
}

fn finish_progress_bar(pb: &mut Option<ProgressBar>, message: impl Into<String>) {
    if let Some(pb) = pb.take() {
        pb.finish_with_message(message.into());
    }
}

fn cmd_p4k_rewrite_v2_tail(p4k: &PathBuf) -> Result<()> {
    let stats =
        svarog::p4k::rewrite_v2_tail_in_place(p4k).context("Failed to rewrite P4K v2 tail")?;
    println!(
        "Rewrote v2 tail for {} ({} entries, {} bytes, CDR at 0x{:X})",
        p4k.display(),
        stats.entry_count,
        stats.file_size,
        stats.cdr_offset
    );
    Ok(())
}

fn parse_manifest_sha256_option(value: Option<&str>) -> Result<[u8; 64]> {
    let Some(value) = value else {
        return Ok([0; 64]);
    };

    let cleaned: String = value
        .chars()
        .filter(|ch| !ch.is_ascii_whitespace() && *ch != ':')
        .collect();
    if cleaned.len() != 64 && cleaned.len() != 128 {
        anyhow::bail!(
            "--manifest-sha256 must contain 64 or 128 hex characters, got {}",
            cleaned.len()
        );
    }

    let mut out = [0u8; 64];
    for (index, chunk) in cleaned.as_bytes().chunks_exact(2).enumerate() {
        out[index] = parse_hex_byte(chunk).with_context(|| {
            format!(
                "invalid --manifest-sha256 hex byte at character {}",
                index * 2
            )
        })?;
    }
    Ok(out)
}

fn parse_hex_byte(bytes: &[u8]) -> Result<u8> {
    debug_assert_eq!(bytes.len(), 2);
    let hi = parse_hex_nibble(bytes[0])?;
    let lo = parse_hex_nibble(bytes[1])?;
    Ok((hi << 4) | lo)
}

fn parse_hex_nibble(byte: u8) -> Result<u8> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => anyhow::bail!("not a hex digit"),
    }
}

fn cmd_cryxml_convert(input: &PathBuf, output: &PathBuf) -> Result<()> {
    println!(
        "Converting CryXmlB to XML: {} -> {}",
        input.display(),
        output.display()
    );

    let data = fs::read(input).context("Failed to read input file")?;

    if !CryXml::is_cryxml(&data) {
        anyhow::bail!("Input file is not a CryXmlB file");
    }

    let cryxml = CryXml::parse(&data).context("Failed to parse CryXmlB")?;
    let xml = cryxml.to_xml_string().context("Failed to convert to XML")?;
    fs::write(output, xml).context("Failed to write output file")?;

    println!("Conversion complete");

    Ok(())
}

fn cmd_cryxml_create(input: &PathBuf, output: &PathBuf) -> Result<()> {
    use svarog::cryxml::builder::CryXmlBuilder;

    println!(
        "Converting XML to CryXmlB: {} -> {}",
        input.display(),
        output.display()
    );

    let xml = fs::read_to_string(input).context("Failed to read input file")?;

    let builder = CryXmlBuilder::from_xml(&xml).context("Failed to parse XML")?;
    let cryxml_bytes = builder.build().context("Failed to build CryXmlB")?;
    fs::write(output, cryxml_bytes).context("Failed to write output file")?;

    println!(
        "Conversion complete ({} bytes)",
        fs::metadata(output)?.len()
    );

    Ok(())
}

fn cmd_dcb_extract(input: &PathBuf, output: &PathBuf, filter: Option<&str>) -> Result<()> {
    println!("Loading DataCore: {}", input.display());

    let start = Instant::now();
    let database = DataCoreDatabase::open(input).context("Failed to parse DataCore")?;

    println!(
        "Loaded in {:?}: {} structs, {} enums, {} records",
        start.elapsed(),
        database.struct_definitions().len(),
        database.enum_definitions().len(),
        database.records().len()
    );

    // Count main records
    let main_records: Vec<_> = database.main_records().collect();
    let filtered_records: Vec<_> = if let Some(pattern) = filter {
        main_records
            .into_iter()
            .filter(|r| {
                database
                    .record_file_name(r)
                    .map(|name| glob_match(pattern, name))
                    .unwrap_or(false)
            })
            .collect()
    } else {
        main_records
    };

    println!(
        "Exporting {} records to {}...",
        filtered_records.len(),
        output.display()
    );

    fs::create_dir_all(output)?;

    let exporter = svarog::XmlExporter::new(&database);
    let pb = ProgressBar::new(filtered_records.len() as u64);
    pb.set_style(
        ProgressStyle::default_bar()
            .template(
                "{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} ({eta})",
            )?
            .progress_chars("#>-"),
    );

    let start = Instant::now();
    let mut exported = 0;
    let mut errors = 0;

    for record in &filtered_records {
        let file_name = database.record_file_name(record).unwrap_or("unknown.xml");

        // Convert path separators and add .xml extension
        let output_path = output.join(file_name.replace('/', std::path::MAIN_SEPARATOR_STR));
        let output_path = output_path.with_extension("xml");

        // Create parent directories
        if let Some(parent) = output_path.parent() {
            fs::create_dir_all(parent)?;
        }

        // Export record
        match exporter.export_record(record) {
            Ok(xml) => {
                fs::write(&output_path, xml)?;
                exported += 1;
            }
            Err(e) => {
                eprintln!("Error exporting {}: {}", file_name, e);
                errors += 1;
            }
        }

        pb.inc(1);
    }

    pb.finish_with_message("Done");
    println!(
        "Exported {} records in {:?} ({} errors)",
        exported,
        start.elapsed(),
        errors
    );

    Ok(())
}

fn cmd_chf_process(input: &PathBuf, output: &PathBuf) -> Result<()> {
    use svarog::chf::parts::ChfData;

    println!(
        "Processing CHF: {} -> {}",
        input.display(),
        output.display()
    );

    let chf = if input.extension().and_then(|e| e.to_str()) == Some("chf") {
        ChfFile::from_chf(input).context("Failed to read CHF file")?
    } else {
        ChfFile::from_bin(input, true).context("Failed to read BIN file")?
    };

    println!(
        "Loaded CHF: {} bytes, modded: {}",
        chf.data().len(),
        chf.is_modded()
    );

    // Parse and display character data
    if let Ok(data) = ChfData::parse(chf.data()) {
        println!("Version: {}", data.version());
        println!("Model tag: {}", data.model_tag());
        println!("Voice tag: {}", data.voice_tag());

        // Show DNA summary
        let mut active_blends = 0;
        for (face_part, blends) in data.dna().iter_face_parts() {
            let blend_count = blends.iter().filter(|b| !b.is_zero()).count();
            if blend_count > 0 {
                active_blends += blend_count;
                println!("  {}: {} active blends", face_part, blend_count);
            }
        }
        println!("DNA: {} total active blends", active_blends);

        // Show item port tree if present
        if let Some(port) = data.item_port() {
            println!("Item ports: {} total, depth {}", port.count(), port.depth());
        }

        // Show materials
        if !data.materials().is_empty() {
            println!("Materials: {}", data.materials().len());
        }

        if !data.decals().is_empty() {
            println!("Decals: {}", data.decals().len());
        }
    }

    if output.extension().and_then(|e| e.to_str()) == Some("chf") {
        chf.write_to_chf(output)
            .context("Failed to write CHF file")?;
    } else {
        chf.write_to_bin(output)
            .context("Failed to write BIN file")?;
    }

    println!("Output written");

    Ok(())
}

fn cmd_dds_merge(input: &PathBuf, output: &PathBuf) -> Result<()> {
    println!("Merging DDS: {} -> {}", input.display(), output.display());

    let merged = merge_dds(input).context("Failed to merge DDS files")?;
    fs::write(output, merged).context("Failed to write output file")?;

    println!("Merge complete");

    Ok(())
}

fn cmd_dcb_schema(input: &PathBuf, output: &PathBuf) -> Result<()> {
    use svarog::datacore::CHeaderExporter;

    println!("Loading DataCore: {}", input.display());

    let start = Instant::now();
    let db = DataCoreDatabase::open(input).context("Failed to parse DataCore")?;

    println!(
        "Loaded in {:?}: {} structs, {} enums",
        start.elapsed(),
        db.struct_definitions().len(),
        db.enum_definitions().len()
    );

    println!("Generating C header schema...");

    let exporter = CHeaderExporter::new(&db);
    let header = exporter.export_all();

    fs::write(output, &header).context("Failed to write output file")?;

    println!(
        "Exported {} structs and {} enums to {}",
        db.struct_definitions().len(),
        db.enum_definitions().len(),
        output.display()
    );

    Ok(())
}

/// Simple glob matching for filtering.
fn glob_match(pattern: &str, name: &str) -> bool {
    // Convert glob pattern to a simple contains check for now
    // A proper implementation would use the `glob` crate
    let pattern_lower = pattern.to_lowercase();
    let name_lower = name.to_lowercase();

    if pattern_lower.contains('*') {
        // Handle * wildcard
        let parts: Vec<&str> = pattern_lower.split('*').collect();
        let mut pos = 0;

        for (i, part) in parts.iter().enumerate() {
            if part.is_empty() {
                continue;
            }

            if let Some(found) = name_lower[pos..].find(part) {
                if i == 0 && found != 0 {
                    // First part must match at start if no leading *
                    return false;
                }
                pos += found + part.len();
            } else {
                return false;
            }
        }

        // If pattern ends with *, any remaining is ok
        // If not, must have consumed the whole string
        parts.last().map_or(true, |p| p.is_empty()) || pos == name_lower.len()
    } else {
        name_lower.contains(&pattern_lower)
    }
}

/// Decode file data to text, handling CryXML.
fn decode_to_text(data: &[u8]) -> String {
    if CryXml::is_cryxml(data) {
        if let Ok(cryxml) = CryXml::parse(data) {
            if let Ok(xml) = cryxml.to_xml_string() {
                return xml;
            }
        }
    }
    String::from_utf8_lossy(data).to_string()
}

/// Compare two P4K archives.
fn cmd_p4k_compare(
    p4k1: &PathBuf,
    p4k2: &PathBuf,
    added_only: bool,
    removed_only: bool,
    modified_only: bool,
    show_diff: bool,
    filter: Option<&str>,
) -> Result<()> {
    use svarog::p4k::{compare_archives, is_text_file, P4kArchive};
    use svarog_common::{generate_unified_diff, DiffLineKind};

    println!("Comparing P4K archives:");
    println!("  Old: {}", p4k1.display());
    println!("  New: {}", p4k2.display());

    let start = Instant::now();

    println!("Opening archives...");
    let old_archive = P4kArchive::open(p4k1).context("Failed to open old P4K")?;
    let new_archive = P4kArchive::open(p4k2).context("Failed to open new P4K")?;

    println!(
        "Comparing {} vs {} files...",
        old_archive.entry_count(),
        new_archive.entry_count()
    );
    let result = compare_archives(&old_archive, &new_archive);

    println!("\nComparison complete in {:?}", start.elapsed());

    let show_all = !added_only && !removed_only && !modified_only;

    // Print summary
    println!(
        "\n  \x1b[32m+{} added\x1b[0m, \x1b[31m-{} removed\x1b[0m, \x1b[33m~{} modified\x1b[0m\n",
        result.added.len(),
        result.removed.len(),
        result.modified.len()
    );

    // Print added files
    if show_all || added_only {
        for item in &result.added {
            if let Some(pattern) = filter {
                if !glob_match(pattern, &item.path) {
                    continue;
                }
            }
            println!(
                "\x1b[32m+ {}\x1b[0m ({} bytes)",
                item.path,
                item.size_new.unwrap_or(0)
            );
        }
    }

    // Print removed files
    if show_all || removed_only {
        for item in &result.removed {
            if let Some(pattern) = filter {
                if !glob_match(pattern, &item.path) {
                    continue;
                }
            }
            println!(
                "\x1b[31m- {}\x1b[0m ({} bytes)",
                item.path,
                item.size_old.unwrap_or(0)
            );
        }
    }

    // Print modified files
    if show_all || modified_only {
        for item in &result.modified {
            if let Some(pattern) = filter {
                if !glob_match(pattern, &item.path) {
                    continue;
                }
            }
            println!(
                "\x1b[33m~ {}\x1b[0m ({} -> {} bytes)",
                item.path,
                item.size_old.unwrap_or(0),
                item.size_new.unwrap_or(0)
            );

            // Show diff for text files if requested
            if show_diff && is_text_file(&item.path) {
                if let (Some(old_entry), Some(new_entry)) =
                    (old_archive.find(&item.path), new_archive.find(&item.path))
                {
                    if let (Ok(old_data), Ok(new_data)) =
                        (old_archive.read(&old_entry), new_archive.read(&new_entry))
                    {
                        let old_text = decode_to_text(&old_data);
                        let new_text = decode_to_text(&new_data);

                        let diff = generate_unified_diff(
                            &old_text,
                            &new_text,
                            &format!("a/{}", item.path),
                            &format!("b/{}", item.path),
                            3,
                        );

                        for line in &diff.lines {
                            let colored = match line.kind {
                                DiffLineKind::Added => format!("\x1b[32m{}\x1b[0m", line.content),
                                DiffLineKind::Removed => format!("\x1b[31m{}\x1b[0m", line.content),
                                DiffLineKind::Header => format!("\x1b[36m{}\x1b[0m", line.content),
                                DiffLineKind::Context => line.content.clone(),
                            };
                            println!("{}", colored);
                        }
                        println!();
                    }
                }
            }
        }
    }

    Ok(())
}

/// Compare two DCB databases.
/// Load DCB data from a file path, extracting from P4K if necessary
fn load_dcb_data(path: &PathBuf) -> Result<Vec<u8>> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");

    if ext.eq_ignore_ascii_case("p4k") {
        // Load from P4K archive
        let archive = P4kArchive::open(path).context("Failed to open P4K archive")?;

        // Try to find DCB file inside
        let dcb_names = ["Data/Game.dcb", "Data/Game2.dcb", "Game.dcb", "Game2.dcb"];
        for name in &dcb_names {
            if let Some(entry) = archive.find(name) {
                let dcb_data = archive
                    .read(&entry)
                    .context("Failed to read DCB from P4K")?;
                println!("  (extracted {} from P4K)", name);
                return Ok(dcb_data);
            }
        }
        anyhow::bail!("No DCB file found in P4K archive (tried: {:?})", dcb_names);
    } else {
        // Load directly
        fs::read(path).context("Failed to read DCB file")
    }
}

#[allow(clippy::too_many_arguments)]
fn cmd_dcb_compare(
    dcb1: &PathBuf,
    dcb2: &PathBuf,
    scope: &str,
    added_only: bool,
    removed_only: bool,
    modified_only: bool,
    show_diff: bool,
    filter: Option<&str>,
) -> Result<()> {
    use svarog::datacore::{
        compare_databases, get_enum_content, get_record_content, get_struct_content,
        DcbCompareScope, DcbItemType,
    };
    use svarog_common::{generate_unified_diff, DiffLineKind};

    println!("Comparing DCB databases:");
    println!("  Old: {}", dcb1.display());
    println!("  New: {}", dcb2.display());

    let start = Instant::now();

    println!("Loading databases...");
    let old_data = load_dcb_data(dcb1)?;
    let new_data = load_dcb_data(dcb2)?;

    let old_db = DataCoreDatabase::parse(&old_data).context("Failed to parse old DCB")?;
    let new_db = DataCoreDatabase::parse(&new_data).context("Failed to parse new DCB")?;

    let compare_scope = DcbCompareScope::from_str(scope);

    println!("Comparing (scope: {:?})...", compare_scope);
    let result = compare_databases(&old_db, &new_db, compare_scope);

    println!("\nComparison complete in {:?}", start.elapsed());
    println!(
        "  Old: {} records, {} structs, {} enums",
        result.old_counts.0, result.old_counts.1, result.old_counts.2
    );
    println!(
        "  New: {} records, {} structs, {} enums",
        result.new_counts.0, result.new_counts.1, result.new_counts.2
    );

    let show_all = !added_only && !removed_only && !modified_only;

    // Print summary
    println!(
        "\n  \x1b[32m+{} added\x1b[0m, \x1b[31m-{} removed\x1b[0m, \x1b[33m~{} modified\x1b[0m\n",
        result.added.len(),
        result.removed.len(),
        result.modified.len()
    );

    // Print added items
    if show_all || added_only {
        for item in &result.added {
            if let Some(pattern) = filter {
                if !glob_match(pattern, &item.name) {
                    continue;
                }
            }
            println!("\x1b[32m+ [{}] {}\x1b[0m", item.item_type, item.name);

            if show_diff {
                if let Some(new_idx) = item.new_index {
                    let content = match item.item_type {
                        DcbItemType::Record => get_record_content(&new_db, new_idx),
                        DcbItemType::Struct => get_struct_content(&new_db, new_idx),
                        DcbItemType::Enum => get_enum_content(&new_db, new_idx),
                    };
                    if let Some(content) = content {
                        for line in content.lines() {
                            println!("\x1b[32m+{}\x1b[0m", line);
                        }
                        println!();
                    }
                }
            }
        }
    }

    // Print removed items
    if show_all || removed_only {
        for item in &result.removed {
            if let Some(pattern) = filter {
                if !glob_match(pattern, &item.name) {
                    continue;
                }
            }
            println!("\x1b[31m- [{}] {}\x1b[0m", item.item_type, item.name);

            if show_diff {
                if let Some(old_idx) = item.old_index {
                    let content = match item.item_type {
                        DcbItemType::Record => get_record_content(&old_db, old_idx),
                        DcbItemType::Struct => get_struct_content(&old_db, old_idx),
                        DcbItemType::Enum => get_enum_content(&old_db, old_idx),
                    };
                    if let Some(content) = content {
                        for line in content.lines() {
                            println!("\x1b[31m-{}\x1b[0m", line);
                        }
                        println!();
                    }
                }
            }
        }
    }

    // Print modified items
    if show_all || modified_only {
        for item in &result.modified {
            if let Some(pattern) = filter {
                if !glob_match(pattern, &item.name) {
                    continue;
                }
            }
            println!("\x1b[33m~ [{}] {}\x1b[0m", item.item_type, item.name);

            if show_diff {
                if let (Some(old_idx), Some(new_idx)) = (item.old_index, item.new_index) {
                    let old_content = match item.item_type {
                        DcbItemType::Record => get_record_content(&old_db, old_idx),
                        DcbItemType::Struct => get_struct_content(&old_db, old_idx),
                        DcbItemType::Enum => get_enum_content(&old_db, old_idx),
                    };
                    let new_content = match item.item_type {
                        DcbItemType::Record => get_record_content(&new_db, new_idx),
                        DcbItemType::Struct => get_struct_content(&new_db, new_idx),
                        DcbItemType::Enum => get_enum_content(&new_db, new_idx),
                    };

                    if let (Some(old_content), Some(new_content)) = (old_content, new_content) {
                        let diff = generate_unified_diff(
                            &old_content,
                            &new_content,
                            &format!("a/{}", item.name),
                            &format!("b/{}", item.name),
                            3,
                        );

                        for line in &diff.lines {
                            let colored = match line.kind {
                                DiffLineKind::Added => format!("\x1b[32m{}\x1b[0m", line.content),
                                DiffLineKind::Removed => format!("\x1b[31m{}\x1b[0m", line.content),
                                DiffLineKind::Header => format!("\x1b[36m{}\x1b[0m", line.content),
                                DiffLineKind::Context => line.content.clone(),
                            };
                            println!("{}", colored);
                        }
                        println!();
                    }
                }
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn prefix_capture_writer_collects_first_eight_bytes_across_writes() {
        let mut writer = PrefixCaptureWriter::new(Vec::new());

        writer.write_all(b"Cry").unwrap();
        writer.write_all(b"XmlB\0payload").unwrap();

        assert_eq!(writer.captured_prefix(), CRYXML_MAGIC);
        assert_eq!(writer.inner, b"CryXmlB\0payload");
    }

    #[test]
    fn cryxml_declared_min_len_uses_header_section_bounds() {
        let mut header = [0u8; CRYXML_HEADER_SIZE];
        header[4..8].copy_from_slice(&44u32.to_le_bytes());
        header[8..12].copy_from_slice(&2u32.to_le_bytes());
        header[12..16].copy_from_slice(&100u32.to_le_bytes());
        header[16..20].copy_from_slice(&3u32.to_le_bytes());
        header[20..24].copy_from_slice(&80u32.to_le_bytes());
        header[24..28].copy_from_slice(&4u32.to_le_bytes());
        header[28..32].copy_from_slice(&140u32.to_le_bytes());
        header[32..36].copy_from_slice(&12u32.to_le_bytes());

        assert_eq!(
            cryxml_declared_min_len(&header),
            Some(140 + 12),
            "string table end should dominate this synthetic header"
        );
        assert!(cryxml_header_fits_member(&header, 152));
        assert!(!cryxml_header_fits_member(&header, 151));
    }

    #[test]
    fn ascii_suffix_match_avoids_case_fold_allocation() {
        assert!(ends_with_ascii_ignore_case(
            "Data/Textures/Pack.SOCPAK",
            ".socpak"
        ));
        assert!(ends_with_ascii_ignore_case("Data/Game.DCB", ".dcb"));
        assert!(!ends_with_ascii_ignore_case("Data/Game.dcba", ".dcb"));
        assert!(!ends_with_ascii_ignore_case("d", ".dcb"));
    }

    #[test]
    fn extract_socpak_streams_non_cryxml_members_after_prefix_probe() {
        let mut zip_bytes = Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut zip_bytes);
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            zip.start_file("Data/test.bin", options).unwrap();
            zip.write_all(b"not-cryxml-payload").unwrap();
            zip.finish().unwrap();
        }

        let out_dir =
            std::env::temp_dir().join(format!("svarog-socpak-stream-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&out_dir);
        let result = extract_socpak(Cursor::new(zip_bytes.get_ref()), &out_dir, None).unwrap();

        assert_eq!(result.files_extracted, 1);
        assert_eq!(result.cryxml_decoded, 0);
        assert_eq!(
            fs::read(out_dir.join("Data/test.bin")).unwrap(),
            b"not-cryxml-payload"
        );

        let _ = fs::remove_dir_all(&out_dir);
    }

    #[test]
    fn extract_socpak_streams_bogus_cryxml_prefix_without_buffering_member_for_parse() {
        let mut bogus = Vec::new();
        bogus.extend_from_slice(CRYXML_MAGIC);
        let mut header = [0u8; CRYXML_HEADER_SIZE];
        header[28..32].copy_from_slice(&1_000_000u32.to_le_bytes());
        header[32..36].copy_from_slice(&64u32.to_le_bytes());
        bogus.extend_from_slice(&header);
        bogus.extend_from_slice(b"raw payload after bogus header");

        let mut zip_bytes = Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut zip_bytes);
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            zip.start_file("Data/bogus.xml", options).unwrap();
            zip.write_all(&bogus).unwrap();
            zip.finish().unwrap();
        }

        let out_dir = std::env::temp_dir().join(format!(
            "svarog-socpak-bogus-cryxml-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&out_dir);
        let result = extract_socpak(Cursor::new(zip_bytes.get_ref()), &out_dir, None).unwrap();

        assert_eq!(result.files_extracted, 1);
        assert_eq!(result.cryxml_decoded, 0);
        assert_eq!(fs::read(out_dir.join("Data/bogus.xml")).unwrap(), bogus);

        let _ = fs::remove_dir_all(&out_dir);
    }
}
