# Svarog

A high-performance Rust library and CLI for extracting and parsing Star Citizen game files.

## Features

- **P4K Archive Read/Write** - Full support for both legacy ZIP64-based v1 and modern v2 ("JiJi") archive formats
  - Reader auto-detects v1 (ZIP64 + `CI` EOCD comment) and v2 (`0xAF` EOCDR, `JiJi` magic) layouts
  - Writer (`P4kBuilder`) for creating v1 or v2 archives with Store, raw DEFLATE, zlib-wrapped DEFLATE, and Zstandard compression
  - AES-128-CBC encryption support (read and write)
  - Fast v1 → v2 conversion preserving payloads, freelists, install progress, and metadata
  - Optional 64-byte manifest digest pass-through for the v2 EOCDR
  - Automatic SOCPAK expansion (extracts nested ZIP archives inline)
  - Automatic CryXML decoding during extraction
  - Incremental extraction (skip unchanged files)
  - Empty directory detection and re-extraction
  - Archive diffing with unified text diff for XML/text files
- **DataCore Database** - Full read/write support for `.dcb` game database files
  - Versions 5, 6, and v8 layout supported (newer Star Citizen builds)
  - High-level Query API for searching records
  - DOM-like Instance API for property access (with lazy resolution of inline Class arrays)
  - DataCoreBuilder for creating/modifying databases
  - XML export with all properties resolved
  - C header export for structs/enums (IDA-compatible, self-contained)
  - Database diffing across records, structs, and enums with unified diff output
- **CryXmlB Read/Write** - Full round-trip support for binary XML files
  - Parse `.mtl`, `.cdf`, `.chrparams`, `.adb`, `.animevents`, `.bspace`, `.xml`
  - Convert to/from standard XML text
  - Programmatic construction via builder API
- **Character File Parsing** - Read and analyze `.chf` character head files
  - Supports newer CHF format versions
- **DDS Mipmap Merging** - Merge split DDS texture files

## GUI Application

Svarog includes a graphical interface for browsing P4K archives and DataCore databases.

| P4K Browser | DataCore Browser |
|:-----------:|:----------------:|
| <img width=100% alt="P4K Browser" src="https://github.com/user-attachments/assets/c7931874-f320-422a-a486-a3d9f4af05df" /> | <img width=100% alt="DataCore Browser" src="https://github.com/user-attachments/assets/67b18a53-ceb2-4f2e-b870-a6e6b1c2033a" /> |

### Download

