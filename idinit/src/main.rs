use anyhow::{Context, Result};
use calamine::{open_workbook, Reader, Xlsx};
use csv::{ReaderBuilder, WriterBuilder};
use std::fs;
use std::path::{Path, PathBuf};

const INPUT_DIR: &str = "../input/new2/";

#[derive(Debug)]
struct FileData {
    path: PathBuf,
    file_type: FileType,
}

#[derive(Debug)]
enum FileType {
    Csv,
    Excel,
}

fn main() -> Result<()> {
    println!("ID Initializer - Adding CFactionId to files");
    println!("Scanning directory: {}", INPUT_DIR);

    // Step 1: Find all files
    let files = find_all_files(INPUT_DIR)?;
    println!("Found {} file(s) to process", files.len());

    // Step 2: Find maximum ID across all files
    let max_id = find_max_id(&files)?;
    println!("Maximum CFactionId found: {}", max_id);

    // Step 3: Process each file and add missing IDs
    let mut next_id = max_id + 1;
    let mut skipped_files = Vec::new();

    for file in &files {
        println!("\nProcessing: {:?}", file.path.file_name().unwrap());
        let result = match file.file_type {
            FileType::Csv => process_csv(&file.path, next_id),
            FileType::Excel => process_excel(&file.path, next_id),
        };

        match result {
            Ok(new_next_id) => {
                next_id = new_next_id;
            }
            Err(e) => {
                println!("  ⚠ ERROR: {} - SKIPPING", e);
                skipped_files.push(file.path.file_name().unwrap().to_string_lossy().to_string());
            }
        }
    }

    if !skipped_files.is_empty() {
        println!("\n⚠ Skipped {} file(s) due to errors:", skipped_files.len());
        for file in &skipped_files {
            println!("  - {}", file);
        }
        println!("\nTip: Convert skipped files to CSV format and run again");
    }

    println!("\n✓ Complete! Next available ID: {}", next_id);
    Ok(())
}

fn find_all_files(dir: &str) -> Result<Vec<FileData>> {
    let mut files = Vec::new();
    let entries = fs::read_dir(dir).context("Failed to read input directory")?;

    for entry in entries {
        let entry = entry?;
        let path = entry.path();

        if path.is_file() {
            if let Some(ext) = path.extension() {
                let ext_str = ext.to_string_lossy().to_lowercase();
                let file_type = match ext_str.as_str() {
                    "csv" => Some(FileType::Csv),
                    "xlsx" | "xls" => Some(FileType::Excel),
                    _ => None,
                };

                if let Some(ft) = file_type {
                    files.push(FileData {
                        path,
                        file_type: ft,
                    });
                }
            }
        }
    }

    Ok(files)
}

fn find_max_id(files: &[FileData]) -> Result<u64> {
    let mut max_id = 0u64;

    for file in files {
        match file.file_type {
            FileType::Csv => {
                let file_max = find_max_id_csv(&file.path)?;
                max_id = max_id.max(file_max);
            }
            FileType::Excel => {
                let file_max = find_max_id_excel(&file.path)?;
                max_id = max_id.max(file_max);
            }
        }
    }

    Ok(max_id)
}

fn find_max_id_csv(path: &Path) -> Result<u64> {
    let mut rdr = ReaderBuilder::new().has_headers(true).from_path(path)?;

    let headers = rdr.headers()?.clone();
    let cfactionid_idx = headers
        .iter()
        .position(|h| h.to_lowercase() == "cfactionid" || h.to_lowercase() == "actionid");

    let mut max_id = 0u64;

    if let Some(idx) = cfactionid_idx {
        for result in rdr.records() {
            let record = result?;
            if let Some(id_str) = record.get(idx) {
                if let Ok(id) = id_str.trim().parse::<u64>() {
                    max_id = max_id.max(id);
                }
            }
        }
    }

    Ok(max_id)
}

