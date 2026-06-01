//! Delta conversion: migrate v1 updates into an existing v2 archive.
//!
//! The fixtures build matched v1/v2 source pairs with `P4kBuilder` so the
//! tests stay portable (no real `Data.p4k` required). Each test then runs
//! `delta_convert_v1_into_v2` and asserts both the per-entry bookkeeping
//! (reused vs. taken vs. removed) and that the resulting v2 archive can
//! be opened and read back with the expected payload bytes.

#![allow(clippy::field_reassign_with_default)]

use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;

use svarog_p4k::zip::CompressionMethod;
use svarog_p4k::{
    convert_v1_to_v2, delta_convert_v1_into_v2, delta_convert_v1_into_v2_with_progress,
    P4kArchive, P4kBuilder, P4kDeltaProgress, P4kVersion, P4kWriterOptions,
};

/// Small sector size keeps test archives compact while still exercising
/// the alignment / install-block code paths in the writer.
const SECTOR_SIZE: u64 = 0x100;

fn tempfile_path(name: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!("{}-{}", std::process::id(), name));
    p
}

fn writer_options(compression: CompressionMethod) -> P4kWriterOptions {
    let mut options = P4kWriterOptions::default();
    options.sector_size = SECTOR_SIZE;
    options.compression = compression;
    options
}

/// Build a v1 archive with the given (name, payload) pairs using `P4kBuilder`.
fn build_v1(path: &std::path::Path, compression: CompressionMethod, entries: &[(&str, &[u8])]) {
    let options = writer_options(compression);
    let mut builder = P4kBuilder::with_options(options);
    for (name, payload) in entries {
        builder.add_bytes(*name, *payload).unwrap();
    }
    builder.write_v1_to_file(path).unwrap();
}

/// Build a v2 archive by writing v1 first, then converting it.
fn build_v2(path: &std::path::Path, compression: CompressionMethod, entries: &[(&str, &[u8])]) {
    let v1_tmp = tempfile_path(&format!(
        "delta-build-v2-via-v1-{}.p4k",
        path.file_name().unwrap().to_string_lossy()
    ));
    build_v1(&v1_tmp, compression, entries);
    convert_v1_to_v2(&v1_tmp, path, writer_options(compression)).unwrap();
    let _ = fs::remove_file(&v1_tmp);
}

/// Collect (normalized) name → payload for every base entry an archive
/// exposes, decoding compression + encryption via the public reader.
fn read_archive_payloads(path: &std::path::Path) -> Vec<(String, Vec<u8>)> {
    let archive = P4kArchive::open(path).unwrap();
    let mut out = Vec::with_capacity(archive.entry_count());
    for entry in archive.iter() {
        let bytes = archive.read(&entry).unwrap();
        out.push((entry.name.to_string(), bytes));
    }
    out
}

#[test]
fn identity_delta_keeps_every_entry_from_v2() {
    // Same content in both archives: every base entry should be reused
    // from the v2 source.
    let v2 = tempfile_path("delta_identity_v2.p4k");
    let v1 = tempfile_path("delta_identity_v1.p4k");
    let out = tempfile_path("delta_identity_out.p4k");

    let entries: &[(&str, &[u8])] = &[
        ("a/first.dat", b"AAAA"),
        ("a/second.dat", b"BBBBBB"),
        ("c/third.txt", b"third payload"),
    ];
    build_v2(&v2, CompressionMethod::Store, entries);
    build_v1(&v1, CompressionMethod::Store, entries);

    let stats = delta_convert_v1_into_v2(&v2, &v1, &out, writer_options(CompressionMethod::Store))
        .unwrap();

    assert_eq!(stats.entries_reused_from_existing_v2, entries.len());
    assert_eq!(stats.entries_taken_from_new_v1, 0);
    assert_eq!(stats.entries_removed, 0);
    assert_eq!(stats.bytes_taken_from_new_v1, 0);

    let result = P4kArchive::open(&out).unwrap();
    assert_eq!(result.version(), P4kVersion::V2);
    assert_eq!(result.entry_count(), entries.len());

    let payloads = read_archive_payloads(&out);
    for (name, data) in entries {
        let found = payloads
            .iter()
            .find(|(stored, _)| stored == name)
            .unwrap();
        assert_eq!(&found.1[..], *data);
    }

    let _ = fs::remove_file(&v2);
    let _ = fs::remove_file(&v1);
    let _ = fs::remove_file(&out);
}

