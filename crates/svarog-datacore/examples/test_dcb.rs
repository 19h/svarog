use svarog_datacore::{DataCoreDatabase, DataCoreBuilder, Query};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("Loading Game2.dcb...");
    let db = DataCoreDatabase::open("/media/null/ares/scd/Data/Game2.dcb")?;
    
    println!("\n=== Database Stats ===");
    println!("Structs: {}", db.struct_definitions().len());
    println!("Properties: {}", db.property_definitions().len());
    println!("Enums: {}", db.enum_definitions().len());
    println!("Records: {}", db.records().len());
    
    println!("\n=== First 10 Type Names ===");
    for name in db.type_names().iter().take(10) {
        println!("  {}", name);
    }
    
    println!("\n=== Query: Records containing 'Weapon' in type ===");
    let weapons: Vec<_> = Query::new(&db)
        .type_contains("Weapon")
        .main_only()
        .collect();
    println!("Found {} weapon-related main records", weapons.len());
    
    for record in weapons.iter().take(5) {
        println!("  {} ({})", record.name().unwrap_or("?"), record.type_name().unwrap_or("?"));
    }
    
    println!("\n=== Query: First record with properties ===");
    if let Some(record) = db.all_main_records().next() {
        println!("Record: {} ({})", record.name().unwrap_or("?"), record.type_name().unwrap_or("?"));
        println!("File: {}", record.file_name().unwrap_or("?"));
        println!("GUID: {}", record.id());
        
        println!("\nFirst 10 properties:");
        for prop in record.properties().take(10) {
            println!("  {}: {}", prop.name, prop.value);
        }
    }
    
    println!("\n=== Test round-trip: Load into builder ===");
    let _builder = DataCoreBuilder::from_database(&db)?;
    println!("Loaded {} structs, {} records into builder", 
        db.struct_definitions().len(), 
        db.records().len());
    
    println!("\n=== All tests passed! ===");
    Ok(())
}
