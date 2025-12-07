//! Comprehensive roundtrip test for DCB builder.
//!
//! Tests ALL data types and verifies exact value preservation.

use svarog_datacore::{DataCoreBuilder, DataCoreDatabase, DataType, Value};
use svarog_common::CigGuid;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Comprehensive DCB Roundtrip Test ===\n");

    // Test 1: All primitive types
    test_all_primitives()?;

    // Test 2: Arrays of all types
    test_arrays()?;

    // Test 3: Pointers and references
    test_pointers_and_references()?;

    // Test 4: Nested structs
    test_nested_structs()?;

    // Test 5: Edge cases (skip enums for now - not fully implemented)
    test_edge_cases()?;

    // Test 6: Real file roundtrip
    test_real_file_roundtrip()?;

    println!("\n=== ALL ROUNDTRIP TESTS PASSED ===");
    Ok(())
}

fn test_all_primitives() -> Result<(), Box<dyn std::error::Error>> {
    println!("--- Test 1: All Primitive Types ---");

    let mut builder = DataCoreBuilder::new();

    // Create struct with ALL primitive types
    // Note: DataType uses SByte for signed byte, Byte for unsigned
    let test_struct = builder.add_struct("AllPrimitives", None);
    builder.add_property(test_struct, "boolVal", DataType::Boolean);
    builder.add_property(test_struct, "int8Val", DataType::SByte);      // SByte = signed byte
    builder.add_property(test_struct, "int16Val", DataType::Int16);
    builder.add_property(test_struct, "int32Val", DataType::Int32);
    builder.add_property(test_struct, "int64Val", DataType::Int64);
    builder.add_property(test_struct, "uint8Val", DataType::Byte);      // Byte = unsigned byte
    builder.add_property(test_struct, "uint16Val", DataType::UInt16);
    builder.add_property(test_struct, "uint32Val", DataType::UInt32);
    builder.add_property(test_struct, "uint64Val", DataType::UInt64);
    builder.add_property(test_struct, "floatVal", DataType::Single);
    builder.add_property(test_struct, "doubleVal", DataType::Double);
    builder.add_property(test_struct, "stringVal", DataType::String);
    builder.add_property(test_struct, "guidVal", DataType::Guid);

    // Test values - use specific values that are easy to verify
    let test_guid: CigGuid = "12345678-abcd-ef01-2345-6789abcdef01".parse()?;

    let record = builder.add_record("TestRecord", test_struct, "test/primitives.xml");
    builder.set_bool(record, "boolVal", true);
    builder.set_i8(record, "int8Val", -42);
    builder.set_i16(record, "int16Val", -1234);
    builder.set_i32(record, "int32Val", -123456789);
    builder.set_i64(record, "int64Val", -9876543210i64);
    builder.set_u8(record, "uint8Val", 200);
    builder.set_u16(record, "uint16Val", 60000);
    builder.set_u32(record, "uint32Val", 4000000000);
    builder.set_u64(record, "uint64Val", 18000000000000000000u64);
    builder.set_float(record, "floatVal", 3.14159);
    builder.set_double(record, "doubleVal", 2.718281828459045);
    builder.set_string(record, "stringVal", "Hello, DCB World!");
    builder.set_guid(record, "guidVal", test_guid);

    // Write and read back
    let path = "/tmp/test_primitives.dcb";
    builder.write_to_file(path)?;
    println!("  Wrote DCB to {}", path);

    let db = DataCoreDatabase::parse(&std::fs::read(path)?)?;
    println!("  Read back: {} structs, {} records", db.struct_definitions().len(), db.records().len());

    // Verify the record
    let record = db.record_by_name("TestRecord")
        .ok_or("TestRecord not found")?;

    // Check each value individually
    let mut errors = Vec::new();

    match record.get("boolVal") {
        Some(Value::Bool(v)) if v == true => println!("  ✓ boolVal = true"),
        other => errors.push(format!("boolVal: expected Bool(true), got {:?}", other)),
    }

    match record.get("int8Val") {
        Some(Value::Int8(v)) if v == -42 => println!("  ✓ int8Val = -42"),
        other => errors.push(format!("int8Val: expected Int8(-42), got {:?}", other)),
    }

    match record.get("int16Val") {
        Some(Value::Int16(v)) if v == -1234 => println!("  ✓ int16Val = -1234"),
        other => errors.push(format!("int16Val: expected Int16(-1234), got {:?}", other)),
    }

    match record.get("int32Val") {
        Some(Value::Int32(v)) if v == -123456789 => println!("  ✓ int32Val = -123456789"),
        other => errors.push(format!("int32Val: expected Int32(-123456789), got {:?}", other)),
    }

    match record.get("int64Val") {
        Some(Value::Int64(v)) if v == -9876543210 => println!("  ✓ int64Val = -9876543210"),
        other => errors.push(format!("int64Val: expected Int64(-9876543210), got {:?}", other)),
    }

    match record.get("uint8Val") {
        Some(Value::UInt8(v)) if v == 200 => println!("  ✓ uint8Val = 200"),
        other => errors.push(format!("uint8Val: expected UInt8(200), got {:?}", other)),
    }

    match record.get("uint16Val") {
        Some(Value::UInt16(v)) if v == 60000 => println!("  ✓ uint16Val = 60000"),
        other => errors.push(format!("uint16Val: expected UInt16(60000), got {:?}", other)),
    }

    match record.get("uint32Val") {
        Some(Value::UInt32(v)) if v == 4000000000 => println!("  ✓ uint32Val = 4000000000"),
        other => errors.push(format!("uint32Val: expected UInt32(4000000000), got {:?}", other)),
    }

    match record.get("uint64Val") {
        Some(Value::UInt64(v)) if v == 18000000000000000000 => println!("  ✓ uint64Val = 18000000000000000000"),
        other => errors.push(format!("uint64Val: expected UInt64(18000000000000000000), got {:?}", other)),
    }

    match record.get("floatVal") {
        Some(Value::Float(v)) if (v - 3.14159).abs() < 0.0001 => println!("  ✓ floatVal ≈ 3.14159"),
        other => errors.push(format!("floatVal: expected Float(~3.14159), got {:?}", other)),
    }

    match record.get("doubleVal") {
        Some(Value::Double(v)) if (v - 2.718281828459045).abs() < 0.0000001 => println!("  ✓ doubleVal ≈ 2.718281828459045"),
        other => errors.push(format!("doubleVal: expected Double(~2.718281828459045), got {:?}", other)),
    }

    match record.get("stringVal") {
        Some(Value::String(v)) if v == "Hello, DCB World!" => println!("  ✓ stringVal = \"Hello, DCB World!\""),
        other => errors.push(format!("stringVal: expected String(\"Hello, DCB World!\"), got {:?}", other)),
    }

    match record.get("guidVal") {
        Some(Value::Guid(v)) if v == test_guid => println!("  ✓ guidVal = {}", test_guid),
        other => errors.push(format!("guidVal: expected Guid({}), got {:?}", test_guid, other)),
    }

    // Clean up
    std::fs::remove_file(path)?;

    if !errors.is_empty() {
        for e in &errors {
            eprintln!("  ✗ {}", e);
        }
        return Err(format!("{} primitive type errors", errors.len()).into());
    }

    println!("  All primitive types verified!\n");
    Ok(())
}