fn find_max_id_excel(path: &Path) -> Result<u64> {
    let mut workbook: Xlsx<_> = open_workbook(path)?;
    let mut max_id = 0u64;

    for sheet_name in workbook.sheet_names().to_vec() {
        if let Ok(range) = workbook.worksheet_range(&sheet_name) {
            let mut cfactionid_idx: Option<usize> = None;

            for (row_idx, row) in range.rows().enumerate() {
                if row_idx == 0 {
                    // Header row
                    let headers: Vec<String> = row
                        .iter()
                        .map(|cell| cell.to_string().to_lowercase())
                        .collect();
                    cfactionid_idx = headers
                        .iter()
                        .position(|h| h == "cfactionid" || h == "actionid");
                } else {
                    // Data rows
                    if let Some(idx) = cfactionid_idx {
                        if let Some(cell) = row.get(idx) {
                            let cell_str = cell.to_string();
                            if let Ok(id) = cell_str.trim().parse::<u64>() {
                                max_id = max_id.max(id);
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(max_id)
}

fn process_csv(path: &Path, mut next_id: u64) -> Result<u64> {
    // Read all data
    let mut rdr = ReaderBuilder::new().has_headers(true).from_path(path)?;

    let headers = rdr.headers()?.clone();
    let mut header_vec: Vec<String> = headers.iter().map(|h| h.to_string()).collect();

    let cfactionid_idx = header_vec
        .iter()
        .position(|h| h.to_lowercase() == "cfactionid" || h.to_lowercase() == "actionid");

    let needs_column = cfactionid_idx.is_none();
    let col_idx = if needs_column {
        header_vec.insert(0, "CFactionId".to_string());
        0
    } else {
        cfactionid_idx.unwrap()
    };

    // Read all records
    let mut records: Vec<Vec<String>> = Vec::new();
    for result in rdr.records() {
        let record = result?;
        let mut row: Vec<String> = record.iter().map(|f| f.to_string()).collect();

        if needs_column {
            row.insert(0, "".to_string());
        }

        records.push(row);
    }

    // Fill in missing IDs
    let mut filled_count = 0;
    for row in &mut records {
        if col_idx < row.len() {
            let id_str = row[col_idx].trim();
            if id_str.is_empty() || id_str.parse::<u64>().is_err() {
                row[col_idx] = next_id.to_string();
                next_id += 1;
                filled_count += 1;
            }
        }
    }

    // Write back
    let mut wtr = WriterBuilder::new().from_path(path)?;
    wtr.write_record(&header_vec)?;
    for row in records {
        wtr.write_record(&row)?;
    }
    wtr.flush()?;

    println!("  → Added {} ID(s)", filled_count);
    Ok(next_id)
}

fn process_excel(path: &Path, mut next_id: u64) -> Result<u64> {
    // Read the Excel file
    let mut reader: Xlsx<_> = open_workbook(path)?;
    let sheet_names = reader.sheet_names().to_vec();
    let mut total_filled = 0;

    // Get base filename without extension
    let base_name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("output");

    // Only process the first sheet
    if let Some(sheet_name) = sheet_names.first() {
        let range = reader.worksheet_range(&sheet_name)?;

        let mut headers: Vec<String> = Vec::new();
        let mut cfactionid_idx: Option<usize> = None;
        let mut needs_column = false;
        let mut col_idx = 0;
        let mut rows_data: Vec<Vec<String>> = Vec::new();

        // Process rows
        for (row_idx, row) in range.rows().enumerate() {
            if row_idx == 0 {
                // Header row
                headers = row.iter().map(|cell| cell.to_string()).collect();
                cfactionid_idx = headers.iter().position(|h| {
                    h.to_lowercase() == "cfactionid" || h.to_lowercase() == "actionid"
                });

                needs_column = cfactionid_idx.is_none();
                col_idx = if needs_column {
                    headers.insert(0, "CFactionId".to_string());
                    0
                } else {
                    cfactionid_idx.unwrap()
                };
            } else {
                // Data row
                let mut values: Vec<String> = row.iter().map(|cell| cell.to_string()).collect();

                if needs_column {
                    values.insert(0, "".to_string());
                }

                // Check if ID needs to be filled
                let id_str = values.get(col_idx).map(|s| s.as_str()).unwrap_or("");
                if id_str.trim().is_empty() || id_str.trim().parse::<u64>().is_err() {
                    values[col_idx] = next_id.to_string();
                    next_id += 1;
                    total_filled += 1;
                }

                rows_data.push(values);
            }
        }

        // Write to CSV file (just base name, no sheet suffix)
        let csv_filename = format!("{}.csv", base_name);
        let csv_path = path.parent().unwrap().join(&csv_filename);

        let mut wtr = WriterBuilder::new().from_path(&csv_path)?;
        wtr.write_record(&headers)?;
        for row in rows_data {
            wtr.write_record(&row)?;
        }
        wtr.flush()?;

        println!("  → Added {} ID(s), saved as {}", total_filled, csv_filename);
    }

    // Move original Excel file to old_excel folder
    let old_excel_dir = path.parent().unwrap().join("old_excel");
    fs::create_dir_all(&old_excel_dir)?;
    let new_path = old_excel_dir.join(path.file_name().unwrap());
    fs::rename(path, new_path)?;

    Ok(next_id)
}
