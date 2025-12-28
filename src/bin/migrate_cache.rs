use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ResourceCache {
    resource_id: String,
    action_ids: Vec<String>,
}

const RESOURCE_CACHE_FILE: &str = "cache/existing_action_ids.json";
const IMPORTED_CACHE_FILE: &str = "cache/imported_ids.txt";

fn main() -> anyhow::Result<()> {
    println!("=== Cache Migration Tool ===");
    println!("Migrating _imported entries from JSON to separate text file\n");

    let json_path = Path::new(RESOURCE_CACHE_FILE);
    if !json_path.exists() {
        println!(
            "No cache file found at {}. Nothing to migrate.",
            RESOURCE_CACHE_FILE
        );
        return Ok(());
    }

    println!("Reading {}...", RESOURCE_CACHE_FILE);
    let contents = fs::read_to_string(json_path)?;
    let mut resources: Vec<ResourceCache> = serde_json::from_str(&contents)?;

    println!("Found {} resource entries", resources.len());

    // Find and extract _imported
    let imported_idx = resources.iter().position(|r| r.resource_id == "_imported");

    if let Some(idx) = imported_idx {
        let imported = resources.remove(idx);
        let imported_count = imported.action_ids.len();

        println!("Found _imported with {} action IDs", imported_count);

        // Read existing imported_ids.txt if it exists
        let mut existing_imported: HashSet<String> = HashSet::new();
        let txt_path = Path::new(IMPORTED_CACHE_FILE);
        if txt_path.exists() {
            let file = fs::File::open(txt_path)?;
            let reader = BufReader::new(file);
            for line in reader.lines() {
                if let Ok(id) = line {
                    let id = id.trim();
                    if !id.is_empty() {
                        existing_imported.insert(id.to_string());
                    }
                }
            }
            println!(
                "Found {} existing IDs in {}",
                existing_imported.len(),
                IMPORTED_CACHE_FILE
            );
        }

        // Merge and write
        let before_count = existing_imported.len();
        for id in imported.action_ids {
            existing_imported.insert(id);
        }
        let new_count = existing_imported.len() - before_count;

        println!(
            "Writing {} IDs to {} ({} new)",
            existing_imported.len(),
            IMPORTED_CACHE_FILE,
            new_count
        );

        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(IMPORTED_CACHE_FILE)?;

        for id in &existing_imported {
            writeln!(file, "{}", id)?;
        }

        // Write back JSON without _imported
        println!(
            "Updating {} (removing _imported entry)...",
            RESOURCE_CACHE_FILE
        );
        let json = serde_json::to_string_pretty(&resources)?;
        fs::write(RESOURCE_CACHE_FILE, json)?;

        println!("\n=== Migration Complete ===");
        println!("- Removed _imported from JSON");
        println!("- {} resource entries remain in JSON", resources.len());
        println!(
            "- {} imported IDs now in {}",
            existing_imported.len(),
            IMPORTED_CACHE_FILE
        );
    } else {
        println!("No _imported entry found in JSON. Nothing to migrate.");
    }

    Ok(())
}