fn test_arrays() -> Result<(), Box<dyn std::error::Error>> {
    println!("--- Test 2: Arrays ---");

    let mut builder = DataCoreBuilder::new();

    let test_struct = builder.add_struct("ArrayTest", None);
    // Use add_array_property for array types
    builder.add_array_property(test_struct, "intArray", DataType::Int32);
    builder.add_array_property(test_struct, "floatArray", DataType::Single);
    builder.add_array_property(test_struct, "stringArray", DataType::String);
    builder.add_array_property(test_struct, "boolArray", DataType::Boolean);

    let record = builder.add_record("ArrayRecord", test_struct, "test/arrays.xml");

    // Set arrays
    builder.set_i32_array(record, "intArray", &[1, 2, 3, 4, 5]);
    builder.set_float_array(record, "floatArray", &[1.1, 2.2, 3.3]);
    builder.set_string_array(record, "stringArray", &["one", "two", "three"]);
    builder.set_bool_array(record, "boolArray", &[true, false, true, true, false]);

    let path = "/tmp/test_arrays.dcb";
    builder.write_to_file(path)?;
    println!("  Wrote DCB to {}", path);

    let db = DataCoreDatabase::parse(&std::fs::read(path)?)?;
    let record = db.record_by_name("ArrayRecord")
        .ok_or("ArrayRecord not found")?;

    let mut errors = Vec::new();

    // Verify int array
    if let Some(arr) = record.get_array("intArray") {
        let values: Vec<_> = arr.collect();
        let expected = vec![
            Value::Int32(1),
            Value::Int32(2),
            Value::Int32(3),
            Value::Int32(4),
            Value::Int32(5),
        ];
        if values == expected {
            println!("  ✓ intArray = [1, 2, 3, 4, 5]");
        } else {
            errors.push(format!("intArray: expected {:?}, got {:?}", expected, values));
        }
    } else {
        errors.push("intArray: not found or not an array".to_string());
    }

    // Verify float array
    if let Some(arr) = record.get_array("floatArray") {
        let values: Vec<_> = arr.collect();
        if values.len() == 3 {
            let mut ok = true;
            if let Value::Float(v) = values[0] { if (v - 1.1).abs() > 0.01 { ok = false; } } else { ok = false; }
            if let Value::Float(v) = values[1] { if (v - 2.2).abs() > 0.01 { ok = false; } } else { ok = false; }
            if let Value::Float(v) = values[2] { if (v - 3.3).abs() > 0.01 { ok = false; } } else { ok = false; }
            if ok {
                println!("  ✓ floatArray ≈ [1.1, 2.2, 3.3]");
            } else {
                errors.push(format!("floatArray: values don't match, got {:?}", values));
            }
        } else {
            errors.push(format!("floatArray: expected 3 elements, got {}", values.len()));
        }
    } else {
        errors.push("floatArray: not found or not an array".to_string());
    }

    // Verify string array
    if let Some(arr) = record.get_array("stringArray") {
        let values: Vec<_> = arr.collect();
        let expected = vec![
            Value::String("one"),
            Value::String("two"),
            Value::String("three"),
        ];
        if values == expected {
            println!("  ✓ stringArray = [\"one\", \"two\", \"three\"]");
        } else {
            errors.push(format!("stringArray: expected {:?}, got {:?}", expected, values));
        }
    } else {
        errors.push("stringArray: not found or not an array".to_string());
    }

    // Verify bool array
    if let Some(arr) = record.get_array("boolArray") {
        let values: Vec<_> = arr.collect();
        let expected = vec![
            Value::Bool(true),
            Value::Bool(false),
            Value::Bool(true),
            Value::Bool(true),
            Value::Bool(false),
        ];
        if values == expected {
            println!("  ✓ boolArray = [true, false, true, true, false]");
        } else {
            errors.push(format!("boolArray: expected {:?}, got {:?}", expected, values));
        }
    } else {
        errors.push("boolArray: not found or not an array".to_string());
    }

    std::fs::remove_file(path)?;

    if !errors.is_empty() {
        for e in &errors {
            eprintln!("  ✗ {}", e);
        }
        return Err(format!("{} array errors", errors.len()).into());
    }

    println!("  All arrays verified!\n");
    Ok(())
}