#[test]
fn delta_takes_modified_and_added_entries_from_v1() {
    let v2 = tempfile_path("delta_modified_v2.p4k");
    let v1 = tempfile_path("delta_modified_v1.p4k");
    let out = tempfile_path("delta_modified_out.p4k");

    let v2_entries: &[(&str, &[u8])] = &[
        ("keep/same.txt", b"unchanged"),
        ("change/me.txt", b"OLD VERSION"),
        ("drop/me.txt", b"goodbye"),
    ];
    let v1_entries: &[(&str, &[u8])] = &[
        ("keep/same.txt", b"unchanged"),
        ("change/me.txt", b"new content here"),
        ("brand/new.txt", b"hello world"),
    ];

    build_v2(&v2, CompressionMethod::Store, v2_entries);
    build_v1(&v1, CompressionMethod::Store, v1_entries);

    let stats = delta_convert_v1_into_v2(&v2, &v1, &out, writer_options(CompressionMethod::Store))
        .unwrap();

    assert_eq!(stats.entries_reused_from_existing_v2, 1, "only keep/same.txt reuses v2 bytes");
    assert_eq!(stats.entries_taken_from_new_v1, 2, "modified + added come from v1");
    assert_eq!(stats.entries_removed, 1, "drop/me.txt was deleted in v1");

    let result = P4kArchive::open(&out).unwrap();
    assert_eq!(result.version(), P4kVersion::V2);
    assert_eq!(result.entry_count(), v1_entries.len());

    let payloads = read_archive_payloads(&out);
    for (name, data) in v1_entries {
        let found = payloads
            .iter()
            .find(|(stored, _)| stored == name)
            .unwrap_or_else(|| panic!("missing entry {name}"));
        assert_eq!(&found.1[..], *data, "payload mismatch for {name}");
    }
    // Deleted entry is gone.
    assert!(payloads.iter().all(|(name, _)| name != "drop/me.txt"));

    let _ = fs::remove_file(&v2);
    let _ = fs::remove_file(&v1);
    let _ = fs::remove_file(&out);
}

#[test]
fn delta_with_disjoint_v1_takes_everything_from_v1() {
    // No overlap between v1 and v2 → every v1 entry must be taken from v1.
    let v2 = tempfile_path("delta_disjoint_v2.p4k");
    let v1 = tempfile_path("delta_disjoint_v1.p4k");
    let out = tempfile_path("delta_disjoint_out.p4k");

    build_v2(
        &v2,
        CompressionMethod::Store,
        &[("only/in/v2.dat", b"old data only here")],
    );
    let v1_entries: &[(&str, &[u8])] = &[
        ("only/in/v1.dat", b"fresh data"),
        ("another/v1.bin", &[7u8; 32]),
    ];
    build_v1(&v1, CompressionMethod::Store, v1_entries);

    let stats = delta_convert_v1_into_v2(&v2, &v1, &out, writer_options(CompressionMethod::Store))
        .unwrap();
    assert_eq!(stats.entries_reused_from_existing_v2, 0);
    assert_eq!(stats.entries_taken_from_new_v1, v1_entries.len());
    assert_eq!(stats.entries_removed, 1);

    let result = P4kArchive::open(&out).unwrap();
    assert_eq!(result.entry_count(), v1_entries.len());
    let payloads = read_archive_payloads(&out);
    for (name, data) in v1_entries {
        let found = payloads
            .iter()
            .find(|(stored, _)| stored == name)
            .unwrap();
        assert_eq!(&found.1[..], *data);
    }

    let _ = fs::remove_file(&v2);
    let _ = fs::remove_file(&v1);
    let _ = fs::remove_file(&out);
}

