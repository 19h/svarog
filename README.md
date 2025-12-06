# Svarog

A high-performance Rust library and CLI for extracting and parsing Star Citizen game files.

## Features

- **P4K Archive Extraction** - Read Star Citizen's ZIP64 archives with AES encryption and Zstandard compression
- **DataCore Database Parsing** - Parse `.dcb` game database files and export to XML
- **CryXmlB Conversion** - Convert binary XML files (`.mtl`, `.cdf`, `.chrparams`) to standard XML
- **Character File Parsing** - Read and analyze `.chf` character head files
- **DDS Mipmap Merging** - Merge split DDS texture files

## Performance

Svarog is heavily optimized for maximum throughput:

- **SIMD-accelerated** null padding detection (AVX2/SSE2)
- **Zero-copy** memory-mapped file access
- **Parallel extraction** with rayon (with `parallel` feature)
- **FxHashMap** for O(1) lookups with fast hashing
- **String interning** to minimize allocations
- **AES-NI** hardware acceleration for decryption

## Installation

```bash
# Clone the repository
git clone https://github.com/diogotr7/Svarog.git
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
# Convert a single file
svarog cryxml-convert -i material.mtl -o material.xml

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

## Crate Structure

| Crate | Description |
|-------|-------------|
| `svarog` | Umbrella crate re-exporting all functionality |
| `svarog-common` | Binary reading, CigGuid, CRC32C utilities |
| `svarog-p4k` | P4K archive reader (ZIP64 + AES + Zstd) |
| `svarog-cryxml` | CryXmlB binary XML parser |
| `svarog-datacore` | DCB database parser + XML export |
| `svarog-chf` | Character head file parser |
| `svarog-dds` | DDS mipmap merger |

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