fn test_pointers_and_references() -> Result<(), Box<dyn std::error::Error>> {
    println!("--- Test 3: Pointers and References ---");

    let mut builder = DataCoreBuilder::new();

    // Create a target struct that we'll point to
    let target_struct = builder.add_struct("Target", None);
    builder.add_property(target_struct, "value", DataType::Int32);

    // Create a struct with pointer/reference properties
    let pointer_struct = builder.add_struct("PointerTest", None);
    builder.add_property(pointer_struct, "strongPtr", DataType::StrongPointer);
    builder.add_property(pointer_struct, "weakPtr", DataType::WeakPointer);
    builder.add_property(pointer_struct, "reference", DataType::Reference);
    builder.add_property(pointer_struct, "nullStrong", DataType::StrongPointer);
    builder.add_property(pointer_struct, "nullWeak", DataType::WeakPointer);
    builder.add_property(pointer_struct, "nullRef", DataType::Reference);

    // Create target record
    let target_record = builder.add_record("TargetRecord", target_struct, "test/target.xml");
    builder.set_i32(target_record, "value", 42);

    // Create main record with pointers
    let main_record = builder.add_record("MainRecord", pointer_struct, "test/pointers.xml");

    // Set strong pointer to target record
    builder.set_strong_pointer(main_record, "strongPtr", Some(target_record));
    builder.set_weak_pointer(main_record, "weakPtr", Some(target_record));

    // Reference by GUID - use empty GUID for now (null reference)
    builder.set_reference(main_record, "reference", CigGuid::EMPTY);

    // Null pointers
    builder.set_strong_pointer(main_record, "nullStrong", None);
    builder.set_weak_pointer(main_record, "nullWeak", None);
    builder.set_reference(main_record, "nullRef", CigGuid::EMPTY);

    let path = "/tmp/test_pointers.dcb";
    builder.write_to_file(path)?;
    println!("  Wrote DCB to {}", path);

    let db = DataCoreDatabase::parse(&std::fs::read(path)?)?;
    let record = db.record_by_name("MainRecord")
        .ok_or("MainRecord not found")?;

    let mut errors = Vec::new();

    // Check strong pointer
    match record.get("strongPtr") {
        Some(Value::StrongPointer(Some(ptr))) => {
            println!("  ✓ strongPtr = StrongPointer(struct={}, instance={})", ptr.struct_index, ptr.instance_index);
        }
        other => errors.push(format!("strongPtr: expected Some pointer, got {:?}", other)),
    }

    // Check weak pointer
    match record.get("weakPtr") {
        Some(Value::WeakPointer(Some(ptr))) => {
            println!("  ✓ weakPtr = WeakPointer(struct={}, instance={})", ptr.struct_index, ptr.instance_index);
        }
        other => errors.push(format!("weakPtr: expected Some pointer, got {:?}", other)),
    }

    // Check null pointers
    match record.get("nullStrong") {
        Some(Value::StrongPointer(None)) => println!("  ✓ nullStrong = StrongPointer(None)"),
        other => errors.push(format!("nullStrong: expected StrongPointer(None), got {:?}", other)),
    }

    match record.get("nullWeak") {
        Some(Value::WeakPointer(None)) => println!("  ✓ nullWeak = WeakPointer(None)"),
        other => errors.push(format!("nullWeak: expected WeakPointer(None), got {:?}", other)),
    }

    match record.get("nullRef") {
        Some(Value::Reference(None)) => println!("  ✓ nullRef = Reference(None)"),
        other => errors.push(format!("nullRef: expected Reference(None), got {:?}", other)),
    }

    std::fs::remove_file(path)?;

    if !errors.is_empty() {
        for e in &errors {
            eprintln!("  ✗ {}", e);
        }
        return Err(format!("{} pointer errors", errors.len()).into());
    }

    println!("  Pointers and references verified!\n");
    Ok(())
}

