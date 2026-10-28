//! CSV files, read as a stream.

use std::path::Path;

use anyhow::Context;
use csv::ReaderBuilder;

use crate::domain::models::action_object::ActionObject;
use crate::inbound::file::RowError;

/// The rows of a CSV file as actions, in file order.
pub struct Actions {
    rdr: csv::DeserializeRecordsIntoIter<std::fs::File, ActionObject>,
    file_name: String,
    row_num: usize,
    total_rows: usize,
}

impl Actions {
    /// How many data rows the file has, good or bad.
    pub fn total_rows(&self) -> usize {
        self.total_rows
    }
}

impl Iterator for Actions {
    type Item = Result<ActionObject, RowError>;

    fn next(&mut self) -> Option<Self::Item> {
        let result = self.rdr.next()?;
        // Every record counts, good or bad, so the numbers match the file's data rows.
        self.row_num += 1;
        Some(result.map_err(|e| RowError {
            row: self.row_num,
            message: format!(
                "failed to deserialize row {} in csv file: {}: {}",
                self.row_num, self.file_name, e
            ),
        }))
    }
}

/// Opens the CSV file at `path` and reads its rows as actions. A row that
/// does not read is an error item, and the rows after it still read.
///
/// # Errors
///
/// When the file cannot be opened.
pub fn actions(path: &Path) -> anyhow::Result<Actions> {
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
        count
    };
    let file = std::fs::File::open(path)
        .with_context(|| format!("failed to open csv file: {}", file_name))?;
    let rdr = ReaderBuilder::new().has_headers(true).from_reader(file);
    Ok(Actions {
        rdr: rdr.into_deserialize(),
        file_name,
        row_num: 0,
        total_rows,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::models::action_object::ActionId;

    /// Every row of the file, or the first row error.
    fn read_all(path: &Path) -> Result<Vec<ActionObject>, RowError> {
        actions(path).unwrap().collect()
    }

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
        let iter = actions(&path).unwrap();
        assert_eq!(iter.total_rows(), 2);
        let actions: Vec<ActionObject> = iter.map(Result::unwrap).collect();
        std::fs::remove_file(&path).unwrap();

        assert_eq!(actions[0].ticket_id, 10);
        assert_eq!(actions[0].action_id(), ActionId::new(500));
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
        let actions = read_all(&path).unwrap();
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
        let message = read_all(&path).unwrap_err().to_string();
        std::fs::remove_file(&path).unwrap();
        assert!(message.contains("row 2"), "{message}");
        assert!(message.contains("bad.csv"), "{message}");
    }

    #[test]
    fn an_action_id_that_is_not_a_number_is_a_row_error() {
        let path = write(
            "bad_id.csv",
            "ticket_id,actionId,actionwho,note,actiondate\n\
             1,abc,w,n,\n",
        );
        let message = read_all(&path).unwrap_err().to_string();
        std::fs::remove_file(&path).unwrap();
        assert!(message.contains("row 1"), "{message}");
        assert!(message.contains("abc"), "{message}");
    }

    #[test]
    fn an_action_id_is_read_as_a_trimmed_number() {
        let path = write(
            "padded_id.csv",
            "ticket_id,actionId,actionwho,note,actiondate\n\
             1, 0042 ,w,n,\n",
        );
        let actions = read_all(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(actions[0].action_id().to_string(), "42");
    }

    #[test]
    fn a_request_id_header_is_read_as_the_ticket() {
        let path = write(
            "request_id.csv",
            "request_id,actionId,actionwho,note,actiondate\n\
             12,1,w,n,\n",
        );
        let actions = read_all(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(actions[0].ticket_id, 12);
    }

    #[test]
    fn row_numbers_stay_right_after_a_bad_row() {
        let path = write(
            "two_bad.csv",
            "ticket_id,actionId,actionwho,note,actiondate\n\
             1,1,w,n,\n\
             nope,2,w,n,\n\
             nope,3,w,n,\n",
        );
        let errors: Vec<String> = actions(&path)
            .unwrap()
            .filter_map(Result::err)
            .map(|e| e.to_string())
            .collect();
        std::fs::remove_file(&path).unwrap();
        assert!(errors[0].contains("row 2"), "{}", errors[0]);
        assert!(errors[1].contains("row 3"), "{}", errors[1]);
    }
}
