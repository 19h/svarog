# P4K Format Notes

This document records the P4K v1/v2 layout facts used by `svarog-p4k`.
The authority is the CigDataPatcher decompilation dump.

Primary functions used:

- `CCigPakFileEntry::GetLocalFileRecordSize`
- `CCigPakFileEntry::WriteLocalFileHeaderToBuffer_v1`
- `CCigPakFile::LoadPakFile_v1`
- `CCigPakFile::UpdateAndWriteCDR_v1`
- `CCigPakFile::LoadPakFile_v2`
- `CCigPakFile::UpdateAndWriteCDR_v2`
- `CCigPakFile::ConvertP4k_v1_v2`

## Decompile Source Map

| Function | Invariant |
| --- | --- |
| `CCigPakFileEntry::GetLocalFileRecordSize` | v1 local record size is aligned to the physical sector size and is zero for non-v1 archives |
| `CCigPakFileEntry::WriteLocalFileHeaderToBuffer_v1` | v1 local header signature, method/time/date/CRC fields, ZIP64 sentinel sizes, ZIP64 extra, and dummy `0x0666` field |
| `CCigPakFile::LoadPakFile_v1` | ZIP64/P4K extra-field ids and total sizes are checked; duplicate v1 filename handles are collapsed after install/freelist data is read |
| `CCigPakFile::UpdateAndWriteCDR_v1` | v1 install block, ZIP64 central directory records, ZIP64 EOCD/locator, `CI`/`CIG\0` EOCD comment, and final sector alignment |
| `CCigPakFile::LoadPakFile_v2` | v2 EOCDR is read from the aligned EOF buffer and drives CDR/name-table parsing |
| `CCigPakFile::UpdateAndWriteCDR_v2` | v2 install block, 64 KiB CDR alignment, `0xCC` CDR entries, name table, and `0xAF` EOCDR at the end of the aligned EOF buffer |
| `CCigPakFile::UpdateAndWriteCDR` | optional 64-byte manifest digest input is copied into `m_manifestSHA256` before the version-specific writer runs |
| `CCigPakFile::UpdateAndWriteCDR_v2` | `m_manifestSHA256[0..64]` is copied verbatim into the v2 EOCDR at offset `0x69` |
| `CCigPakFile::ConvertP4k_v1_v2` | converter computes and removes v1 local-record spans, records freed ranges, and rewrites archive metadata as v2 |
| `CCigPakFile::AddNewFiles` | newly added files are sorted by compressed size descending, then filename handle ascending, then placed into physical-sector-aligned archive storage spans |
| `BuildUncompressedDataFileNameList` | encrypted entries are enumerated for the uncompressed-data workflow; retail P4Ks can still contain encrypted compressed payloads, which are decoded by AES decrypt-then-decompress |
| `CreateP4kReadyFile_Impl` | filesystem-backed entries copy DOS modification time/date from the source file metadata |
| `CreateP4kReadyFile_Impl` | CIG CRC32C is computed from the original mapped input before compression/encryption |
| `CreateP4kReadyFile_Impl` | encrypted payloads are zero-padded to 16 bytes and passed through `AES128_ECB_EncryptBuffer` |
| `AES128_ECB_EncryptBuffer` | helper name says ECB, but the body XORs with the previous ciphertext block, i.e. AES-128-CBC with a zero first block |
| `CRC32_ExecuteComputeFromBuffer` | CRC32C/Castagnoli over uncompressed bytes with initial `0xffffffff` and final complement |
| `cigNormalizePathInplace` | archive paths use `/`, collapse duplicate separators, remove `./`, resolve interior `../`, optionally lowercase ASCII, and trim trailing spaces |
| `VerifyFileIntegrity` / `VerifyP4kReadyFile_Impl` | verification hashes the raw compressed/post-encryption payload with SHA-256 before decompression, then validates CIG CRC32C over uncompressed bytes |
| `RSA1024_SignMetaData` / `SHA256_ExecuteComputeFromFileMetaData` | entry signatures are RSA-1024 signatures over a SHA-256 metadata digest of CIG CRC32C, compressed size, uncompressed size, and path bytes |
| `CompressBuffer` / `UncompressBuffer` | compression method IDs and decode families: store `0`, raw deflate `8`, deprecated zstd `93`, zstd `100`, and zlib-wrapped Xbox deflate `101` |
| `cig_p4k_subarchive::*` | P4K subarchive EOCDR section size is `0x1062`; marker `0x5005` and incomplete flag are checked at fixed offsets |
| `GetSubArchiveCDRInformation` / `EnumerateSubArchiveCDREntries` | P4K subarchive CDR info and entry enumeration use ZIP64-like records with version-specific P4K extra-field layouts |