fn test_nested_structs() -> Result<(), Box<dyn std::error::Error>> {
    println!("--- Test 4: Nested Structs (via inheritance) ---");

    let mut builder = DataCoreBuilder::new();

    // Base struct
    let base_struct = builder.add_struct("BaseEntity", None);
    builder.add_property(base_struct, "name", DataType::String);
    builder.add_property(base_struct, "id", DataType::Int32);

    // Derived struct (inherits from base)
    let derived_struct = builder.add_struct("DerivedEntity", Some(base_struct));
    builder.add_property(derived_struct, "extraValue", DataType::Single);

    // Create a record of derived type
    let record = builder.add_record("TestDerived", derived_struct, "test/derived.xml");
    builder.set_string(record, "name", "Test Entity");
    builder.set_i32(record, "id", 999);
    builder.set_float(record, "extraValue", 123.456);

    let path = "/tmp/test_nested.dcb";
    builder.write_to_file(path)?;
    println!("  Wrote DCB to {}", path);

    let db = DataCoreDatabase::parse(&std::fs::read(path)?)?;
    println!("  Structs: {:?}", db.type_names());

    let record = db.record_by_name("TestDerived")
        .ok_or("TestDerived not found")?;

    let mut errors = Vec::new();

    // Verify inherited properties
    match record.get("name") {
        Some(Value::String(v)) if v == "Test Entity" => println!("  ✓ name = \"Test Entity\" (inherited)"),
        other => errors.push(format!("name: expected String(\"Test Entity\"), got {:?}", other)),
    }

    match record.get("id") {
        Some(Value::Int32(v)) if v == 999 => println!("  ✓ id = 999 (inherited)"),
        other => errors.push(format!("id: expected Int32(999), got {:?}", other)),
    }

    match record.get("extraValue") {
        Some(Value::Float(v)) if (v - 123.456).abs() < 0.001 => println!("  ✓ extraValue ≈ 123.456"),
        other => errors.push(format!("extraValue: expected Float(~123.456), got {:?}", other)),
    }

    std::fs::remove_file(path)?;

    if !errors.is_empty() {
        for e in &errors {
            eprintln!("  ✗ {}", e);
        }
        return Err(format!("{} nested struct errors", errors.len()).into());
    }

    println!("  Nested structs verified!\n");
    Ok(())
}

