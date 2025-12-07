//! Svarog CLI - Command-line tool for Star Citizen game file extraction.
//!
//! This is the main entry point for the Svarog command-line application.

use std::collections::HashMap;
use std::fs;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Instant;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use indicatif::{ProgressBar, ProgressStyle};

use svarog::prelude::*;

/// Progress stage for detailed visualization
#[derive(Clone, Copy)]
enum Stage {
    P4kExtract,
    SocpakExpand,
    CryXmlDecode,
    DcbExport,
}

impl Stage {
    fn prefix(self) -> &'static str {
        match self {
            Stage::P4kExtract => "P4K",
            Stage::SocpakExpand => "SOCPAK",
            Stage::CryXmlDecode => "CryXML",
            Stage::DcbExport => "DCB",
        }
    }

    fn color(self) -> &'static str {
        match self {
            Stage::P4kExtract => "cyan",
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

/// Check if data looks like CryXML (first 8 bytes are "CryXmlB\0")
fn is_cryxml_data(data: &[u8]) -> bool {
    CryXml::is_cryxml(data)
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
        Commands::P4kList { p4k, filter, detailed } => {
            cmd_p4k_list(&p4k, filter.as_deref(), detailed)?;
        }
        Commands::CryxmlConvert { input, output } => {
            cmd_cryxml_convert(&input, &output)?;
        }
        Commands::CryxmlCreate { input, output } => {
            cmd_cryxml_create(&input, &output)?;
        }
        Commands::DcbExtract { input, output, filter } => {
            cmd_dcb_extract(&input, &output, filter.as_deref())?;
        }
        Commands::ChfProcess { input, output } => {
            cmd_chf_process(&input, &output)?;
        }
        Commands::DdsMerge { input, output } => {
            cmd_dds_merge(&input, &output)?;
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
        let components: Vec<&str> = relative.split(['/', '\\']).filter(|s| !s.is_empty()).collect();

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

/// Extract a SOCPAK (which is just a ZIP file) to a directory.
/// Also decodes any CryXML files found inside.
fn extract_socpak(data: &[u8], output_dir: &Path, pb: Option<&ProgressBar>) -> Result<SocpakExtractionResult> {
    let cursor = Cursor::new(data);
    let mut archive = zip::ZipArchive::new(cursor)
        .context("Failed to open SOCPAK as ZIP archive")?;

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

        let mut contents = Vec::new();
        file.read_to_end(&mut contents)?;

        // Check if this is a CryXML file and decode it
        if is_cryxml_data(&contents) {
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
        }

        fs::write(&output_path, contents)?;
        extracted += 1;
    }

    Ok(SocpakExtractionResult {
        files_extracted: extracted,
        cryxml_decoded,
    })
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

    if &magic != b"CryXmlB\0" {
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

/// Check if a directory contains any files (recursively).
/// Returns false for empty directories or directories containing only empty subdirectories.
fn has_any_files(dir: &Path) -> bool {
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() {
                return true;
            } else if path.is_dir() && has_any_files(&path) {
                return true;
            }
        }
    }
    false
}

fn cmd_p4k_extract(
    p4k_path: &PathBuf,
    output: &PathBuf,
    filter: Option<&str>,
    use_regex: bool,
    incremental: bool,
    extract_dcb: bool,
    expand_socpak: bool,
    _parallel: usize,
) -> Result<()> {
    println!("Opening P4K archive: {}", p4k_path.display());

    let start = Instant::now();
    let archive = P4kArchive::open(p4k_path).context("Failed to open P4K archive")?;

    println!("Loaded {} entries in {:?}", archive.entry_count(), start.elapsed());

    // Compile regex if using regex mode
    let regex_filter = if use_regex {
        filter.map(|p| regex::Regex::new(p).context("Invalid regex pattern")).transpose()?
    } else {
        None
    };

    // Collect matching indices
    let entries: Vec<_> = if let Some(ref re) = regex_filter {
        archive
            .iter()
            .enumerate()
            .filter(|(_, e)| re.is_match(e.name))
            .map(|(i, e)| (i, e.name.to_string(), e.uncompressed_size))
            .collect()
    } else if let Some(pattern) = filter {
        archive
            .iter()
            .enumerate()
            .filter(|(_, e)| glob_match(pattern, e.name))
            .map(|(i, e)| (i, e.name.to_string(), e.uncompressed_size))
            .collect()
    } else {
        archive
            .iter()
            .enumerate()
            .map(|(i, e)| (i, e.name.to_string(), e.uncompressed_size))
            .collect()
    };

    println!("Extracting {} entries from P4K...", entries.len());

    // Find all DCB entries if extraction is requested
    let dcb_entries: Vec<(usize, String)> = if extract_dcb {
        archive
            .iter()
            .enumerate()
            .filter(|(_, e)| e.name.to_lowercase().ends_with(".dcb"))
            .map(|(i, e)| (i, e.name.to_string()))
            .collect()
    } else {
        Vec::new()
    };

    if !dcb_entries.is_empty() {
        println!(
            "Found {} DCB file(s) - will extract and process DataCore",
            dcb_entries.len()
        );
    }

    // Track ALL SOCPAK directories for CryXML post-processing check
    let mut all_socpak_dirs: Vec<PathBuf> = Vec::new();

    let pb = create_progress_bar(entries.len() as u64, Stage::P4kExtract);

    fs::create_dir_all(output)?;

    // Statistics
    let extracted = AtomicU64::new(0);
    let skipped = AtomicU64::new(0);
    let socpak_expanded = AtomicU64::new(0);
    let cryxml_decoded = AtomicU64::new(0);
    let errors = AtomicU64::new(0);

    // Path mapper for case-insensitive merging
    let path_mapper = CaseInsensitivePathMapper::new();

    let start = Instant::now();

    for (idx, name, size) in &entries {
        let name_normalized = name.replace('\\', "/");
        let output_path = path_mapper.resolve(output, &name_normalized);

        // Check if this is a SOCPAK file
        let is_socpak = expand_socpak && name_normalized.to_lowercase().ends_with(".socpak");

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
            let dominated = should_skip_file(&output_path, *size);
            if dominated {
                // File exists with matching size - but check if it's undecoded CryXML
                if check_and_decode_cryxml(&output_path) {
                    cryxml_decoded.fetch_add(1, Ordering::Relaxed);
                }
            }
            !dominated
        } else {
            true
        };

        if !should_extract {
            skipped.fetch_add(1, Ordering::Relaxed);
            pb.inc(1);
            continue;
        }

        // Update progress with current file
        set_progress_message(&pb, Stage::P4kExtract, &name_normalized);

        // Create parent directories
        if let Some(parent) = output_path.parent() {
            if let Err(e) = fs::create_dir_all(parent) {
                eprintln!("Failed to create directory {}: {}", parent.display(), e);
                errors.fetch_add(1, Ordering::Relaxed);
                pb.inc(1);
                continue;
            }
        }

        // Read entry data
        let data = match archive.read_index(*idx) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("Failed to read {}: {}", name, e);
                errors.fetch_add(1, Ordering::Relaxed);
                pb.inc(1);
                continue;
            }
        };

        if let Some(socpak_dir) = socpak_dir {
            // Extract SOCPAK contents to a directory with the same basename
            set_progress_message(&pb, Stage::SocpakExpand, &name_normalized);

            match extract_socpak(&data, &socpak_dir, Some(&pb)) {
                Ok(result) => {
                    socpak_expanded.fetch_add(result.files_extracted as u64, Ordering::Relaxed);
                    cryxml_decoded.fetch_add(result.cryxml_decoded as u64, Ordering::Relaxed);
                    extracted.fetch_add(1, Ordering::Relaxed);
                    // Don't write the .socpak file - we've extracted it
                }
                Err(e) => {
                    eprintln!("Failed to extract SOCPAK {}: {}", name, e);
                    // Fall back to writing the raw file
                    if let Err(e) = fs::write(&output_path, &data) {
                        eprintln!("Failed to write {}: {}", name, e);
                        errors.fetch_add(1, Ordering::Relaxed);
                    } else {
                        extracted.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
        } else {
            // Write regular file, decoding CryXML if applicable
            let data_to_write = if is_cryxml_data(&data) {
                // Decode CryXML to text XML
                set_progress_message(&pb, Stage::CryXmlDecode, &name_normalized);
                match CryXml::parse(&data) {
                    Ok(cryxml) => match cryxml.to_xml_string() {
                        Ok(xml) => {
                            cryxml_decoded.fetch_add(1, Ordering::Relaxed);
                            xml.into_bytes()
                        }
                        Err(_) => data, // Fall back to raw data
                    },
                    Err(_) => data, // Fall back to raw data
                }
            } else {
                data
            };

            if let Err(e) = fs::write(&output_path, data_to_write) {
                eprintln!("Failed to write {}: {}", name, e);
                errors.fetch_add(1, Ordering::Relaxed);
            } else {
                extracted.fetch_add(1, Ordering::Relaxed);
            }
        }

        pb.inc(1);
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
                let file_name = database
                    .record_file_name(record)
                    .unwrap_or("unknown.xml");

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

fn cmd_p4k_list(p4k_path: &PathBuf, filter: Option<&str>, detailed: bool) -> Result<()> {
    let archive = P4kArchive::open(p4k_path).context("Failed to open P4K archive")?;

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

fn cmd_cryxml_convert(input: &PathBuf, output: &PathBuf) -> Result<()> {
    println!("Converting CryXmlB to XML: {} -> {}", input.display(), output.display());

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

    println!("Converting XML to CryXmlB: {} -> {}", input.display(), output.display());

    let xml = fs::read_to_string(input).context("Failed to read input file")?;

    let builder = CryXmlBuilder::from_xml(&xml).context("Failed to parse XML")?;
    let cryxml_bytes = builder.build().context("Failed to build CryXmlB")?;
    fs::write(output, cryxml_bytes).context("Failed to write output file")?;

    println!("Conversion complete ({} bytes)", fs::metadata(output)?.len());

    Ok(())
}

fn cmd_dcb_extract(input: &PathBuf, output: &PathBuf, filter: Option<&str>) -> Result<()> {
    println!("Loading DataCore: {}", input.display());

    let start = Instant::now();
    let data = fs::read(input).context("Failed to read input file")?;
    let database = DataCoreDatabase::parse(&data).context("Failed to parse DataCore")?;

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

    println!("Exporting {} records to {}...", filtered_records.len(), output.display());

    fs::create_dir_all(output)?;

    let exporter = svarog::XmlExporter::new(&database);
    let pb = ProgressBar::new(filtered_records.len() as u64);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} ({eta})")?
            .progress_chars("#>-"),
    );

    let start = Instant::now();
    let mut exported = 0;
    let mut errors = 0;

    for record in &filtered_records {
        let file_name = database
            .record_file_name(record)
            .unwrap_or("unknown.xml");

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

    println!("Processing CHF: {} -> {}", input.display(), output.display());

    let chf = if input.extension().and_then(|e| e.to_str()) == Some("chf") {
        ChfFile::from_chf(input).context("Failed to read CHF file")?
    } else {
        ChfFile::from_bin(input, true).context("Failed to read BIN file")?
    };

    println!("Loaded CHF: {} bytes, modded: {}", chf.data().len(), chf.is_modded());

    // Parse and display character data
    if let Ok(data) = ChfData::parse(chf.data()) {
        println!("Gender ID: {}", data.gender_id());

        // Show DNA summary
        let mut active_blends = 0;
        for (face_part, blends) in data.dna().iter_face_parts() {
            let blend_count = blends.iter().filter(|b| !b.is_zero()).count();
            if blend_count > 0 {
                active_blends += blend_count;
                println!(
                    "  {}: {} active blends",
                    face_part, blend_count
                );
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
    }

    if output.extension().and_then(|e| e.to_str()) == Some("chf") {
        chf.write_to_chf(output).context("Failed to write CHF file")?;
    } else {
        chf.write_to_bin(output).context("Failed to write BIN file")?;
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