Pre-built binaries are available in [Releases](https://github.com/19h/svarog/releases) for:

| Platform | Architectures |
|----------|---------------|
| Linux | x86_64, ARM64 |
| macOS | x86_64 (Intel), ARM64 (Apple Silicon) |
| Windows | x86_64, ARM64 |

> **Note:** Linux ARM64 and Windows ARM64 builds include only the CLI due to cross-compilation constraints.

### GUI Features

**P4K Browser**
- Browse and search P4K archive contents with a file tree
- Preview files directly: text, XML (with syntax highlighting), hex view
- Automatic CryXML decoding for binary XML files
- Extract individual files or entire directories

**DataCore Browser**
- Three browsing modes: Records, Structs, and Enums
- **Records View**: Browse all records organized by type hierarchy
  - Search records by name with real-time filtering
  - Filter by record type (click type badges to filter)
  - XML content viewer with line numbers and syntax highlighting
  - Reference navigation: click references to jump between related records
  - Incoming/outgoing reference tracking with counts
- **Structs View**: Browse C-style struct definitions
  - Type reference counts showing usage across the database
  - Export structs as C headers (IDA-compatible)
- **Enums View**: Browse C-style enum definitions with usage counts
- Navigation history with back/forward (mouse buttons, Alt+Left/Right)
- Alternating row backgrounds (zebra striping) in all tree views
- Text selection with non-copyable line numbers

**Comparison Tools**
- Diff two P4K archives or two DCB databases side-by-side
- Unified diff for XML/text files inside P4K
- DCB diff across records, structs, and enums

## Performance

Svarog is heavily optimized for maximum throughput with cross-platform SIMD acceleration:

### SIMD Support

| Architecture | Instruction Sets | Operations |
|-------------|------------------|------------|
| x86_64 | AVX2, SSE2 | Null detection, pattern search, slice comparison |
| aarch64 | NEON | Null detection, pattern search, slice comparison |
| Other | Scalar (u64) | Fallback with optimized u64 reads |

All SIMD features are **runtime-detected** on x86_64, ensuring optimal performance on any CPU.

### Optimizations

- **SIMD-accelerated** null padding detection and byte searching (via memchr)
- **Zero-copy** memory-mapped file access
- **Parallel extraction** with rayon (with `parallel` feature)
- **FxHashMap** for O(1) lookups with fast hashing
- **String interning** with arena allocation to minimize allocations
- **AES-NI** hardware acceleration for decryption
- **CRC32C** hardware acceleration (SSE4.2 on x86, ARMv8 CRC)

## Supported Platforms

| Platform | Architecture | Status |
|----------|-------------|--------|
| Linux | x86_64 | Full SIMD (AVX2/SSE2) |
| Linux | aarch64 | Full SIMD (NEON) |
| macOS | x86_64 | Full SIMD (AVX2/SSE2) |
| macOS | aarch64 (Apple Silicon) | Full SIMD (NEON) |
| Windows | x86_64 | Full SIMD (AVX2/SSE2) |
| Windows | aarch64 | Full SIMD (NEON) |

## Installation

```bash
# Clone the repository
git clone https://github.com/19h/Svarog.git
cd Svarog

# Build release version with all optimizations
cargo build --release --all-features

# The binary will be at ./target/release/svarog
```

## CLI Usage

### P4K Archive Operations

```bash
# List all files in a P4K archive (auto-detects v1 or v2)
svarog p4k-list -p /path/to/Data.p4k

# List with size, encryption flag, and other details
svarog p4k-list -p Data.p4k --detailed

# Dump archive/entry metadata as JSON: methods, offsets, CRC, SHA-256, signatures, install progress
svarog p4k-list -p Data.p4k --json

# Filter by pattern
svarog p4k-list -p Data.p4k --filter "*.xml"

# Smart extraction: SOCPAK expansion, CryXML decoding, DCB → XML, incremental
svarog p4k-extract -p Data.p4k -o ./output

# Extract with filter (glob or regex)
svarog p4k-extract -p Data.p4k -o ./output --filter "Data/Scripts/*.lua"
svarog p4k-extract -p Data.p4k -o ./output --filter "Data/Ships/.*" --regex

# Raw dump of every file with no post-processing
svarog p4k-dump -p Data.p4k -o ./raw
svarog p4k-dump -p Data.p4k -o ./raw -j 8

# Verify raw payload SHA-256 and decoded CRC32C metadata
svarog p4k-verify -p Data.p4k

# Fast verification of raw compressed/encrypted payload SHA-256 only
svarog p4k-verify -p Data.p4k --raw-sha-only

# Create a P4K archive from a directory (default: v2, Zstandard)
svarog p4k-create -i ./content -o NewArchive.p4k
svarog p4k-create -i ./content -o NewArchive.p4k --version v1 --compression deflate
svarog p4k-create -i ./content -o Encrypted.p4k --encrypt --compression zstd
svarog p4k-create -i ./content -o XboxStyle.p4k --compression deflate-zlib

# Convert an existing v1 archive to the v2 ("JiJi") format
svarog p4k-convert-v2 -i Old.p4k -o New.p4k

# Diff two archives (with optional unified diff for XML/text files)
svarog p4k-compare --old Old.p4k --new New.p4k --diff
svarog p4k-compare --old Old.p4k --new New.p4k --modified-only --filter "*.xml"
```

Opt-in real-archive validation for the P4K crate:

```bash
# Open and classify one real archive
SVAROG_P4K_TEST_FILE=/path/to/Data.p4k cargo test -p svarog-p4k real_world_p4k_parses -- --nocapture

# Walk a corpus directory and open every .p4k file
SVAROG_P4K_TEST_DIR=/path/to/corpus cargo test -p svarog-p4k real_world_p4k_corpus_parses -- --nocapture

# Fast raw stored payload SHA-256 verification
SVAROG_P4K_TEST_SHA256=1 SVAROG_P4K_TEST_FILE=/path/to/Data.p4k cargo test --release -p svarog-p4k --features parallel real_world_p4k_parses -- --nocapture

# Add full stored SHA-256 plus decoded CRC32C verification
SVAROG_P4K_TEST_VERIFY=1 SVAROG_P4K_TEST_FILE=/path/to/Data.p4k cargo test -p svarog-p4k real_world_p4k_parses -- --nocapture
```

### DataCore Database Operations

```bash
# Extract all records to XML (output organized by record type)
svarog dcb-extract -i Game.dcb -o ./datacore

# Filter records by file name pattern
svarog dcb-extract -i Game.dcb -o ./datacore --filter "Ships/*"

# Export the full DataCore schema as a single C header (IDA-compatible)
svarog dcb-schema -i Game.dcb -o datacore_types.h

# Diff two DCB databases (DCB files or P4K archives containing them)
svarog dcb-compare --old Old.dcb --new New.dcb --diff
svarog dcb-compare --old Old.p4k --new New.p4k --scope records --modified-only
```

### CryXmlB Conversion

```bash
# Convert CryXmlB to XML
svarog cryxml-convert -i material.mtl -o material.xml

# Convert XML back to CryXmlB
svarog cryxml-create -i material.xml -o material.mtl
```

### Character File Processing

```bash
# Process a character file (CHF ↔ BIN, extension-driven)
svarog chf-process -i character.chf -o character.bin
svarog chf-process -i character.bin -o character.chf
```

### DDS Mipmap Merging

```bash
# Merge split DDS files (texture.dds, texture.dds.1, texture.dds.2, ...)
svarog dds-merge -i texture.dds -o merged.dds
```

## Library Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
svarog = "0.1"

# Or individual crates:
svarog-p4k = "0.1"
svarog-datacore = "0.1"
svarog-cryxml = "0.1"
```

### Example: Reading a P4K Archive

```rust
use svarog::prelude::*;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Open archive (memory-mapped for efficiency)
    let archive = P4kArchive::open("Data.p4k")?;

    println!("Archive contains {} entries", archive.entry_count());

    // Iterate over entries (zero-copy)
    for entry in archive.iter() {
        println!("{}: {} bytes", entry.name, entry.uncompressed_size);
    }

    // Read a specific file
    if let Some(entry) = archive.find("Data\\Game.dcb") {
        let data = archive.read(&entry)?;
        println!("Read {} bytes", data.len());
    }

    Ok(())
}
```

### Example: Parsing DataCore

```rust
use svarog::prelude::*;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Load database (memory-mapped)
    let database = DataCoreDatabase::open("Game.dcb")?;

    println!("Structs: {}", database.struct_definitions().len());
    println!("Records: {}", database.records().len());

    // Export all records to XML
    let exporter = XmlExporter::new(&database);
    exporter.export_all("./output", |current, total| {
        println!("Progress: {}/{}", current, total);
    })?;

    Ok(())
}
```

### Example: Parallel Extraction

```rust
use svarog_p4k::P4kArchive;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let archive = P4kArchive::open("Data.p4k")?;

    // Get indices of files to extract
    let indices: Vec<usize> = archive
        .iter()
        .enumerate()
        .filter(|(_, e)| e.name.ends_with(".xml"))
        .map(|(i, _)| i)
        .collect();

    // Extract in parallel (requires "parallel" feature)
    #[cfg(feature = "parallel")]
    archive.extract_parallel(&indices, |idx, name, result| {
        match result {
            Ok(data) => println!("Extracted {}: {} bytes", name, data.len()),
            Err(e) => eprintln!("Failed {}: {}", name, e),
        }
    })?;

    Ok(())
}
```

### Example: Writing a P4K Archive

```rust
use svarog::p4k::{P4kBuilder, P4kWriterOptions};
use svarog::p4k::zip::CompressionMethod;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut options = P4kWriterOptions::default();
    options.compression = CompressionMethod::Zstd;
    options.zstd_level = 1;

    let mut builder = P4kBuilder::with_options(options);
    builder.add_file("./content/material.mtl", "Data\\material.mtl")?;
    builder.add_file_encrypted("./content/secret.bin", "Data\\secret.bin")?;

    // Write a v2 ("JiJi") archive — use write_v1_to_file() for the legacy layout
    let stats = builder.write_to_file("NewArchive.p4k")?;
    println!("Wrote {} entries ({} bytes)", stats.entry_count, stats.file_size);
    Ok(())
}
```

### Example: Converting a P4K v1 Archive to v2

```rust
use svarog::p4k::{convert_v1_to_v2, P4kWriterOptions};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let stats = convert_v1_to_v2(
        "Legacy.p4k",
        "Modern.p4k",
        P4kWriterOptions::default(),
    )?;
    println!("Converted {} entries", stats.entry_count);
    Ok(())
}
```

### Example: Exporting C Headers

```rust
use svarog_datacore::{DataCoreDatabase, CHeaderExporter};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let database = DataCoreDatabase::open("Game.dcb")?;
    let exporter = CHeaderExporter::new(&database);

    // Export all structs and enums to a single header file
    let header = exporter.export_all();
    std::fs::write("datacore_types.h", header)?;

    // Or export specific structs by index
    let partial = exporter.export_structs(&[0, 1, 2]);

    Ok(())
}
```

The generated headers are self-contained (no `#include` required) and compatible with IDA's type parser.