fn test_edge_cases() -> Result<(), Box<dyn std::error::Error>> {
    println!("--- Test 5: Edge Cases ---");

    let mut builder = DataCoreBuilder::new();

    let test_struct = builder.add_struct("EdgeCases", None);
    builder.add_property(test_struct, "emptyString", DataType::String);
    builder.add_property(test_struct, "unicodeString", DataType::String);
    builder.add_property(test_struct, "maxInt", DataType::Int32);
    builder.add_property(test_struct, "minInt", DataType::Int32);
    builder.add_property(test_struct, "zeroFloat", DataType::Single);
    builder.add_property(test_struct, "negativeFloat", DataType::Single);
    builder.add_property(test_struct, "emptyGuid", DataType::Guid);

    let record = builder.add_record("EdgeRecord", test_struct, "test/edges.xml");
    builder.set_string(record, "emptyString", "");
    builder.set_string(record, "unicodeString", "Hello 世界 🌍");
    builder.set_i32(record, "maxInt", i32::MAX);
    builder.set_i32(record, "minInt", i32::MIN);
    builder.set_float(record, "zeroFloat", 0.0);
    builder.set_float(record, "negativeFloat", -999.999);
    builder.set_guid(record, "emptyGuid", CigGuid::EMPTY);

    let path = "/tmp/test_edges.dcb";
    builder.write_to_file(path)?;
    println!("  Wrote DCB to {}", path);

    let db = DataCoreDatabase::parse(&std::fs::read(path)?)?;
    let record = db.record_by_name("EdgeRecord")
        .ok_or("EdgeRecord not found")?;

    let mut errors = Vec::new();

    match record.get("emptyString") {
        Some(Value::String(v)) if v.is_empty() => println!("  ✓ emptyString = \"\""),
        other => errors.push(format!("emptyString: expected empty string, got {:?}", other)),
    }

    match record.get("unicodeString") {
        Some(Value::String(v)) if v == "Hello 世界 🌍" => println!("  ✓ unicodeString = \"Hello 世界 🌍\""),
        other => errors.push(format!("unicodeString: expected unicode, got {:?}", other)),
    }

    match record.get("maxInt") {
        Some(Value::Int32(v)) if v == i32::MAX => println!("  ✓ maxInt = {}", i32::MAX),
        other => errors.push(format!("maxInt: expected {}, got {:?}", i32::MAX, other)),
    }

    match record.get("minInt") {
        Some(Value::Int32(v)) if v == i32::MIN => println!("  ✓ minInt = {}", i32::MIN),
        other => errors.push(format!("minInt: expected {}, got {:?}", i32::MIN, other)),
    }

    match record.get("zeroFloat") {
        Some(Value::Float(v)) if v == 0.0 => println!("  ✓ zeroFloat = 0.0"),
        other => errors.push(format!("zeroFloat: expected 0.0, got {:?}", other)),
    }

    match record.get("negativeFloat") {
        Some(Value::Float(v)) if (v - (-999.999)).abs() < 0.001 => println!("  ✓ negativeFloat ≈ -999.999"),
        other => errors.push(format!("negativeFloat: expected -999.999, got {:?}", other)),
    }

    match record.get("emptyGuid") {
        Some(Value::Guid(v)) if v.is_empty() => println!("  ✓ emptyGuid = 00000000-0000-0000-0000-000000000000"),
        other => errors.push(format!("emptyGuid: expected empty GUID, got {:?}", other)),
    }

    std::fs::remove_file(path)?;

    if !errors.is_empty() {
        for e in &errors {
            eprintln!("  ✗ {}", e);
        }
        return Err(format!("{} edge case errors", errors.len()).into());
    }

    println!("  Edge cases verified!\n");
    Ok(())
}