#[test]
fn delta_preserves_zstd_payload_bytes() {
    // With Zstd compression, the v1 and v2 archives carry identical
    // compressed bytes for unchanged entries, so the reuse path must
    // produce a v2 file whose decoded payload still matches.
    let v2 = tempfile_path("delta_zstd_v2.p4k");
    let v1 = tempfile_path("delta_zstd_v1.p4k");
    let out = tempfile_path("delta_zstd_out.p4k");

    // Repeated-byte payloads compress well so we exercise the Zstd path
    // without depending on a particular compression ratio.
    let stable: Vec<u8> = (0..1024u16).map(|n| (n & 0xFF) as u8).collect();
    let changing_old: Vec<u8> = vec![0xAA; 512];
    let changing_new: Vec<u8> = vec![0x55; 512];
    let added: Vec<u8> = vec![0x33; 256];

    build_v2(
        &v2,
        CompressionMethod::Zstd,
        &[("stable.bin", &stable), ("changing.bin", &changing_old)],
    );
    build_v1(
        &v1,
        CompressionMethod::Zstd,
        &[
            ("stable.bin", &stable),
            ("changing.bin", &changing_new),
            ("added.bin", &added),
        ],
    );

    let stats = delta_convert_v1_into_v2(&v2, &v1, &out, writer_options(CompressionMethod::Zstd))
        .unwrap();
    assert_eq!(stats.entries_reused_from_existing_v2, 1);
    assert_eq!(stats.entries_taken_from_new_v1, 2);
    assert_eq!(stats.entries_removed, 0);

    let payloads = read_archive_payloads(&out);
    let by_name = |key: &str| -> Vec<u8> {
        payloads
            .iter()
            .find(|(name, _)| name == key)
            .unwrap()
            .1
            .clone()
    };
    assert_eq!(by_name("stable.bin"), stable);
    assert_eq!(by_name("changing.bin"), changing_new);
    assert_eq!(by_name("added.bin"), added);

    let _ = fs::remove_file(&v2);
    let _ = fs::remove_file(&v1);
    let _ = fs::remove_file(&out);
}

#[test]
fn delta_rejects_mismatched_source_versions() {
    // Both inputs are v2 → the existing path is fine but the "new v1"
    // path isn't, so the function must reject before writing anything.
    let existing_v2 = tempfile_path("delta_bad_existing_v2.p4k");
    let bad_v1 = tempfile_path("delta_bad_new_v1_is_actually_v2.p4k");
    let out = tempfile_path("delta_bad_versions_out.p4k");

    build_v2(&existing_v2, CompressionMethod::Store, &[("a.txt", b"x")]);
    build_v2(&bad_v1, CompressionMethod::Store, &[("b.txt", b"y")]);

    let err = delta_convert_v1_into_v2(
        &existing_v2,
        &bad_v1,
        &out,
        writer_options(CompressionMethod::Store),
    )
    .unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("not P4K v1"),
        "expected v1-version rejection, got: {msg}"
    );
    assert!(!out.exists(), "no output should have been written");

    // And the symmetric mistake (passing a v1 where v2 is expected).
    let v1_only = tempfile_path("delta_bad_existing_is_v1.p4k");
    build_v1(&v1_only, CompressionMethod::Store, &[("a.txt", b"x")]);
    let err = delta_convert_v1_into_v2(
        &v1_only,
        &v1_only,
        &out,
        writer_options(CompressionMethod::Store),
    )
    .unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("not P4K v2") || msg.contains("same file"),
        "expected v2-version or same-path rejection, got: {msg}"
    );

    let _ = fs::remove_file(&existing_v2);
    let _ = fs::remove_file(&bad_v1);
    let _ = fs::remove_file(&v1_only);
}

#[test]
fn delta_rejects_output_pointing_at_a_source() {
    let v2 = tempfile_path("delta_aliased_v2.p4k");
    let v1 = tempfile_path("delta_aliased_v1.p4k");

    build_v2(&v2, CompressionMethod::Store, &[("a.txt", b"x")]);
    build_v1(&v1, CompressionMethod::Store, &[("a.txt", b"x")]);

    // Output aliases the v2 input.
    let err = delta_convert_v1_into_v2(&v2, &v1, &v2, writer_options(CompressionMethod::Store))
        .unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("same file"), "expected alias rejection, got: {msg}");

    // Output aliases the v1 input.
    let err = delta_convert_v1_into_v2(&v2, &v1, &v1, writer_options(CompressionMethod::Store))
        .unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("same file"), "expected alias rejection, got: {msg}");

    let _ = fs::remove_file(&v2);
    let _ = fs::remove_file(&v1);
}