## Shared Data Model

Entries carry:

- compression method: `0` store, `8` raw deflate, `101` zlib-wrapped
  Xbox deflate, and zstd via `100` (`file_is_zstandard_compressed`) or
  `93` (`zstandard_compression`)
- DOS modification time/date
- CIG CRC32C of uncompressed data
- compressed and uncompressed sizes
- payload offset or local-header offset, depending on version
- 128-byte RSA signature
- 16-bit encryption flag: `0` plain, `1` encrypted
- 32-byte SHA-256

Paths are normalized with `cigNormalizePathInplace(..., false)` semantics:
backslashes become forward slashes, duplicate separators collapse, `./`
segments are removed, interior `../` segments are resolved, and trailing
spaces are trimmed. The decompiled loop is purely streaming: a leading
`../name` becomes `.name`, not a parent-directory traversal. The v2 name
table stores those normalized, NUL-terminated UTF-8 paths. Because the
v2 writer normalizes before writing, the reader rejects v2 name-table
entries that are invalid UTF-8 or differ from their normalized form.

Encrypted entries use the crate's P4K AES-128-CBC path after the selected
compression step. The writer compresses first, zero-pads the stored
payload to a 16-byte AES block boundary, encrypts with the fixed P4K key
and zero IV, stores the padded encrypted byte length as
`compressed_size`, and sets the `0x5002`/v2 encryption flag. Decryption
preserves the full padded block stream. Stored readers return only the
first `uncompressed_size` bytes so real trailing zero bytes in stored
payloads are not lost; compressed readers decrypt first and then
decompress to exactly `uncompressed_size`.
The official creation path special-cases zero-length inputs before
compression or encryption: empty files remain method `0` (stored),
unencrypted, with `compressed_size == uncompressed_size == 0`, CRC32C
`0`, the SHA-256 of the empty raw payload, and v2 `offset_to_file_data`
`0`, even when the caller's normal settings request compression or
encryption.
The encryption flag is treated as a boolean field; values other than
`0` or `1` are rejected.

Compressed reads use the metadata `uncompressed_size` as a hard bound.
The dump's `UncompressBuffer` loops until the destination size is filled
for raw/zlib deflate, and zstd paths decompress into buffers sized from
the same metadata before CRC verification.
Stored, unencrypted entries are valid only when `compressed_size` equals
`uncompressed_size`; encrypted stored entries may have a larger
block-aligned `compressed_size` because of AES zero padding.
`VerifyFileIntegrity` and `VerifyP4kReadyFile_Impl` compute SHA-256 over
the raw stored payload bytes and compare it with the P4K metadata before
decompression. After read/decompression, `VerifyP4kReadyFile_Impl`
computes CIG CRC32C over the uncompressed bytes and compares it with the entry
metadata. `svarog-p4k` exposes this strict path as explicit integrity
verification: `verify_payload_sha256`, `verify_entry_integrity`, and
`verify_integrity`. Normal reads and extraction do not enforce CRC32C:
retail v1 archives have been observed with CDR CRC values that do not
match raw encrypted bytes, decrypted compressed bytes, decoded CryXMLB
bytes, or decoded XML text, while their SHA-256 values still match the
raw stored payload. `p4k-verify` remains the strict SHA-plus-CRC path.

The dump names the helpers `AES128_ECB_EncryptBuffer` and
`AES128_ECB_DecryptBuffer`; their implementation performs the CBC
previous-block XOR around each ECB block.

`CompressBuffer` uses zstd level `1` for methods `93` and `100`,
raw deflate level `9` with `windowBits = -15` for method `8`, and
zlib-wrapped deflate level `9` for method `101`. Those are the
`P4kWriterOptions` defaults; callers may override the levels when exact
official compression output is not required.

Fresh archive creation sorts input paths before ready-file creation and
sorts new P4K entries before placement. The placement comparator orders
by `compressed_size` descending and then by `FileName.nHandle`
ascending. `svarog-p4k` assigns fresh filename handles from the sorted
set of lowercase normalized archive names so equal-size entries have
deterministic physical order independent of caller staging order.