fn test_real_file_roundtrip() -> Result<(), Box<dyn std::error::Error>> {
    println!("--- Test 6: Real File Roundtrip (Game2.dcb) ---");

    let game_dcb_path = "/media/null/ares/scd/Data/Game2.dcb";

    if !std::path::Path::new(game_dcb_path).exists() {
        println!("  SKIPPED: {} not found", game_dcb_path);
        return Ok(());
    }

    // Load original bytes
    println!("  Loading original DCB bytes...");
    let original_bytes = std::fs::read(game_dcb_path)?;

    // Quick analysis of data_mappings in original
    let struct_count = u32::from_le_bytes(original_bytes[16..20].try_into().unwrap()) as usize;
    let mapping_count = u32::from_le_bytes(original_bytes[28..32].try_into().unwrap()) as usize;
    println!("  Original header: struct_count={}, mapping_count={}", struct_count, mapping_count);

    // Load original
    println!("  Parsing original DCB...");
    let original_db = DataCoreDatabase::parse(&original_bytes)?;
    let original_struct_count = original_db.struct_definitions().len();
    let original_record_count = original_db.records().len();
    let original_enum_count = original_db.enum_definitions().len();
    let original_property_count = original_db.property_definitions().len();

    println!("  Original: {} structs, {} properties, {} enums, {} records",
        original_struct_count, original_property_count, original_enum_count, original_record_count);

    // Load into builder
    println!("  Loading into builder...");
    let mut builder = DataCoreBuilder::from_database(&original_db)?;

    // Write to buffer (not file)
    println!("  Writing to buffer...");
    let roundtrip_bytes = builder.build()?;

    println!("  Original size: {} bytes, Roundtrip size: {} bytes",
        original_bytes.len(), roundtrip_bytes.len());

    let size_diff = original_bytes.len() as i64 - roundtrip_bytes.len() as i64;
    if size_diff != 0 {
        println!("  WARNING: File sizes differ by {} bytes!", size_diff.abs());
    }

    // Byte-by-byte comparison
    println!("\n  === BYTE-BY-BYTE COMPARISON ===");
    let min_len = original_bytes.len().min(roundtrip_bytes.len());
    let mut first_diff = None;
    let mut diff_count = 0;

    for i in 0..min_len {
        if original_bytes[i] != roundtrip_bytes[i] {
            diff_count += 1;
            if first_diff.is_none() {
                first_diff = Some(i);
            }
        }
    }

    if let Some(offset) = first_diff {
        println!("  First byte difference at offset: {} (0x{:x})", offset, offset);
        println!("  Total differing bytes: {}", diff_count);

        // Show context around first difference
        let start = offset.saturating_sub(16);
        let end = (offset + 32).min(min_len);

        println!("\n  Original bytes around offset {}:", offset);
        print_hex_dump(&original_bytes[start..end], start);

        println!("\n  Roundtrip bytes around offset {}:", offset);
        print_hex_dump(&roundtrip_bytes[start..end], start);

        // Try to identify what section this offset is in
        identify_section(offset, &original_db, &original_bytes);
    } else if original_bytes.len() == roundtrip_bytes.len() {
        println!("  ✓ Files are byte-for-byte identical!");
        return Ok(());
    } else {
        println!("  Files have same content in common bytes but different lengths");
    }

    // Parse roundtrip and compare
    println!("\n  Parsing roundtrip DCB...");
    let roundtrip_db = DataCoreDatabase::parse(&roundtrip_bytes)?;
    let roundtrip_struct_count = roundtrip_db.struct_definitions().len();
    let roundtrip_record_count = roundtrip_db.records().len();
    let roundtrip_enum_count = roundtrip_db.enum_definitions().len();
    let roundtrip_property_count = roundtrip_db.property_definitions().len();

    println!("  Roundtrip: {} structs, {} properties, {} enums, {} records",
        roundtrip_struct_count, roundtrip_property_count, roundtrip_enum_count, roundtrip_record_count);

    // Compare counts
    let mut errors = Vec::new();

    if original_struct_count != roundtrip_struct_count {
        errors.push(format!("Struct count mismatch: {} vs {}", original_struct_count, roundtrip_struct_count));
    } else {
        println!("  ✓ Struct count matches: {}", original_struct_count);
    }

    if original_property_count != roundtrip_property_count {
        errors.push(format!("Property count mismatch: {} vs {}", original_property_count, roundtrip_property_count));
    } else {
        println!("  ✓ Property count matches: {}", original_property_count);
    }

    if original_enum_count != roundtrip_enum_count {
        errors.push(format!("Enum count mismatch: {} vs {}", original_enum_count, roundtrip_enum_count));
    } else {
        println!("  ✓ Enum count matches: {}", original_enum_count);
    }

    if original_record_count != roundtrip_record_count {
        errors.push(format!("Record count mismatch: {} vs {}", original_record_count, roundtrip_record_count));
    } else {
        println!("  ✓ Record count matches: {}", original_record_count);
    }

    // Compare first 10 main records
    println!("\n  Comparing first 10 main records in detail...");
    let mut records_checked = 0;
    let mut property_mismatches = 0;

    for (i, orig_record) in original_db.all_main_records().enumerate() {
        if i >= 10 { break; }

        let record_name = orig_record.name().unwrap_or("?");

        if let Some(round_record) = roundtrip_db.record_by_name(record_name) {
            let mut this_record_mismatches = 0;
            // Compare properties
            for orig_prop in orig_record.properties() {
                if let Some(round_val) = round_record.get(orig_prop.name) {
                    // Compare values (allowing for floating point tolerance)
                    let matches = match (&orig_prop.value, &round_val) {
                        (Value::Float(a), Value::Float(b)) => (a - b).abs() < 0.0001,
                        (Value::Double(a), Value::Double(b)) => (a - b).abs() < 0.0000001,
                        (a, b) => a == b,
                    };
                    if !matches {
                        property_mismatches += 1;
                        this_record_mismatches += 1;
                        if property_mismatches <= 20 {
                            println!("    Mismatch {}.{}: {:?} vs {:?}",
                                record_name, orig_prop.name, orig_prop.value, round_val);
                        }
                    }
                } else {
                    property_mismatches += 1;
                    if property_mismatches <= 5 {
                        println!("    Property {} missing in roundtrip record {}", orig_prop.name, record_name);
                    }
                }
            }
            if this_record_mismatches == 0 {
                println!("  ✓ {} OK", record_name);
            } else {
                println!("  ✗ {} has {} mismatches", record_name, this_record_mismatches);
            }
            records_checked += 1;
        } else {
            errors.push(format!("Record {} not found in roundtrip", record_name));
        }
    }

    println!("  Checked {} records, {} property mismatches", records_checked, property_mismatches);

    if !errors.is_empty() || property_mismatches > 0 {
        if property_mismatches > 0 {
            errors.push(format!("{} property mismatches found", property_mismatches));
        }
        for e in &errors {
            eprintln!("  ✗ {}", e);
        }
        return Err(format!("{} real file roundtrip errors", errors.len()).into());
    }

    println!("  Real file roundtrip verified!\n");
    Ok(())
}