### Example: Using SIMD Utilities

```rust
use svarog_common::simd;

// Find content end (skip null padding) - uses AVX2/SSE2/NEON
let data = vec![1u8; 1000];
let content_end = simd::find_content_end(&data);

// Check if slice is all zeros - SIMD accelerated
let zeros = vec![0u8; 1000];
assert!(simd::is_all_zeros(&zeros));

// Fast byte search via memchr
let pos = simd::find_byte(0x50, &data);
```

## Crate Structure

| Crate | Description |
|-------|-------------|
| `svarog` | Umbrella crate re-exporting all functionality |
| `svarog-common` | Binary reading, CigGuid, CRC32C, **SIMD utilities** |
| `svarog-p4k` | P4K archive reader (ZIP64 + AES + Zstd) |
| `svarog-cryxml` | CryXmlB binary XML parser + writer |
| `svarog-datacore` | DCB database parser + XML/C header export |
| `svarog-chf` | Character head file parser |
| `svarog-dds` | DDS mipmap merger |
| `svarog-gui` | GUI application (egui/eframe) |

## File Format Details

### P4K Archive

Two on-disk layouts are supported, auto-detected by the reader:

- **v1** — ZIP64-based with per-payload local file records, a 16-byte `CI` EOCD comment, and custom extra fields `0x0001`/`0x5000`/`0x5002`/`0x5003`
- **v2** ("JiJi") — local headers removed; payloads followed by an install/freelist block, a 64 KiB-aligned `0xCC`-byte CDR, a name table, and a fixed `0xAF`-byte EOCDR ending in `version=2` + magic `JiJi`

