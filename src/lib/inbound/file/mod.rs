pub mod csv;
pub mod excel;

pub use csv::{Csv, CsvActionIterator};
pub use excel::{Excel, ExcelActionIterator};
use std::{ffi::OsString, fs::FileType, path::Path};

pub struct Reader;

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