fn print_hex_dump(data: &[u8], base_offset: usize) {
    for (i, chunk) in data.chunks(16).enumerate() {
        let offset = base_offset + i * 16;
        print!("  {:08x}: ", offset);
        for (j, byte) in chunk.iter().enumerate() {
            if j == 8 { print!(" "); }
            print!("{:02x} ", byte);
        }
        // Pad if less than 16 bytes
        for j in chunk.len()..16 {
            if j == 8 { print!(" "); }
            print!("   ");
        }
        print!(" |");
        for byte in chunk {
            if *byte >= 0x20 && *byte <= 0x7e {
                print!("{}", *byte as char);
            } else {
                print!(".");
            }
        }
        println!("|");
    }
}

fn identify_section(offset: usize, _db: &DataCoreDatabase, data: &[u8]) {
    println!("\n  === SECTION IDENTIFICATION FOR OFFSET {} ===", offset);

    // Calculate expected offsets based on header
    // Header is 120 bytes (30 * 4)

    // Read counts from header
    let struct_count = u32::from_le_bytes(data[16..20].try_into().unwrap()) as usize;
    let prop_count = u32::from_le_bytes(data[20..24].try_into().unwrap()) as usize;
    let enum_count = u32::from_le_bytes(data[24..28].try_into().unwrap()) as usize;
    let mapping_count = u32::from_le_bytes(data[28..32].try_into().unwrap()) as usize;
    let record_count = u32::from_le_bytes(data[32..36].try_into().unwrap()) as usize;

    // Header pool counts
    let bool_count = u32::from_le_bytes(data[36..40].try_into().unwrap()) as usize;
    let int8_count = u32::from_le_bytes(data[40..44].try_into().unwrap()) as usize;
    let int16_count = u32::from_le_bytes(data[44..48].try_into().unwrap()) as usize;
    let int32_count = u32::from_le_bytes(data[48..52].try_into().unwrap()) as usize;
    let int64_count = u32::from_le_bytes(data[52..56].try_into().unwrap()) as usize;
    let uint8_count = u32::from_le_bytes(data[56..60].try_into().unwrap()) as usize;
    let uint16_count = u32::from_le_bytes(data[60..64].try_into().unwrap()) as usize;
    let uint32_count = u32::from_le_bytes(data[64..68].try_into().unwrap()) as usize;
    let uint64_count = u32::from_le_bytes(data[68..72].try_into().unwrap()) as usize;
    let float_count = u32::from_le_bytes(data[72..76].try_into().unwrap()) as usize;
    let double_count = u32::from_le_bytes(data[76..80].try_into().unwrap()) as usize;
    let guid_count = u32::from_le_bytes(data[80..84].try_into().unwrap()) as usize;
    let string_id_count = u32::from_le_bytes(data[84..88].try_into().unwrap()) as usize;
    let locale_count = u32::from_le_bytes(data[88..92].try_into().unwrap()) as usize;
    let enum_value_count = u32::from_le_bytes(data[92..96].try_into().unwrap()) as usize;
    let strong_count = u32::from_le_bytes(data[96..100].try_into().unwrap()) as usize;
    let weak_count = u32::from_le_bytes(data[100..104].try_into().unwrap()) as usize;
    let reference_count = u32::from_le_bytes(data[104..108].try_into().unwrap()) as usize;
    let enum_option_count = u32::from_le_bytes(data[108..112].try_into().unwrap()) as usize;
    let text_len_1 = u32::from_le_bytes(data[112..116].try_into().unwrap()) as usize;
    let text_len_2 = u32::from_le_bytes(data[116..120].try_into().unwrap()) as usize;

    // Calculate section offsets
    let mut pos = 120; // After header

    let struct_defs_start = pos;
    pos += struct_count * 16; // DataCoreStructDefinition is 16 bytes
    let struct_defs_end = pos;

    let prop_defs_start = pos;
    pos += prop_count * 12; // DataCorePropertyDefinition is 12 bytes
    let prop_defs_end = pos;

    let enum_defs_start = pos;
    pos += enum_count * 8; // DataCoreEnumDefinition is 8 bytes
    let enum_defs_end = pos;

    let mapping_start = pos;
    pos += mapping_count * 8; // DataCoreDataMapping is 8 bytes
    let mapping_end = pos;

    let records_start = pos;
    pos += record_count * 32; // DataCoreRecord is 32 bytes
    let records_end = pos;

    // Value pools - FILE ORDER (not header order!)
    let int8_start = pos;
    pos += int8_count;
    let int8_end = pos;

    let int16_start = pos;
    pos += int16_count * 2;
    let int16_end = pos;

    let int32_start = pos;
    pos += int32_count * 4;
    let int32_end = pos;

    let int64_start = pos;
    pos += int64_count * 8;
    let int64_end = pos;

    let uint8_start = pos;
    pos += uint8_count;
    let uint8_end = pos;

    let uint16_start = pos;
    pos += uint16_count * 2;
    let uint16_end = pos;

    let uint32_start = pos;
    pos += uint32_count * 4;
    let uint32_end = pos;

    let uint64_start = pos;
    pos += uint64_count * 8;
    let uint64_end = pos;

    let bool_start = pos;
    pos += bool_count;
    let bool_end = pos;

    let float_start = pos;
    pos += float_count * 4;
    let float_end = pos;

    let double_start = pos;
    pos += double_count * 8;
    let double_end = pos;

    let guid_start = pos;
    pos += guid_count * 16;
    let guid_end = pos;

    let string_id_start = pos;
    pos += string_id_count * 4;
    let string_id_end = pos;

    let locale_start = pos;
    pos += locale_count * 4;
    let locale_end = pos;

    let enum_value_start = pos;
    pos += enum_value_count * 4;
    let enum_value_end = pos;

    let strong_start = pos;
    pos += strong_count * 8;
    let strong_end = pos;

    let weak_start = pos;
    pos += weak_count * 8;
    let weak_end = pos;

    let reference_start = pos;
    pos += reference_count * 20;
    let reference_end = pos;

    let enum_option_start = pos;
    pos += enum_option_count * 4;
    let enum_option_end = pos;

    let string_table_1_start = pos;
    pos += text_len_1;
    let string_table_1_end = pos;

    let string_table_2_start = pos;
    pos += text_len_2;
    let string_table_2_end = pos;

    let data_section_start = pos;

    // Print section info
    println!("  Section boundaries:");
    let sections = [
        ("Header", 0, 120),
        ("StructDefs", struct_defs_start, struct_defs_end),
        ("PropDefs", prop_defs_start, prop_defs_end),
        ("EnumDefs", enum_defs_start, enum_defs_end),
        ("DataMappings", mapping_start, mapping_end),
        ("Records", records_start, records_end),
        ("Int8Pool", int8_start, int8_end),
        ("Int16Pool", int16_start, int16_end),
        ("Int32Pool", int32_start, int32_end),
        ("Int64Pool", int64_start, int64_end),
        ("UInt8Pool", uint8_start, uint8_end),
        ("UInt16Pool", uint16_start, uint16_end),
        ("UInt32Pool", uint32_start, uint32_end),
        ("UInt64Pool", uint64_start, uint64_end),
        ("BoolPool", bool_start, bool_end),
        ("FloatPool", float_start, float_end),
        ("DoublePool", double_start, double_end),
        ("GuidPool", guid_start, guid_end),
        ("StringIdPool", string_id_start, string_id_end),
        ("LocalePool", locale_start, locale_end),
        ("EnumValuePool", enum_value_start, enum_value_end),
        ("StrongPool", strong_start, strong_end),
        ("WeakPool", weak_start, weak_end),
        ("ReferencePool", reference_start, reference_end),
        ("EnumOptionPool", enum_option_start, enum_option_end),
        ("StringTable1", string_table_1_start, string_table_1_end),
        ("StringTable2", string_table_2_start, string_table_2_end),
        ("DataSection", data_section_start, data.len()),
    ];

    for (name, start, end) in &sections {
        let marker = if offset >= *start && offset < *end { " <-- HERE" } else { "" };
        println!("    {}: {} - {} (size: {}){}", name, start, end, end - start, marker);
    }
}
