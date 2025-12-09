# Svarog

A high-performance Rust library and CLI for extracting and parsing Star Citizen game files.

## Features

- **P4K Archive Extraction** - Read Star Citizen's ZIP64 archives with AES encryption and Zstandard compression
  - Automatic SOCPAK expansion (extracts nested ZIP archives inline)
  - Automatic CryXML decoding during extraction
  - Incremental extraction (skip unchanged files)
  - Empty directory detection and re-extraction
- **DataCore Database** - Full read/write support for `.dcb` game database files
  - High-level Query API for searching records
  - DOM-like Instance API for property access
  - DataCoreBuilder for creating/modifying databases
  - XML export with all properties resolved
  - C header export for structs/enums (IDA-compatible, self-contained)
- **CryXmlB Read/Write** - Full round-trip support for binary XML files
  - Parse `.mtl`, `.cdf`, `.chrparams`, `.adb`, `.animevents`, `.bspace`, `.xml`
  - Convert to/from standard XML text
  - Programmatic construction via builder API
- **Character File Parsing** - Read and analyze `.chf` character head files
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
# List all files in a P4K archive
svarog p4k-list -p /path/to/Data.p4k

# List with size details
svarog p4k-list -p Data.p4k --detailed

# Filter by pattern
svarog p4k-list -p Data.p4k --filter "*.xml"

# Extract all files
svarog p4k-extract -p Data.p4k -o ./output

# Extract with filter
svarog p4k-extract -p Data.p4k -o ./output --filter "Data/Scripts/*.lua"
```

### DataCore Database Operations

```bash
# Extract all records to XML
svarog dcb-extract -i Game.dcb -o ./datacore

# The output will be organized by record type
```

### CryXmlB Conversion

```bash
# Convert CryXmlB to XML
svarog cryxml-convert -i material.mtl -o material.xml

# Convert XML back to CryXmlB
svarog cryxml-create -i material.xml -o material.mtl

# Convert all CryXmlB files in a directory
svarog cryxml-convert-all -i ./extracted -o ./converted
```

### Character File Processing

```bash
# Process a character file
svarog chf-process -i character.chf -o character.json
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

- ZIP64 format with custom extensions
- AES-128-CBC encryption (zero IV)
- Zstandard compression (method 100)
- DEFLATE compression (method 8)
- Custom extra fields (0x5000, 0x5002, 0x5003)

### DataCore Database (DCB)

- Versions 5 and 6 supported
- Contains struct definitions, properties, enums
- Value pools for all primitive types
- Records with GUID identifiers
- Two string tables

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
