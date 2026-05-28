use std::ffi::OsStr;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn svarog<I, S>(args: I) -> String
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let output = Command::new(env!("CARGO_BIN_EXE_svarog"))
        .args(args)
        .output()
        .expect("run svarog");
    assert!(
        output.status.success(),
        "svarog failed with status {:?}\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[test]
fn cli_create_verify_dump_and_convert_p4k_archives() {
    let root = temp_root("svarog_p4k_cli");
    let input = root.join("input");
    let nested = input.join("Data").join("Nested");
    fs::create_dir_all(&nested).unwrap();
    fs::write(input.join("readme.txt"), b"created through CLI\n").unwrap();
    fs::write(nested.join("blob.bin"), [0u8, 1, 2, 3, 4, 5, 0xFE]).unwrap();
    let manifest: [u8; 64] = std::array::from_fn(|i| 0xA0u8.wrapping_add(i as u8));
    let manifest_hex = manifest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();

    let v2 = root.join("created-v2.p4k");
    svarog([
        "p4k-create",
        "-i",
        input.to_str().unwrap(),
        "-o",
        v2.to_str().unwrap(),
        "--compression",
        "store",
        "--sector-size",
        "256",
        "--manifest-sha256",
        manifest_hex.as_str(),
    ]);
    assert_v2_manifest(&v2, &manifest);
    let verify_v2 = svarog(["p4k-verify", "-p", v2.to_str().unwrap()]);
    assert!(verify_v2.contains("Verified 2 entries"));
    let verify_v2_raw = svarog(["p4k-verify", "-p", v2.to_str().unwrap(), "--raw-sha-only"]);
    assert!(verify_v2_raw.contains("Verified 2 entries"));

    let list_v2 = svarog(["p4k-list", "-p", v2.to_str().unwrap()]);
    assert!(list_v2.contains("readme.txt"));
    assert!(list_v2.contains("Data/Nested/blob.bin"));
    let metadata_v2 = svarog([
        "p4k-list",
        "-p",
        v2.to_str().unwrap(),
        "--filter",
        "readme.txt",
        "--json",
    ]);
    let metadata_v2: serde_json::Value = serde_json::from_str(&metadata_v2).unwrap();
    assert_eq!(metadata_v2["version"], "V2");
    assert_eq!(metadata_v2["entry_count"], 2);
    assert_eq!(metadata_v2["matched_entry_count"], 1);
    assert_eq!(metadata_v2["layout"]["physical_sector_size"], 256);
    assert_eq!(metadata_v2["layout"]["manifest_sha256"], manifest_hex);
    assert_eq!(
        metadata_v2["layout"]["v1_payload_placement"],
        serde_json::Value::Null
    );
    assert_eq!(
        metadata_v2["layout"]["cdr_offset"].as_u64().unwrap() % 0x1_0000,
        0
    );
    assert!(metadata_v2["layout"]["end_of_payload"].as_u64().unwrap() > 0);
    assert_eq!(metadata_v2["freelist_blocks"].as_array().unwrap().len(), 0);
    assert_eq!(metadata_v2["entries"][0]["name"], "readme.txt");
    assert_eq!(metadata_v2["entries"][0]["compression_method"], 0);
    assert_eq!(metadata_v2["entries"][0]["compression"], "Store");
    assert_eq!(metadata_v2["entries"][0]["encrypted"], false);
    assert_eq!(metadata_v2["entries"][0]["offset_kind"], "payload");
    assert_eq!(
        metadata_v2["entries"][0]["payload_offset"],
        metadata_v2["entries"][0]["offset"]
    );
    assert_eq!(
        metadata_v2["entries"][0]["uncompressed_size"],
        b"created through CLI\n".len() as u64
    );
    assert_eq!(
        metadata_v2["entries"][0]["sha256"].as_str().unwrap().len(),
        64
    );
    assert_eq!(
        metadata_v2["entries"][0]["signature"]
            .as_str()
            .unwrap()
            .len(),
        256
    );

    let dump_v2 = root.join("dump-v2");
    svarog([
        "p4k-dump",
        "-p",
        v2.to_str().unwrap(),
        "-o",
        dump_v2.to_str().unwrap(),
        "-j",
        "2",
    ]);
    assert_eq!(
        fs::read(dump_v2.join("readme.txt")).unwrap(),
        b"created through CLI\n"
    );
    assert_eq!(
        fs::read(dump_v2.join("Data").join("Nested").join("blob.bin")).unwrap(),
        [0u8, 1, 2, 3, 4, 5, 0xFE]
    );

    let extract_v2 = root.join("extract-v2");
    let extract_v2_output = svarog([
        "p4k-extract",
        "-p",
        v2.to_str().unwrap(),
        "-o",
        extract_v2.to_str().unwrap(),
    ]);
    assert!(extract_v2_output.contains("Extracted 2 files"));
    assert_eq!(
        fs::read(extract_v2.join("readme.txt")).unwrap(),
        b"created through CLI\n"
    );
    assert_eq!(
        fs::read(extract_v2.join("Data").join("Nested").join("blob.bin")).unwrap(),
        [0u8, 1, 2, 3, 4, 5, 0xFE]
    );

    let v1 = root.join("created-v1.p4k");
    svarog([
        "p4k-create",
        "-i",
        input.to_str().unwrap(),
        "-o",
        v1.to_str().unwrap(),
        "--version",
        "v1",
        "--compression",
        "store",
        "--sector-size",
        "256",
    ]);
    let metadata_v1 = svarog([
        "p4k-list",
        "-p",
        v1.to_str().unwrap(),
        "--filter",
        "readme.txt",
        "--json",
    ]);
    let metadata_v1: serde_json::Value = serde_json::from_str(&metadata_v1).unwrap();
    assert_eq!(metadata_v1["version"], "V1");
    assert_eq!(metadata_v1["matched_entry_count"], 1);
    let verify_v1_raw = svarog(["p4k-verify", "-p", v1.to_str().unwrap(), "--raw-sha-only"]);
    assert!(verify_v1_raw.contains("Verified 2 entries"));
    assert_eq!(metadata_v1["layout"]["physical_sector_size"], 256);
    assert_eq!(
        metadata_v1["layout"]["v1_payload_placement"],
        "aligned_record"
    );
    assert_eq!(
        metadata_v1["layout"]["manifest_sha256"],
        serde_json::Value::Null
    );
    assert!(
        metadata_v1["layout"]["install_block_offset"]
            .as_u64()
            .unwrap()
            > 0
    );
    assert_eq!(metadata_v1["entries"][0]["offset_kind"], "local_header");
    assert!(
        metadata_v1["entries"][0]["payload_offset"]
            .as_u64()
            .unwrap()
            > metadata_v1["entries"][0]["offset"].as_u64().unwrap()
    );
    let converted = root.join("converted-v2.p4k");
    svarog([
        "p4k-convert-v2",
        "-i",
        v1.to_str().unwrap(),
        "-o",
        converted.to_str().unwrap(),
        "--sector-size",
        "256",
        "--manifest-sha256",
        manifest_hex.as_str(),
    ]);
    assert_v2_manifest(&converted, &manifest);
    let verify_converted = svarog(["p4k-verify", "-p", converted.to_str().unwrap()]);
    assert!(verify_converted.contains("Verified 2 entries"));
    let metadata_converted = svarog([
        "p4k-list",
        "-p",
        converted.to_str().unwrap(),
        "--filter",
        "readme.txt",
        "--json",
    ]);
    let metadata_converted: serde_json::Value = serde_json::from_str(&metadata_converted).unwrap();
    assert_eq!(metadata_converted["version"], "V2");
    assert_eq!(metadata_converted["entry_count"], 2);
    assert_eq!(metadata_converted["matched_entry_count"], 1);
    assert_eq!(metadata_converted["layout"]["physical_sector_size"], 256);
    assert_eq!(
        metadata_converted["layout"]["v1_payload_placement"],
        serde_json::Value::Null
    );
    assert_eq!(
        metadata_converted["layout"]["manifest_sha256"],
        manifest_hex
    );
    assert_eq!(metadata_converted["entries"][0]["offset_kind"], "payload");
    assert_eq!(
        metadata_converted["entries"][0]["payload_offset"],
        metadata_converted["entries"][0]["offset"]
    );
    assert_eq!(metadata_converted["entries"][0]["compression"], "Store");
    assert!(metadata_converted["freelist_blocks"]
        .as_array()
        .unwrap()
        .iter()
        .any(|block| block["size"].as_u64().unwrap() > 0));

    let dump_converted = root.join("dump-converted");
    svarog([
        "p4k-dump",
        "-p",
        converted.to_str().unwrap(),
        "-o",
        dump_converted.to_str().unwrap(),
    ]);
    assert_eq!(
        fs::read(dump_converted.join("readme.txt")).unwrap(),
        b"created through CLI\n"
    );
    assert_eq!(
        fs::read(dump_converted.join("Data").join("Nested").join("blob.bin")).unwrap(),
        [0u8, 1, 2, 3, 4, 5, 0xFE]
    );

    let encrypted_zstd = root.join("encrypted-zstd-v2.p4k");
    svarog([
        "p4k-create",
        "-i",
        input.to_str().unwrap(),
        "-o",
        encrypted_zstd.to_str().unwrap(),
        "--compression",
        "zstd",
        "--encrypt",
        "--sector-size",
        "256",
    ]);
    let verify_encrypted_zstd = svarog(["p4k-verify", "-p", encrypted_zstd.to_str().unwrap()]);
    assert!(verify_encrypted_zstd.contains("Verified 2 entries"));

    let dump_encrypted_zstd = root.join("dump-encrypted-zstd");
    svarog([
        "p4k-dump",
        "-p",
        encrypted_zstd.to_str().unwrap(),
        "-o",
        dump_encrypted_zstd.to_str().unwrap(),
    ]);
    assert_eq!(
        fs::read(dump_encrypted_zstd.join("readme.txt")).unwrap(),
        b"created through CLI\n"
    );
    assert_eq!(
        fs::read(
            dump_encrypted_zstd
                .join("Data")
                .join("Nested")
                .join("blob.bin")
        )
        .unwrap(),
        [0u8, 1, 2, 3, 4, 5, 0xFE]
    );
    let raw_dump_encrypted_zstd = root.join("raw-dump-encrypted-zstd");
    svarog([
        "p4k-dump",
        "-p",
        encrypted_zstd.to_str().unwrap(),
        "-o",
        raw_dump_encrypted_zstd.to_str().unwrap(),
        "--raw-payloads",
    ]);
    let raw_metadata = svarog([
        "p4k-list",
        "-p",
        encrypted_zstd.to_str().unwrap(),
        "--filter",
        "readme.txt",
        "--json",
    ]);
    let raw_metadata: serde_json::Value = serde_json::from_str(&raw_metadata).unwrap();
    let raw_entry = &raw_metadata["entries"][0];
    let raw_offset = raw_entry["payload_offset"].as_u64().unwrap() as usize;
    let raw_size = raw_entry["compressed_size"].as_u64().unwrap() as usize;
    let archive_bytes = fs::read(&encrypted_zstd).unwrap();
    let raw_file = fs::read(raw_dump_encrypted_zstd.join("readme.txt")).unwrap();
    assert_eq!(&raw_file, &archive_bytes[raw_offset..raw_offset + raw_size]);
    assert_ne!(raw_file, b"created through CLI\n");

    let extract_encrypted_zstd = root.join("extract-encrypted-zstd");
    let extract_encrypted_zstd_output = svarog([
        "p4k-extract",
        "-p",
        encrypted_zstd.to_str().unwrap(),
        "-o",
        extract_encrypted_zstd.to_str().unwrap(),
    ]);
    assert!(extract_encrypted_zstd_output.contains("Extracted 2 files"));
    assert_eq!(
        fs::read(extract_encrypted_zstd.join("readme.txt")).unwrap(),
        b"created through CLI\n"
    );
    assert_eq!(
        fs::read(
            extract_encrypted_zstd
                .join("Data")
                .join("Nested")
                .join("blob.bin")
        )
        .unwrap(),
        [0u8, 1, 2, 3, 4, 5, 0xFE]
    );

    let encrypted_zstd_v1 = root.join("encrypted-zstd-v1.p4k");
    svarog([
        "p4k-create",
        "-i",
        input.to_str().unwrap(),
        "-o",
        encrypted_zstd_v1.to_str().unwrap(),
        "--version",
        "v1",
        "--compression",
        "zstd",
        "--encrypt",
        "--sector-size",
        "256",
    ]);
    let converted_encrypted_zstd = root.join("converted-encrypted-zstd-v2.p4k");
    svarog([
        "p4k-convert-v2",
        "-i",
        encrypted_zstd_v1.to_str().unwrap(),
        "-o",
        converted_encrypted_zstd.to_str().unwrap(),
        "--sector-size",
        "256",
    ]);
    let verify_converted_encrypted_zstd = svarog([
        "p4k-verify",
        "-p",
        converted_encrypted_zstd.to_str().unwrap(),
    ]);
    assert!(verify_converted_encrypted_zstd.contains("Verified 2 entries"));

    let dump_converted_encrypted_zstd = root.join("dump-converted-encrypted-zstd");
    svarog([
        "p4k-dump",
        "-p",
        converted_encrypted_zstd.to_str().unwrap(),
        "-o",
        dump_converted_encrypted_zstd.to_str().unwrap(),
    ]);
    assert_eq!(
        fs::read(dump_converted_encrypted_zstd.join("readme.txt")).unwrap(),
        b"created through CLI\n"
    );
    assert_eq!(
        fs::read(
            dump_converted_encrypted_zstd
                .join("Data")
                .join("Nested")
                .join("blob.bin")
        )
        .unwrap(),
        [0u8, 1, 2, 3, 4, 5, 0xFE]
    );

    for (compression, file_name) in [
        ("deflate", "created-deflate-v2.p4k"),
        ("deflate-zlib", "created-deflate-zlib-v2.p4k"),
        ("zstd-deprecated", "created-zstd-deprecated-v2.p4k"),
    ] {
        let archive_path = root.join(file_name);
        svarog([
            "p4k-create",
            "-i",
            input.to_str().unwrap(),
            "-o",
            archive_path.to_str().unwrap(),
            "--compression",
            compression,
            "--sector-size",
            "256",
        ]);
        let verify = svarog(["p4k-verify", "-p", archive_path.to_str().unwrap()]);
        assert!(verify.contains("Verified 2 entries"));

        let dump_dir = root.join(format!("dump-{compression}"));
        svarog([
            "p4k-dump",
            "-p",
            archive_path.to_str().unwrap(),
            "-o",
            dump_dir.to_str().unwrap(),
        ]);
        assert_eq!(
            fs::read(dump_dir.join("readme.txt")).unwrap(),
            b"created through CLI\n"
        );
        assert_eq!(
            fs::read(dump_dir.join("Data").join("Nested").join("blob.bin")).unwrap(),
            [0u8, 1, 2, 3, 4, 5, 0xFE]
        );
    }

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn cli_create_orders_equal_size_files_by_normalized_archive_name() {
    let root = temp_root("svarog_p4k_cli_order");
    let input = root.join("input");
    fs::create_dir_all(&input).unwrap();
    fs::write(input.join("z.bin"), b"same").unwrap();
    fs::write(input.join("A.bin"), b"same").unwrap();
    fs::write(input.join("m.bin"), b"same").unwrap();

    let archive = root.join("ordered.p4k");
    svarog([
        "p4k-create",
        "-i",
        input.to_str().unwrap(),
        "-o",
        archive.to_str().unwrap(),
        "--compression",
        "store",
        "--sector-size",
        "256",
    ]);

    let metadata = svarog(["p4k-list", "-p", archive.to_str().unwrap(), "--json"]);
    let metadata: serde_json::Value = serde_json::from_str(&metadata).unwrap();
    let names: Vec<_> = metadata["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["A.bin", "m.bin", "z.bin"]);

    let _ = fs::remove_dir_all(&root);
}

fn temp_root(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!("{prefix}-{}-{nanos}", std::process::id()));
    fs::create_dir_all(&path).unwrap();
    path
}

fn assert_v2_manifest(path: &PathBuf, expected: &[u8; 64]) {
    let bytes = fs::read(path).unwrap();
    let eocdr_start = bytes.len() - 0xAF;
    assert_eq!(&bytes[eocdr_start + 0x69..eocdr_start + 0xA9], expected);
}