#[test]
fn delta_progress_events_describe_the_run() {
    // Exercises the progress callback wiring end-to-end. We just care that
    // a sensible sequence shows up and that the final `Finished` event
    // carries the same stats we get from the non-progress variant.
    let v2 = tempfile_path("delta_progress_v2.p4k");
    let v1 = tempfile_path("delta_progress_v1.p4k");
    let out = tempfile_path("delta_progress_out.p4k");

    let v2_entries: &[(&str, &[u8])] = &[("keep.txt", b"K"), ("change.txt", b"OLD")];
    let v1_entries: &[(&str, &[u8])] = &[("keep.txt", b"K"), ("change.txt", b"NEW"), ("new.txt", b"N")];

    build_v2(&v2, CompressionMethod::Store, v2_entries);
    build_v1(&v1, CompressionMethod::Store, v1_entries);

    let events = Mutex::new(Vec::<P4kDeltaProgress>::new());
    let stats = delta_convert_v1_into_v2_with_progress(
        &v2,
        &v1,
        &out,
        writer_options(CompressionMethod::Store),
        |event| events.lock().unwrap().push(event),
    )
    .unwrap();

    let events = events.into_inner().unwrap();
    assert!(matches!(events.first(), Some(P4kDeltaProgress::OpeningSources)));
    assert!(events
        .iter()
        .any(|e| matches!(e, P4kDeltaProgress::SourcesOpened { .. })));
    assert!(events.iter().any(|e| matches!(e, P4kDeltaProgress::Planning)));
    assert!(events
        .iter()
        .any(|e| matches!(e, P4kDeltaProgress::PlanningFinished { .. })));
    assert!(events.iter().any(|e| matches!(e, P4kDeltaProgress::Writing { .. })));
    assert!(events
        .iter()
        .any(|e| matches!(e, P4kDeltaProgress::WritingFinished)));
    assert!(matches!(events.last(), Some(P4kDeltaProgress::Finished { .. })));
    if let Some(P4kDeltaProgress::Finished { stats: final_stats }) = events.last() {
        assert_eq!(final_stats, &stats);
    } else {
        panic!("missing Finished event");
    }

    let _ = fs::remove_file(&v2);
    let _ = fs::remove_file(&v1);
    let _ = fs::remove_file(&out);
}

#[test]
fn delta_results_match_rebuilding_v2_from_new_v1() {
    // A useful invariant: the delta output, as observed through the public
    // reader, should be functionally indistinguishable from converting the
    // new v1 to v2 directly. We use Store compression so the comparison
    // can rely on byte-exact payload reads.
    let v2 = tempfile_path("delta_invariant_v2.p4k");
    let v1 = tempfile_path("delta_invariant_v1.p4k");
    let out_delta = tempfile_path("delta_invariant_delta.p4k");
    let out_fresh = tempfile_path("delta_invariant_fresh.p4k");

    let v2_entries: &[(&str, &[u8])] = &[
        ("alpha.txt", b"alpha"),
        ("bravo.txt", b"OLD-bravo"),
        ("charlie.txt", b"charlie"),
        ("removed.txt", b"goodbye"),
    ];
    let v1_entries: &[(&str, &[u8])] = &[
        ("alpha.txt", b"alpha"),
        ("bravo.txt", b"NEW-bravo!"),
        ("charlie.txt", b"charlie"),
        ("delta.txt", b"freshly added"),
    ];

    build_v2(&v2, CompressionMethod::Store, v2_entries);
    build_v1(&v1, CompressionMethod::Store, v1_entries);

    delta_convert_v1_into_v2(&v2, &v1, &out_delta, writer_options(CompressionMethod::Store)).unwrap();
    convert_v1_to_v2(&v1, &out_fresh, writer_options(CompressionMethod::Store)).unwrap();

    let mut delta_payloads = read_archive_payloads(&out_delta);
    let mut fresh_payloads = read_archive_payloads(&out_fresh);
    delta_payloads.sort_by(|a, b| a.0.cmp(&b.0));
    fresh_payloads.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(delta_payloads, fresh_payloads);

    let _ = fs::remove_file(&v2);
    let _ = fs::remove_file(&v1);
    let _ = fs::remove_file(&out_delta);
    let _ = fs::remove_file(&out_fresh);
}