Shared properties:

- AES-128-CBC encryption with zero IV, applied after the selected compression step
- Store (method 0), raw DEFLATE (method 8), Zstandard (methods 93 and 100), and zlib-wrapped DEFLATE (method 101)
- Hardware-accelerated AES-NI and Zstd
- Optional 64-byte manifest SHA-256 digest stored in the v2 EOCDR

See `crates/svarog-p4k/P4K_FORMAT.md` for the full decompilation-backed layout notes.

### DataCore Database (DCB)

- Versions 5, 6, and v8 (newer Star Citizen builds) supported
- Contains struct definitions, properties, enums
- Value pools for all primitive types
- Records with GUID identifiers
- Two string tables
- Lazy resolution of inline Class arrays for correctness on v8 layouts

### CryXmlB

- Magic: `CryXmlB\0`
- Node tree with attributes
- String pool for names/values

### CHF Character Files

- Fixed 4096 bytes
- Zstd-compressed payload
- DNA face morphing data
- Equipment item ports
- Material customizations

## Building

```bash
# Debug build
cargo build

# Release build with all features
cargo build --release --all-features

# Run tests
cargo test --all --all-features

# Run benchmarks (if available)
cargo bench
```

## Feature Flags

| Feature | Description |
|---------|-------------|
| `parallel` | Enable rayon-based parallel processing |
| `xml-export` | Enable XML export for DataCore (default) |
| `json-export` | Enable JSON export for DataCore (default) |

## License

MIT License - See LICENSE file for details.

## Acknowledgments

This is a Rust port of the original .NET StarBreaker project, optimized for maximum performance, modularity and portability across all relevant platforms.
