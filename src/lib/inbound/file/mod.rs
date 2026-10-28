pub mod csv;
pub mod excel;

use std::ffi::OsString;
use std::fs::FileType;
use std::path::Path;

pub use csv::{Csv, CsvActionIterator};
pub use excel::{Excel, ExcelActionIterator};

pub struct Reader;

/// A data row that does not read as an action. The message names the file
/// and the row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowError {
    /// The row's number as the file shows it.
    pub row: usize,
    pub message: String,
}

impl std::fmt::Display for RowError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for RowError {}

impl Reader {
    pub fn get_paths_of_type(dir: &Path, file_type: FileType) -> anyhow::Result<Vec<OsString>> {
        let mut output: Vec<OsString> = Vec::new();
        for entry_result in std::fs::read_dir(dir)? {
            let entry = entry_result?;
            if entry.file_type()? == file_type {
                output.push(entry.file_name());
            }
        }
        Ok(output)
    }
}