The 128-byte signature field is produced by `RSA1024_SignMetaData`.
That function first computes `SHA256_ExecuteComputeFromFileMetaData`:
little-endian CIG CRC32C, little-endian compressed size, little-endian
uncompressed size, then file-name bytes. The P4K-ready creation path
sets `bLowercaseNormalize = 1`, so the file-name bytes are lowercased
and `\` is mapped to `/` before hashing. This is not the same as archive
path normalization: the signature path does not collapse duplicate
separators, remove `./`, resolve `../`, or trim trailing spaces. The
signing call uses
`rsa_sign_hash_ex(..., hash="sha256", saltlen=0x10)` against a caller
supplied private key. `svarog-p4k` exposes the metadata digest as
`signature_metadata_sha256` and preserves caller-supplied 128-byte
signatures for both precompressed entries and fresh byte/file entries
through `P4kEntryMetadata`; it does not fabricate official signatures
without a private key. The traced `VerifyP4kReadyFile_Impl` path checks
payload SHA-256, payload MD5, decompressed CRC32C, and sizes, but does
not call an RSA signature verifier.

The final P4K CDR/local metadata uses `CRC32_ExecuteComputeFromBuffer`:
CRC32C/Castagnoli over the uncompressed file bytes with initial
`0xffffffff` and final bitwise complement; empty input returns `0`. The
dump also computes a `nZipCRC32` with the deprecated ZIP/IEEE path for
an intermediate P4K-ready local header, but that value is not the CRC
stored in the final P4K CDR.

## V1 Layout

V1 is ZIP64-based and uses local file records before every payload:

```text
[local record entry 0][payload entry 0]
[local record entry 1][payload entry 1]
...
[install block: { u64 bytes_already_written_to_disk, u32 zero } per entry]
[freelist blocks: { u64 offset, u64 size } * freelist_count]
[ZIP64 central directory entries]
[ZIP64 EOCD]
[ZIP64 locator]
[EOCD + 16-byte P4K comment]
[sector padding]
```

The central-directory extra-field length is `0xCE` for normal P4K
entries. Its fields use the P4K convention that the `size` word is the
total field size, including the `id` and `size` words.
The fixed CDR fields follow the official ZIP64 writer/verifier values:
version made by `46`, version needed `45`, flags `0`, 32-bit size
sentinels `0xffffffff`, file comment length `0`, disk start `0xffff`,
zero attributes, and 32-bit local-header offset sentinel `0xffffffff`.

| Field | Total Size | Payload |
| --- | ---: | --- |
| `0x0001` ZIP64 | `0x20` | uncompressed `u64`, compressed `u64`, local-header offset `u64`, disk `u32` |
| `0x5000` | `0x84` | 128-byte signature |
| `0x5002` | `0x06` | encryption `u16` |
| `0x5003` | `0x24` | SHA-256 `[u8; 32]` |

The ZIP64 extra-field disk value is always `0` in P4K v1 CDR entries
written by the official tool.

The local file record is not just the ZIP local header span. The writer
uses:

```text
local_record_size = align_up(name_len + physical_sector_size + 0x3D, physical_sector_size)
```

The local header itself reports:

```text
extra_len = physical_sector_size - name_len - 0x1E
```

and includes a ZIP64 field followed by dummy field `0x0666`. The dump
writer places payload bytes after the full `local_record_size`; retail v1
archives also exist with payload bytes immediately after the ZIP local
header's advertised variable data. `svarog-p4k` uses the CDR SHA-256 to
detect the archive's placement convention when a v1 archive is opened.
Known ZIP-local and aligned-record archives then use that placement
directly for reads, dumps, conversion, and raw SHA verification; mixed
or inconclusive archives fall back to per-entry SHA-256 selection when
both layouts are plausible. ZIP-local placement is sampled because local
retail `Data.p4k` validation showed that placement for sampled entries.
The verification path cross-checks v1 central-directory metadata against
the local header before reading payload bytes: version needed `45`, flags
`0`, compression method, DOS time/date, CRC32C, ZIP64 size sentinels,
and file-name length/bytes must agree with the CDR. `svarog-p4k` also validates that
the local fixed header, name, and extra-field span equals the physical
sector size when the P4K CI comment is present, and that the local ZIP64
field matches the CDR sizes and disk `0`. Retail v1 archives
store either the CDR local-header offset or `0` in the local ZIP64 offset
slot, so both are accepted. The following `0x0666` dummy field must occupy
the remaining local extra space. Its id and size are validated, but its
padding bytes are not interpreted: `WriteLocalFileHeaderToBuffer_v1`
only writes the dummy field header, and retail archives can contain
non-zero bytes in the unused tail.
Fresh v1 writes place the next local record at the next physical sector
after the previous payload, matching the aligned storage span allocated
by `AddNewFiles`. They also use the same fresh-file ordering noted for
v2: compressed size descending, with filename-handle order as the
tie-break.

The v1 EOCD has the standard ZIP64 EOCD and locator plus a 16-byte EOCD
comment. The ZIP64 tail is written as a single-disk
archive: the classic EOCD uses ZIP64 sentinels for disk fields,
count/size/offset fields, and comment length `16`; the ZIP64 EOCD uses record size
`44`, versions `46/45`, disk fields `0`, matching per-disk/total entry
counts, and a CDR range ending exactly at the ZIP64 EOCD; the ZIP64
locator immediately follows it with disk fields `0/1`. `LoadPakFile_v1`
consumes the total entry count through a 32-bit load from the ZIP64 EOCD
total-count slot, so `svarog-p4k` rejects v1 ZIP64 entry counts above
`u32::MAX`. The comment
stores writer metadata. `UpdateAndWriteCDR_v1` writes comment length
`0x0010`, then `CI`, then marker `0x00010047`, so the first six comment
bytes on disk are `43 49 47 00 01 00` (`CIG\0\x01\0`):

| Offset | Type | Meaning |
| --- | --- | --- |
| `0x00` | `[u8;2]` | ASCII `CI` |
| `0x02` | `u32` | marker `0x00010047` |
| `0x06` | `u16` | physical sector size |
| `0x08` | `u64` | freelist-block count |

`LoadPakFile_v1` uses that freelist count to size the install block before
the central directory; the per-entry 12-byte progress records come first,
followed by `freelist_count * 16` bytes of freelist records. The
reserved `u32` in each progress record is written as zero and rejected
when nonzero. Persisted freelist records with `size == 0` are rejected;
freelist records whose `offset + size` overflows are also rejected;
the official merge path drops zero-sized blocks before writing.
Official v1 P4Ks are padded so the physical file length is aligned to the
sector size recorded in the P4K EOCD comment, and `svarog-p4k` rejects
unaligned archives with this comment. The padding bytes after the EOCD
comment must be zero.

After the v1 install data is loaded, `LoadPakFile_v1` sorts entries by
the dump's filename handle. That handle is created from
`cigNormalizePathInplace(..., true)`, so duplicate removal uses the
lowercased, slash-normalized path identity. Removed duplicates have their
aligned local-record-plus-payload spans recorded as freelist blocks, then
adjacent freelist blocks are merged. `svarog-p4k` performs the same
visible load behavior for v1 P4K files with the P4K EOCD comment: only the first
entry for a filename-handle path remains readable, and duplicate storage
is carried into the freelist used by conversion.
Both v1 and v2 loaders construct entries with `bEnforceAlignment = true`;
entry data offsets must therefore be multiples of the archive physical
sector size. For v1 this is the ZIP64 local-header offset from the CDR;
for v2 this is the direct `offset_to_file_data` payload pointer.

## V2 Layout

V2 removes ZIP local headers and writes payloads directly:

```text
[payload bytes]
[sector padding]
[install block: u64 bytes_already_written_to_disk per entry]
[freelist blocks: { u64 offset, u64 size } * num_freelist_blocks]
[sector padding]
[padding to 64 KiB CDR boundary]
[CDR: entry_count * 0xCC]
[name table: NUL-terminated normalized paths]
[zero padding to place EOCDR at end of the aligned EOF buffer]
[EOCDR: 0xAF bytes ending in version=2, magic="JiJi"]
```

`LoadPakFile_v2` rejects archives whose physical file length is not
aligned to the archive physical sector size. `svarog-p4k` still accepts
aligned trailing zero padding by locating the last non-zero EOCDR byte
before applying the structural EOF-buffer checks.

The v2 CDR entry is exactly `0xCC` bytes:

| Offset | Type | Meaning |
| ---: | --- | --- |
| `0x00` | `u16` | compression method |
| `0x02` | `u16` | DOS modification time |
| `0x04` | `u16` | DOS modification date |
| `0x06` | `u32` | CIG CRC32C |
| `0x0A` | `u64` | compressed size |
| `0x12` | `u64` | uncompressed size |
| `0x1A` | `u64` | absolute payload offset |
| `0x22` | `u64` | name-table offset |
| `0x2A` | `[u8; 128]` | signature |
| `0xAA` | `u16` | encryption flag |
| `0xAC` | `[u8; 32]` | SHA-256 |

The v2 EOCDR is exactly `0xAF` bytes. The last six content bytes are:

```text
u16 version = 2
u32 magic = 0x696A694A  // "JiJi"
```

Important EOCDR offsets:

| Offset | Type | Meaning |
| ---: | --- | --- |
| `0x00` | `u64` | number of file entries |
| `0x10` | `u64` | CDR start offset |
| `0x18` | `u64` | CDR byte size |
| `0x28` | `u64` | name-table absolute offset |
| `0x30` | `u64` | total name-table length |
| `0x40` | `u64` | end of payload area |
| `0x48` | `u64` | freelist block count |
| `0x60` | `u64` | physical sector size |
| `0x68` | `u8` | writer flag, observed as `1` |
| `0x69` | `[u8; 64]` | manifest digests |
| `0xA9` | `u16` | version |
| `0xAB` | `u32` | magic |

The reserved EOCDR slots at `0x08`, `0x20`, `0x38`, `0x50`, and
`0x58` are written as zero by `UpdateAndWriteCDR_v2`; `svarog-p4k`
rejects v2 archives where any of those fields are nonzero. The byte at
`0x68` is likewise treated as a fixed writer flag and must be `1`.

The v2 writer does not derive these manifest digests from the CDR. The
official wrapper accepts an optional 64-byte digest buffer, stores it in
`m_manifestSHA256`, then `UpdateAndWriteCDR_v2` copies those 64 bytes
verbatim into the EOCDR.

Before new files are placed, `AddNewFiles` sorts them by compressed size
descending, with an internal filename handle as the tie-break. Fresh v2
archives from `svarog-p4k` reproduce that tie-break by assigning handles
from the sorted set of `cigNormalizePathInplace(..., true)` path
identities.
Each fresh v2 payload starts at a physical-sector boundary; the stored
`compressed_size` remains the real payload byte length, so sector padding
between entries is not part of the file data.
Every non-empty v2 payload range must end at or before `end_of_payload`;
non-empty payload ranges must not overlap each other. The install block
begins at `end_of_payload`, followed by freelist records and zero
padding before the CDR.
V2 name-table offsets are increasing in CDR entry order. The normal
packed case is that each `offset_to_filename` equals the current byte
cursor in the name table, then the writer copies one normalized
NUL-terminated name and advances by `strlen(name) + 1`. The
decompiled `UpdateAndWriteCDR_v2` path copies the original filename
handle text, normalizes it in place with `cigNormalizePathInplace(...,
false)`, and advances by the original handle string length. If the
original text normalizes shorter, non-name slack can remain after the
first NUL before the next offset. `LoadPakFile_v2` follows each
`offset_to_filename` directly and reads up to the next NUL; it does not
require the name table to be tightly packed.

On load, `LoadPakFile_v2` sizes the install buffer as
`8 * (num_file_entries + 2 * num_freelist_blocks)`, reads it from
`end_of_payload`, uses the first `num_file_entries * 8` bytes as
`bytes_already_written_to_disk`, and copies the following
`num_freelist_blocks * 16` bytes into the freelist vector. Persisted
freelist records with `size == 0`, or whose `offset + size` overflows,
are rejected; the official merge path drops zero-sized blocks before
writing, and `svarog-p4k` rejects merged-size overflow instead of
saturating when it coalesces adjacent blocks.

## V1 to V2 Conversion

The official converter supports v1 to v2 only. It removes local records
from the payload offsets, records the removed local-record ranges in the
freelist, aligns `end_of_payload`, switches the archive version to `2`,
and writes a v2 CDR/EOCDR.

`svarog-p4k` preserves compressed payload bytes, compression method,
sizes, CRC, DOS time/date, encryption flag, signature, SHA-256, and
install-block `bytes_already_written_to_disk` values when converting. It
also follows the official in-place conversion layout: the original v1
payload region is copied through, v2 entry offsets point past each local
record, existing v1 freelist blocks are carried forward, and the removed
local-record spans are appended as freelist blocks in the v2 install
block. Entries with
`compressed_size == 0` get v2 `offset_to_file_data = 0` and do not
advance `end_of_payload`, matching the converter branch in the dump.
Adjacent freelist blocks are merged before the v2 tail is written; merged
size overflow is rejected instead of saturating. If a merged freelist
block ends exactly at `end_of_payload`, it is removed and `end_of_payload`
is moved back to that block's offset; this repeats for trailing free
blocks. The merge routine does not otherwise clip freelist ranges: a
removed v1 local record that starts at the aligned v2 `end_of_payload`
and extends beyond it remains a persisted freelist block, matching the
dump's exact trim predicate.
Because v2 loaders enforce that every payload offset is aligned to the
v2 physical sector size, `svarog-p4k` rejects conversion options whose
destination sector size is incompatible with the source v1 payload
offsets.

## P4K Subarchives

The dump also contains a separate `cig_p4k_subarchive` subsystem used
for `.p4k_subarchive.zst` payloads. This is not the main P4K v1/v2 file
container, but it is part of the P4K toolchain and is tracked separately
so the main archive work does not silently ignore it.

`GetSubArchiveEOCDRSize` returns `0x1062`, and
`GetEOCDRBeginOffset(size)` subtracts that value from the subarchive
file length. `IsValidSubArchive` checks a `0x5005` marker at EOCDR
section offset `0x38`; if the marker is absent the file is not a
subarchive, and if bit `0` at section offset `0x40` is set the
subarchive is incomplete. `svarog-p4k`'s full subarchive parser follows
that EOF convention and requires the declared CDR range to end exactly
where the EOCDR section begins.

`GetSubArchiveCDRInformation` reads:

| Offset | Type | Meaning |
| ---: | --- | --- |
| `0x20` | `u64` | number of CDR entries |
| `0x28` | `u64` | total CDR size |
| `0x30` | `u64` | CDR begin, relative to subarchive file start |
| `0x3c` | `u32` | subarchive version |
| `0xc8` | `u64` | total path bytes including NUL terminators |

`EnumerateSubArchiveCDREntries` walks ZIP64-like CDR records. Both
subarchive versions store method `93` (`zstandard_compression`) in the
fixed ZIP method field and compute each payload offset as
`record_start + name_len + zip64_extra_relative_offset + 0x3e`. Version
`1` reads CRC32C from fixed CDR offset `0x10`, signature from
`name_len + 0x52`, and SHA-256 from `name_len + 0xd6`. Version `2`
reads CRC32C from `name_len + 0x52`, SHA-256 from `name_len + 0x56`,
and signature from `name_len + 0x76`.
Because those metadata records are fully contained in the EOF section
and CDR, `svarog-p4k` exposes a seeked subarchive parser that reads only
the `0x1062`-byte EOCDR section and the declared CDR range, not the
payload span before it.

## Verification Coverage

The crate tests pin:

- v1 extra-field struct sizes and offsets
- v2 CDR and EOCDR struct sizes and offsets
- v2 detection through trailing null padding
- v2 store/zstd payload reading
- compression method numeric ID mapping and round-trip conversion
- `cigNormalizePathInplace` slash/`.`/`..`/lowercase/trailing-space behavior
- compressed payload `uncompressed_size` mismatch rejection
- explicit integrity-verifier uncompressed payload CRC32C mismatch rejection
- raw stored payload SHA-256 mismatch rejection via the integrity verifier
- sequential and parallel raw-payload SHA-256 verification before decoded CRC verification
- decoded CRC32C verification streams through AES/decompression adapters instead of retaining full decoded payload buffers
- decoded `read` paths use the same AES/decompression adapters, avoiding extra compressed/decrypted payload copies before producing the returned buffer
- v1 payload placement detection distinguishes sector-spanned ZIP-local retail archives, variable-span ZIP-local archives, and aligned-record writer archives using sampled local-header spans plus SHA-256 metadata; after the first match it hashes the expected placement first and only probes the alternate candidate on mismatch, stops after a bounded confident sample set to keep large retail opens fast, and known-offset fast paths then use either `local_header_offset + sector_size`, parsed local-header spans, or the dump local-record formula, with per-entry SHA-256 fallback only when archive-wide detection remains inconclusive
- v1 ZIP64 central-directory parsing uses the dump's fixed `0xFC + name_len` P4K record layout for direct field extraction, normalizes parsed path bytes without first allocating an intermediate lossy string, can scan record spans once and parse the fixed P4K metadata records in parallel under the `parallel` feature, and preserves CDR order plus malformed-span/custom-field checks
- v2 central-directory parsing keeps name-table order validation sequential, checks normalized name-table bytes before allocation, and moves validated name strings into final entry records; the sequential path validates each name from the current packed `0xCC` CDR record's `0x22` `offset_to_filename` field and fills final entry plus payload-range arrays directly without staging a separate name vector, while the parallel path first reads those packed name offsets and then parses fixed CDR records plus install progress values under the `parallel` feature, preserving entry order and the existing payload overlap validation without cloning or normalizing every valid name after validation
- raw-payload SHA verification and full SHA-plus-CRC integrity verification can run in physical offset order so large v1 retail archives are read sequentially instead of filename-handle order
- zero-copy dump/verify fast path for unencrypted stored payloads with the same stored-size validation as decoded reads
- streaming dump/extract writer path for stored, compressed, and encrypted entries, avoiding full decoded payload allocation before file output
- parallel dump-to-directory path under the `parallel` feature, with CLI worker-count control, one-time parent-directory creation for unique output paths, ordered sequential fallback for duplicate output paths, and explicit output flushing so late buffered write errors are reported
- CLI P4K extraction planning filters archive entries, detects DCB/SOCPAK paths, performs incremental skip checks, and builds extraction tasks in one archive pass without retaining a duplicate matched-entry name list; extension checks use ASCII case-insensitive suffix comparison without lowercasing each path
- CLI SOCPAK expansion extracts the `.socpak` P4K entry to disk and opens that file as the ZIP source, avoiding a whole-SOCPAK resident buffer; nested ZIP members then probe only the first eight bytes and stream non-CryXML files directly to disk, while `CryXmlB\0` candidates must also pass cheap 36-byte header bounds validation before the member is buffered for binary-to-XML conversion
- v2 writer byte layout, install block, and readback
- v2 writer derives `offset_to_filename` from a running name-table cursor instead of retaining a per-entry offset vector, matching the official packed-name-table order while reducing large-create/convert metadata memory
- v2 writer checked CDR/name-table/EOF tail size arithmetic, including CDR offset overflow rejection
- v1/v2 writer ordering of newly added entries by compressed size and deterministic filename-handle tie-break; fresh writer filename-handle assignment normalizes each staged path once, sorts the normalized names, and assigns handles back by original entry index to avoid a second per-entry normalization pass
- v2 writer physical-sector alignment for each newly added entry
- filesystem-backed writer DOS timestamp preservation
- filesystem-backed creator staging can produce opaque staged entries directly from files and append them to one builder, so parallel CLI creation no longer allocates one intermediate `P4kBuilder` per input file while preserving caller order before the dump-backed fresh-file placement sort
- filesystem-backed unencrypted stored entries are staged without retaining a payload buffer; v2 creation resolves CRC32C/SHA-256 in the single payload copy pass because its CDR follows payload data, while v1 creation reserves the dump-sized local-record span, resolves CRC32C/SHA-256 during the payload copy, then seeks back to fill the local header before writing the CDR
- filesystem-backed unencrypted compressed entries are compressed into temporary staged payload files with streamed CRC/SHA metadata and cleanup on builder drop
- filesystem-backed encrypted stored/compressed entries are encrypted into temporary staged payload files without retaining full payload buffers, with raw encrypted SHA validation and cleanup on builder drop; stored encrypted file creation computes plaintext CRC32C while streaming through encryption, and compressed encrypted file creation pipes compression directly into the encrypted temp payload without an intermediate compressed temp file
- temporary staged payload files are SHA-256 checked during the output copy pass, avoiding a separate full-file validation read before copying archive payload bytes
- v2 EOCDR manifest digest pass-through
- buffered file output for fresh v1/v2 creation and v1-to-v2 conversion without changing on-disk byte layout
- fresh v1/v2 archive creation writes to a sibling temporary output and publishes it with `rename` only after the full archive is written successfully, preserving existing targets on staged-payload validation failures
- v1 to v2 conversion first attempts Linux reflink cloning for the preserved source prefix, then falls back to large buffered copy with exact byte-count validation before the v2 tail is written
- v1 to v2 conversion builds the v2 tail from validated payload offsets and lengths without retaining per-entry payload slice references or a parallel payload-offset array, computes the preserved payload prefix end in a streaming max pass instead of materializing per-entry end offsets, and keeps large-archive metadata memory bounded while preserving local-header/range checks
- v1 to v2 conversion writes to a sibling temporary output and publishes it with `rename` only after the preserved payload prefix and full v2 tail are written successfully
- v1 writer local ZIP64 extra, dummy `0x0666`, local-record padding, and readback
- v1 writer local-record size calculation follows the dump formula `align_up(name_len + sector_size + 0x3D, sector_size)` with checked overflow handling
- v1 reader acceptance of retail non-zero bytes after the local dummy `0x0666` extra-field header
- v1 reader and converter SHA-256 selection for ZIP-local retail payload placement
- v1 writer exact `CIG\0\x01\0` EOCD comment prefix and fields
- v1 writer physical-sector alignment between entries
- v1 reader rejection for local-header filename length/byte mismatches
- v1 filename-handle duplicate collapse and conversion of removed spans into freelist blocks
- v1 filename-handle duplicate collapse borrows already-normalized lowercase path keys and only allocates when the key must be normalized/lowercased, preserving duplicate semantics while reducing large-archive open memory churn
- v1/v2 encrypted stored and encrypted compressed writer output and readback
- fresh v2 empty entries use zero data offsets even when written after non-empty payloads
- CLI create/verify/dump/extract/conversion readback for store, raw deflate, zlib deflate, zstd, deprecated zstd, and encrypted zstd payloads
- CLI JSON metadata dump for archive layout, freelists, v1 payload-placement convention, method, offset kind, known payload offset, CRC/SHA/signature, encryption, install progress fields, and converted v2 freelist metadata
- precompressed writer rejection for invalid stored payload sizes
- P4K AES-128-CBC fixed-vector encryption/decryption
- RSA signature metadata digest field order and path normalization
- seeked `.p4k_subarchive.zst` metadata parsing from a file/reader without loading payload bytes, plus an opt-in real-data test via `SVAROG_P4K_SUBARCHIVE_TEST_FILE`
- fresh byte/file writer preservation of caller-supplied DOS timestamp and 128-byte signature metadata
- v1 to v2 conversion freelist records for removed local headers
- v1 to v2 conversion preserving encrypted compressed payload bytes
- v1 to v2 conversion handling for empty entries and merged freelist spans
- v1 to v2 conversion rejection for merged freelist span overflow
- v1 to v2 conversion with and without P4K custom metadata
- v1 to v2 conversion preserving partial install progress
- v1 to v2 conversion preserving existing v1 freelist blocks
- v1 to v2 conversion trimming freelist blocks at the payload tail
- v1 to v2 conversion preserving mixed empty-entry local-record freelist blocks after payload end
- v1 to v2 conversion rejection for incompatible destination sector size
- v1 to v2 conversion rejection when source and destination paths refer to the same file
- malformed v1 custom-field size rejection
- malformed v1 CDR fixed-field rejection
- malformed v1 CDR trailing-byte rejection when the EOCD-declared CDR span is not exactly consumed
- malformed v1 CDR/local ZIP64 extra-field rejection
- malformed v1 local dummy extra-field rejection
- malformed v1/v2 non-boolean encryption-flag rejection
- malformed v1 ZIP64 EOCD and locator fixed-field rejection
- malformed v1 ZIP64 entry-count rejection above the official loader's 32-bit count path
- malformed v1 local-header metadata mismatch rejection
- malformed v1 local-header DOS timestamp mismatch rejection
- malformed v1 local-header physical-sector span mismatch rejection
- malformed v1 CI file-size alignment rejection
- malformed v1 CI sector-padding rejection when post-comment padding bytes are nonzero
- malformed v2 file-size alignment rejection
- malformed v1/v2 entry data-offset alignment rejection
- malformed v2 payload range rejection when a CDR entry overlaps the install block
- malformed v2 payload range rejection when non-empty CDR entry payloads overlap each other
- malformed v2 oversized CDR/name-table metadata rejection before slicing archive bytes
- malformed v2 overlapping name-table offset rejection
- malformed v2 non-normalized name-table entry rejection
- malformed stored payload size rejection even when `uncompressed_size == 0`
- malformed precompressed writer payload rejection when supplied SHA-256 metadata does not match the raw payload bytes
- malformed precompressed writer rejection when supplied CIG CRC32C metadata does not match the decoded uncompressed bytes, including encrypted stored payloads after decrypt-and-truncate and compressed payloads after decrypt-and-decompress
- precompressed writer CRC32C validation streams decoded bytes through AES/decompression adapters instead of retaining full decoded payload buffers
- P4K subarchive EOCDR size, start offset, marker/incomplete status, full-file CDR range/count validation, CDR information offsets, and v1/v2 CDR entry enumeration offsets
- malformed v1/v2 install progress rejection
- malformed v1 install reserved-word rejection
- malformed v1/v2 zero-sized freelist block rejection
- malformed v1/v2 overflowing freelist block rejection
- malformed v2 install span rejection when freelist records do not fit
- malformed v2 CDR alignment and install-padding rejection
- malformed v2 EOCDR physical-sector-size rejection
- malformed v2 EOCDR reserved-field and writer-flag rejection
- malformed v2 EOF-buffer placement rejection

Real P4K corpus validation is opt-in because retail archives are large
and not stored in the repository:

```bash
SVAROG_P4K_TEST_FILE=/path/to/Data.p4k cargo test -p svarog-p4k real_world_p4k_parses -- --nocapture
SVAROG_P4K_TEST_DIR=/path/to/corpus cargo test -p svarog-p4k real_world_p4k_corpus_parses -- --nocapture
SVAROG_P4K_TEST_SHA256=1 SVAROG_P4K_TEST_FILE=/path/to/Data.p4k cargo test -p svarog-p4k --features parallel real_world_p4k_parses -- --nocapture
SVAROG_P4K_TEST_VERIFY=1 SVAROG_P4K_TEST_FILE=/path/to/Data.p4k cargo test -p svarog-p4k real_world_p4k_parses -- --nocapture
```

Without `SVAROG_P4K_TEST_VERIFY` or `SVAROG_P4K_TEST_SHA256`, the tests
open each archive, dispatch v1/v2 parsing, validate structural metadata,
and require at least one entry. With `SVAROG_P4K_TEST_SHA256`, every
entry is checked against its stored raw-payload SHA-256. With
`SVAROG_P4K_TEST_VERIFY`, every entry is also decoded and checked
against its CRC32C metadata.
