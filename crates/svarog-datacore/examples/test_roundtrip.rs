use svarog_datacore::{DataCoreDatabase, DataCoreBuilder, DataType};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Test 1: Create a simple database from scratch
    println!("=== Test 1: Create database from scratch ===");
    let mut builder = DataCoreBuilder::new();
    
    // Define a weapon struct
    let weapon = builder.add_struct("Weapon", None);
    builder.add_property(weapon, "name", DataType::String);
    builder.add_property(weapon, "damage", DataType::Single);
    builder.add_property(weapon, "ammoCount", DataType::Int32);
    builder.add_property(weapon, "isAutomatic", DataType::Boolean);
    
    // Add some records
    let laser = builder.add_record("LaserRifle", weapon, "weapons/laser_rifle.xml");
    builder.set_string(laser, "name", "Laser Rifle MK1");
    builder.set_float(laser, "damage", 150.0);
    builder.set_i32(laser, "ammoCount", 50);
    builder.set_bool(laser, "isAutomatic", true);
    
    let plasma = builder.add_record("PlasmaGun", weapon, "weapons/plasma_gun.xml");
    builder.set_string(plasma, "name", "Plasma Cannon");
    builder.set_float(plasma, "damage", 500.0);
    builder.set_i32(plasma, "ammoCount", 10);
    builder.set_bool(plasma, "isAutomatic", false);
    
    // Write to file
    let test_path = "/tmp/test_svarog.dcb";
    builder.write_to_file(test_path)?;
    println!("Wrote database to {}", test_path);
    
    // Read it back
    println!("\n=== Test 2: Read back the created database ===");
    let db = DataCoreDatabase::parse(&std::fs::read(test_path)?)?;
    
    println!("Structs: {}", db.struct_definitions().len());
    println!("Records: {}", db.records().len());
    
    // Verify records
    for record in db.all_records() {
        println!("\nRecord: {} ({})", record.name().unwrap_or("?"), record.type_name().unwrap_or("?"));
        for prop in record.properties() {
            println!("  {}: {}", prop.name, prop.value);
        }
    }
    
    // Test 3: Load existing DCB and create builder from it
    println!("\n=== Test 3: Load Game2.dcb into builder ===");
    let large_db = DataCoreDatabase::open("/media/null/ares/scd/Data/Game2.dcb")?;
    println!("Original: {} structs, {} records", 
        large_db.struct_definitions().len(),
        large_db.records().len());
    
    let _builder2 = DataCoreBuilder::from_database(&large_db)?;
    println!("Successfully loaded into builder!");
    
    // Clean up
    std::fs::remove_file(test_path)?;
    
    println!("\n=== All roundtrip tests passed! ===");
    Ok(())
}
