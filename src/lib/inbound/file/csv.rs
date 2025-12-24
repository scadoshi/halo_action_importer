use crate::{domain::models::action_object::ActionObject, inbound::file::Reader};
use anyhow::Context;
use csv::ReaderBuilder;
use std::path::Path;

pub trait Csv {
    fn try_csv_to_action_objects(path: &Path) -> anyhow::Result<Vec<ActionObject>>;
    fn csv_action_iter(path: &Path) -> anyhow::Result<CsvActionIterator>;
}

pub struct CsvActionIterator {
    rdr: csv::DeserializeRecordsIntoIter<std::fs::File, ActionObject>,
    file_name: String,
    row_num: usize,
    total_rows: Option<usize>,
}

impl CsvActionIterator {
    pub fn total_rows(&self) -> Option<usize> {
        self.total_rows
    }
}

impl Iterator for CsvActionIterator {
    type Item = anyhow::Result<ActionObject>;

    fn next(&mut self) -> Option<Self::Item> {
        match self.rdr.next() {
            Some(Ok(action)) => {
                self.row_num += 1;
                Some(Ok(action))
            }
            Some(Err(e)) => Some(Err(anyhow::anyhow!(
                "failed to deserialize row {} in csv file: {}: {}",
                self.row_num + 1,
                self.file_name,
                e
            ))),
            None => None,
        }
    }
}

impl Csv for Reader {
    fn try_csv_to_action_objects(path: &Path) -> anyhow::Result<Vec<ActionObject>> {
        let iter = Self::csv_action_iter(path)?;
        let mut output = Vec::new();
        for result in iter {
            output.push(result?);
        }
        Ok(output)
    }

    fn csv_action_iter(path: &Path) -> anyhow::Result<CsvActionIterator> {
        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown file")
            .to_string();
        let total_rows = {
            let file = std::fs::File::open(path)
                .with_context(|| format!("failed to open csv file: {}", file_name))?;
            let mut rdr = ReaderBuilder::new().has_headers(true).from_reader(file);
            let mut count = 0;
            let mut records = rdr.records();
            while records.next().is_some() {
                count += 1;
            }
            Some(count)
        };
        let file = std::fs::File::open(path)
            .with_context(|| format!("failed to open csv file: {}", file_name))?;
        let rdr = ReaderBuilder::new().has_headers(true).from_reader(file);
        Ok(CsvActionIterator {
            rdr: rdr.into_deserialize(),
            file_name,
            row_num: 0,
            total_rows,
        })
    }
}
