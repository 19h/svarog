//! Svarog CLI - Command-line tool for Star Citizen game file extraction.
//!
//! This is the main entry point for the Svarog command-line application.

use std::fs;
use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use indicatif::{ProgressBar, ProgressStyle};

use svarog::prelude::*;

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
    /// Extract files from a P4K archive
    P4kExtract {
        /// Path to the P4K file
        #[arg(short, long, env = "INPUT_P4K")]
        p4k: PathBuf,

        /// Output directory
        #[arg(short, long, env = "OUTPUT_FOLDER")]
        output: PathBuf,

        /// Filter pattern (glob-style)
        #[arg(short, long)]
        filter: Option<String>,
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
        Commands::P4kExtract { p4k, output, filter } => {
            cmd_p4k_extract(&p4k, &output, filter.as_deref())?;
        }
        Commands::P4kList { p4k, filter, detailed } => {
            cmd_p4k_list(&p4k, filter.as_deref(), detailed)?;
        }
        Commands::CryxmlConvert { input, output } => {
            cmd_cryxml_convert(&input, &output)?;
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

fn cmd_p4k_extract(p4k_path: &PathBuf, output: &PathBuf, filter: Option<&str>) -> Result<()> {
    println!("Opening P4K archive: {}", p4k_path.display());

    let start = Instant::now();
    let archive = P4kArchive::open(p4k_path).context("Failed to open P4K archive")?;

    println!("Loaded {} entries in {:?}", archive.entry_count(), start.elapsed());

    // Collect matching indices
    let indices: Vec<usize> = if let Some(pattern) = filter {
        archive
            .iter()
            .enumerate()
            .filter(|(_, e)| glob_match(pattern, e.name))
            .map(|(i, _)| i)
            .collect()
    } else {
        (0..archive.entry_count()).collect()
    };

    println!("Extracting {} entries...", indices.len());

    let pb = ProgressBar::new(indices.len() as u64);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} ({eta})")?
            .progress_chars("#>-"),
    );

    fs::create_dir_all(output)?;

    let start = Instant::now();
    for idx in &indices {
        if let Some(entry) = archive.get(*idx) {
            let output_path = output.join(entry.name.replace('\\', "/"));

            if let Some(parent) = output_path.parent() {
                fs::create_dir_all(parent)?;
            }

            let data = archive.read(&entry)?;
            fs::write(&output_path, data)?;
        }

        pb.inc(1);
    }

    pb.finish_with_message("Done");
    println!("Extraction completed in {:?}", start.elapsed());

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
    println!("Converting: {} -> {}", input.display(), output.display());

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
