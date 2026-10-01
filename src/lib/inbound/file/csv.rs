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

#[cfg(test)]
mod tests {
    use super::*;

    fn write(name: &str, csv: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("halo_{}_{name}", std::process::id()));
        std::fs::write(&path, csv).unwrap();
        path
    }

    #[test]
    fn rows_become_actions_under_any_accepted_header_spelling() {
        let path = write(
            "aliases.csv",
            "requestId,CFactionId,actionWho,note,actionDate\n\
             10,500,who,hello,2026-01-15T10:00:00.000Z\n\
             11,501,who,again,\n",
        );
        let iter = Reader::csv_action_iter(&path).unwrap();
        assert_eq!(iter.total_rows(), Some(2));
        let actions: Vec<ActionObject> = iter.map(Result::unwrap).collect();
        std::fs::remove_file(&path).unwrap();

        assert_eq!(actions[0].ticket_id, 10);
        assert_eq!(actions[0].action_id(), "500");
        assert_eq!(actions[0].outcome, "Imported Note");
        assert_eq!(
            actions[0].actiondate.unwrap().to_string(),
            "2026-01-15 10:00:00"
        );
        assert_eq!(actions[1].actiondate, None);
    }

    #[test]
    fn every_date_spelling_the_exports_use_is_read() {
        let csv = "ticket_id,actionId,actionwho,note,actiondate\n\
                   1,1,w,n,2026-01-15T10:00:00\n\
                   1,2,w,n,2026-01-15 10:00:00\n\
                   1,3,w,n,2026-01-15T10:00:00Z\n\
                   1,4,w,n,2026-01-15T10:00:00.5\n";
        let path = write("dates.csv", csv);
        let actions = Reader::try_csv_to_action_objects(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        for action in &actions {
            assert_eq!(
                action.actiondate.unwrap().date().to_string(),
                "2026-01-15",
                "{action:?}"
            );
        }
    }

    #[test]
    fn a_bad_row_is_reported_with_its_number_and_the_file() {
        let path = write(
            "bad.csv",
            "ticket_id,actionId,actionwho,note,actiondate\n\
             1,1,w,n,\n\
             nope,2,w,n,\n",
        );
        let message = Reader::try_csv_to_action_objects(&path)
            .unwrap_err()
            .to_string();
        std::fs::remove_file(&path).unwrap();
        assert!(message.contains("row 2"), "{message}");
        assert!(message.contains("bad.csv"), "{message}");
    }
}
