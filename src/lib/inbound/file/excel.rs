//! Excel files: the first worksheet, read into memory.

use std::path::Path;
use std::time::Duration;

use anyhow::Context;
use calamine::{Data, Reader as CalamineReader};
use chrono::{NaiveDate, NaiveDateTime, NaiveTime};
use serde::Deserialize;
use serde_json::{Map, Value};

use crate::domain::models::action_object::ActionObject;
use crate::inbound::file::RowError;

/// Seconds in a day: an Excel serial date counts days, and its fraction is
/// the time of day.
const SECONDS_PER_DAY: u32 = 86_400;

fn excel_serial_to_datetime(serial: f64) -> Option<NaiveDateTime> {
    let excel_epoch = NaiveDate::from_ymd_opt(1899, 12, 30)?;
    #[expect(
        clippy::as_conversions,
        clippy::cast_possible_truncation,
        reason = "std has no checked f64 to i64; the cast saturates, and a day count out of range fails try_days below"
    )]
    let mut days = serial.floor() as i64;
    // Excel stores times as fractions of a day, so a time typed as 10:00:00
    // can come back a hair short; round to the nearest second.
    // A negative fraction reads as midnight.
    let rounded = (serial.fract() * f64::from(SECONDS_PER_DAY)).round();
    let mut seconds_in_day =
        u32::try_from(Duration::try_from_secs_f64(rounded).map_or(0, |d| d.as_secs())).ok()?;
    if seconds_in_day >= SECONDS_PER_DAY {
        days = days.checked_add(1)?;
        seconds_in_day = 0;
    }
    let time = NaiveTime::from_num_seconds_from_midnight_opt(seconds_in_day, 0)?;
    let date = excel_epoch.checked_add_signed(chrono::TimeDelta::try_days(days)?)?;
    Some(NaiveDateTime::new(date, time))
}

/// Every header [`ActionObject`] reads the ticket id from. A numeric cell
/// under one of these becomes a number, not text.
const TICKET_ID_HEADERS: &[&str] = &[
    "ticket_id",
    "requestId",
    "requestID",
    "RequestId",
    "RequestID",
    "requestid",
    "REQUESTID",
    "request_id",
];

/// A ticket ID from a numeric cell: a whole number that fits a `u32`.
fn ticket_id_from_float(value: f64) -> Option<u32> {
    let in_range = (0.0..=f64::from(u32::MAX)).contains(&value);
    if in_range && value.fract() == 0.0 {
        #[expect(
            clippy::as_conversions,
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "value is whole and within u32, checked above, so the cast is exact"
        )]
        Some(value as u32)
    } else {
        None
    }
}

/// The rows of an Excel file's first worksheet as actions, in sheet order.
/// Rows with no data in them are left out.
#[derive(Debug)]
pub struct Actions {
    rows: Vec<Vec<Data>>,
    headers: Vec<String>,
    file_name: String,
    sheet_name: String,
    row_num: usize,
    /// The spreadsheet row number of the first data row, as Excel shows it.
    first_data_row: usize,
}

impl Actions {
    /// How many rows the sheet has under the header, blank ones included.
    pub fn total_rows(&self) -> usize {
        self.rows.len()
    }

    /// The name of the worksheet being read.
    pub fn sheet_name(&self) -> &str {
        &self.sheet_name
    }
}

impl Iterator for Actions {
    type Item = Result<ActionObject, RowError>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            match self.read_row() {
                RowRead::End => return None,
                RowRead::Blank => {}
                RowRead::Row(item) => return Some(item),
            }
        }
    }
}

/// What reading the next row of a sheet came to.
enum RowRead {
    /// The sheet has no more rows.
    End,
    /// The row has no data in it.
    Blank,
    /// The row as an action, or why it is not one.
    Row(Result<ActionObject, RowError>),
}

impl Actions {
    fn read_row(&mut self) -> RowRead {
        let Some(row) = self.rows.get(self.row_num) else {
            return RowRead::End;
        };
        let row_number = self.first_data_row.saturating_add(self.row_num);
        let mut record = Map::new();
        let mut has_any_data = false;
        let mut bad_ticket_id: Option<String> = None;
        for (idx, header) in self.headers.iter().enumerate() {
            let column = Column::of(header);
            let cell = read_cell(row.get(idx).unwrap_or(&EMPTY), column);
            has_any_data |= cell.has_data;
            if cell.bad_ticket_id.is_some() {
                bad_ticket_id = cell.bad_ticket_id;
            }
            let key = if column == Column::Date {
                "actionDate".to_string()
            } else {
                header.clone()
            };
            record.insert(key, cell.value);
        }
        self.row_num = self.row_num.saturating_add(1);
        if let Some(value) = bad_ticket_id {
            return RowRead::Row(Err(RowError {
                row: row_number,
                message: format!(
                    "ticket id {value} in row {row_number} of worksheet '{}' of excel file '{}' is not a whole number from 0 to {}",
                    self.sheet_name,
                    self.file_name,
                    u32::MAX
                ),
            }));
        }
        if !has_any_data {
            return RowRead::Blank;
        }
        RowRead::Row(self.to_action(record, row_number))
    }

    /// Deserializes a row's `record` as an action.
    fn to_action(
        &self,
        record: Map<String, Value>,
        row_number: usize,
    ) -> Result<ActionObject, RowError> {
        let available_fields: Vec<String> = record.keys().cloned().collect();
        let json_value = Value::Object(record);
        match ActionObject::deserialize(&json_value) {
            Ok(action_object) => {
                if action_object.actiondate.is_none() {
                    tracing::warn!(
                        "Date field is None after deserialization. Available fields: {available_fields:?}"
                    );
                    if let Some(date_val) = json_value.get("actionDate") {
                        tracing::warn!("Date value in JSON was: {date_val:?}");
                    }
                }
                Ok(action_object)
            }
            Err(e) => {
                let json_str = serde_json::to_string(&json_value)
                    .unwrap_or_else(|_| "failed to serialize".to_string());
                let field_info = format!(
                    "Available fields: [{}]. Error: {e}",
                    available_fields.join(", ")
                );
                Err(RowError {
                    row: row_number,
                    message: format!(
                        "failed to deserialize row {row_number} in worksheet '{}' of excel file '{}': {field_info} (data: {json_str})",
                        self.sheet_name, self.file_name
                    ),
                })
            }
        }
    }
}

/// A cell past the end of a short row reads as empty.
const EMPTY: Data = Data::Empty;

/// How a date goes to the action's deserializer.
const DATE_FORMAT: &str = "%Y-%m-%dT%H:%M:%S";

/// The formats a date typed as text is tried in, in order.
const TEXT_DATE_FORMATS: &[&str] = &["%Y-%m-%dT%H:%M:%S%.f", "%Y-%m-%dT%H:%M:%S"];

/// The formats an ISO date cell is tried in, in order.
const ISO_DATE_FORMATS: &[&str] = &[
    "%Y-%m-%dT%H:%M:%S%.f",
    "%Y-%m-%dT%H:%M:%S",
    "%Y-%m-%d %H:%M:%S",
];

/// The first of `formats` that `text` parses in.
fn parse_date(text: &str, formats: &[&str]) -> Option<NaiveDateTime> {
    formats
        .iter()
        .find_map(|format| NaiveDateTime::parse_from_str(text, format).ok())
}

fn date_value(date: NaiveDateTime) -> Value {
    Value::String(date.format(DATE_FORMAT).to_string())
}

/// What a column holds, which decides how its cells read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Column {
    /// The action date: serial dates and date text become [`DATE_FORMAT`].
    Date,
    /// The ticket ID: a number that fits a `u32`.
    TicketId,
    /// Anything else, read as text.
    Text,
}

impl Column {
    fn of(header: &str) -> Self {
        if matches!(header.to_lowercase().as_str(), "actiondate" | "action_date") {
            Self::Date
        } else if TICKET_ID_HEADERS.contains(&header) {
            Self::TicketId
        } else {
            Self::Text
        }
    }
}

/// A cell as the row's JSON record takes it.
struct CellRead {
    value: Value,
    /// Whether the cell holds anything; a row of empty cells is skipped.
    has_data: bool,
    /// A ticket ID cell's value when it is not a whole number that fits.
    bad_ticket_id: Option<String>,
}

impl CellRead {
    fn data(value: Value) -> Self {
        Self {
            value,
            has_data: true,
            bad_ticket_id: None,
        }
    }
}

fn read_cell(cell: &Data, column: Column) -> CellRead {
    match cell {
        Data::Empty => CellRead {
            value: if column == Column::Date {
                Value::Null
            } else {
                Value::String(String::new())
            },
            has_data: false,
            bad_ticket_id: None,
        },
        Data::String(s) => {
            let trimmed = s.trim();
            let value = match column {
                Column::Date if trimmed.is_empty() => Value::Null,
                Column::Date => {
                    if let Ok(serial) = trimmed.parse::<f64>() {
                        excel_serial_to_datetime(serial)
                            .map_or_else(|| Value::String(s.clone()), date_value)
                    } else {
                        let clean = trimmed.trim_end_matches('Z').trim_end_matches('z');
                        parse_date(clean, TEXT_DATE_FORMATS)
                            .map_or_else(|| Value::String(s.clone()), date_value)
                    }
                }
                Column::TicketId => trimmed
                    .parse::<u32>()
                    .map_or_else(|_| Value::String(s.clone()), |id| Value::Number(id.into())),
                Column::Text => Value::String(s.clone()),
            };
            CellRead {
                value,
                has_data: !trimmed.is_empty(),
                bad_ticket_id: None,
            }
        }
        Data::Float(f) => match column {
            Column::Date => {
                CellRead::data(excel_serial_to_datetime(*f).map_or(Value::Null, date_value))
            }
            Column::TicketId => match ticket_id_from_float(*f) {
                Some(id) => CellRead::data(Value::Number(id.into())),
                None => CellRead {
                    bad_ticket_id: Some(f.to_string()),
                    ..CellRead::data(Value::Null)
                },
            },
            Column::Text => CellRead::data(Value::String(f.to_string())),
        },
        Data::Int(i) => match (column, u32::try_from(*i)) {
            (Column::TicketId, Ok(id)) => CellRead::data(Value::Number(id.into())),
            (Column::TicketId, Err(_)) => CellRead {
                bad_ticket_id: Some(i.to_string()),
                ..CellRead::data(Value::Null)
            },
            _ => CellRead::data(Value::String(i.to_string())),
        },
        Data::Bool(b) => CellRead::data(Value::String(b.to_string())),
        Data::DateTime(dt) if column == Column::Date => {
            let serial = dt.as_f64();
            CellRead::data(excel_serial_to_datetime(serial).map_or_else(
                || {
                    tracing::warn!(
                        "Failed to convert Excel DateTime serial {serial} to NaiveDateTime"
                    );
                    Value::Null
                },
                date_value,
            ))
        }
        Data::DateTime(dt) => CellRead::data(Value::String(dt.to_string())),
        Data::DateTimeIso(dt) if column == Column::Date => {
            let clean = dt.trim_end_matches('Z').trim_end_matches('z');
            CellRead::data(
                parse_date(clean, ISO_DATE_FORMATS)
                    .map_or_else(|| Value::String(clean.to_string()), date_value),
            )
        }
        Data::DateTimeIso(dt) => CellRead::data(Value::String(dt.clone())),
        Data::DurationIso(d) => CellRead::data(Value::String(d.clone())),
        Data::Error(e) => CellRead::data(Value::String(format!("{e:?}"))),
    }
}

/// Opens the Excel file at `path` and reads the rows of its first worksheet
/// as actions. A row that does not read is an error item, and the rows after
/// it still read.
///
/// # Errors
///
/// When the file cannot be opened or has no worksheet or header row.
pub fn actions(path: &Path) -> anyhow::Result<Actions> {
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown file")
        .to_string();
    let mut workbook = calamine::open_workbook_auto(path)
        .with_context(|| format!("failed to open excel file: {file_name}"))?;
    let sheet_names = workbook.sheet_names();
    let first_sheet_name = sheet_names
        .first()
        .ok_or_else(|| anyhow::anyhow!("excel file '{file_name}' has no worksheets"))?
        .clone();
    let range = workbook
        .worksheet_range(&first_sheet_name)
        .with_context(|| {
            format!("failed to read worksheet '{first_sheet_name}' from excel file: {file_name}")
        })?;
    // The range starts at the first non-empty cell; Excel numbers rows from 1
    // and the header takes the first one.
    let header_row = range
        .start()
        .and_then(|(row, _)| usize::try_from(row).ok())
        .unwrap_or(0);
    let mut rows_iter = range.rows();
    let headers: Vec<String> = match rows_iter.next() {
        Some(header_row) => header_row
            .iter()
            .map(std::string::ToString::to_string)
            .collect(),
        None => {
            return Err(anyhow::anyhow!(
                "first worksheet '{first_sheet_name}' of excel file '{file_name}' has no header row"
            ));
        }
    };
    let rows: Vec<Vec<Data>> = rows_iter
        .map(|row| {
            let mut row_vec = row.to_vec();
            while row_vec.len() < headers.len() {
                row_vec.push(Data::Empty);
            }
            row_vec
        })
        .collect();
    Ok(Actions {
        rows,
        headers,
        file_name,
        sheet_name: first_sheet_name,
        row_num: 0,
        first_data_row: header_row.saturating_add(2),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn excel_serial_dates_count_days_from_the_1900_epoch_with_the_lotus_bug() {
        let at = |y, m, d, h, min| {
            NaiveDate::from_ymd_opt(y, m, d)
                .unwrap()
                .and_hms_opt(h, min, 0)
                .unwrap()
        };
        assert_eq!(excel_serial_to_datetime(0.0), Some(at(1899, 12, 30, 0, 0)));
        assert_eq!(
            excel_serial_to_datetime(1.75),
            Some(at(1899, 12, 31, 18, 0))
        );
        assert_eq!(
            excel_serial_to_datetime(45658.5),
            Some(at(2025, 1, 1, 12, 0))
        );
    }

    fn sheet(headers: &[&str], rows: Vec<Vec<Data>>) -> Actions {
        Actions {
            rows,
            headers: headers.iter().map(|h| (*h).to_string()).collect(),
            file_name: "book.xlsx".to_string(),
            sheet_name: "Sheet1".to_string(),
            row_num: 0,
            first_data_row: 2,
        }
    }

    fn row(ticket: Data) -> Vec<Data> {
        vec![
            ticket,
            Data::Int(1),
            Data::String("w".to_string()),
            Data::String("n".to_string()),
            Data::Empty,
        ]
    }

    const HEADERS: &[&str] = &["requestId", "actionId", "actionWho", "note", "actionDate"];

    #[test]
    fn a_ticket_id_that_does_not_fit_or_is_not_whole_is_a_row_error() {
        for bad in [
            Data::Float(4_294_967_301.0),
            Data::Float(10.5),
            Data::Float(-1.0),
            Data::Int(4_294_967_301),
            Data::Int(-1),
        ] {
            let mut iter = sheet(HEADERS, vec![row(bad.clone())]);
            let message = iter.next().unwrap().unwrap_err().to_string();
            assert!(message.contains("row"), "{bad:?}: {message}");
        }
    }

    #[test]
    fn a_whole_ticket_id_reads_from_a_float_or_an_int() {
        let mut iter = sheet(
            HEADERS,
            vec![row(Data::Float(10.0)), row(Data::Int(4_294_967_295))],
        );
        assert_eq!(iter.next().unwrap().unwrap().ticket_id, 10);
        assert_eq!(iter.next().unwrap().unwrap().ticket_id, u32::MAX);
    }

    #[test]
    fn a_long_run_of_blank_rows_is_skipped_without_exhausting_the_stack() {
        let mut rows = vec![vec![Data::Empty; HEADERS.len()]; 200_000];
        rows.push(row(Data::Int(7)));
        let mut iter = sheet(HEADERS, rows);
        assert_eq!(iter.next().unwrap().unwrap().ticket_id, 7);
        assert!(iter.next().is_none());
    }

    #[test]
    fn a_request_id_header_is_read_as_the_ticket() {
        let headers = ["request_id", "actionId", "actionWho", "note", "actionDate"];
        let mut iter = sheet(&headers, vec![row(Data::Float(12.0))]);
        assert_eq!(iter.next().unwrap().unwrap().ticket_id, 12);
    }

    #[test]
    fn a_numeric_ticket_reads_under_every_ticket_header_the_model_accepts() {
        for header in [
            "ticket_id",
            "requestId",
            "requestID",
            "RequestId",
            "RequestID",
            "requestid",
            "REQUESTID",
            "request_id",
        ] {
            let headers = [header, "actionId", "actionWho", "note", "actionDate"];
            for cell in [Data::Float(12.0), Data::Int(12)] {
                let mut iter = sheet(&headers, vec![row(cell.clone())]);
                let ticket = iter.next().unwrap().map(|action| action.ticket_id);
                assert_eq!(ticket, Ok(12), "{header} {cell:?}");
            }
        }
    }

    #[test]
    fn excel_times_round_to_the_nearest_second() {
        let at = |d, h, min, sec| {
            NaiveDate::from_ymd_opt(2025, 1, d)
                .unwrap()
                .and_hms_opt(h, min, sec)
                .unwrap()
        };
        let serial = |seconds: f64| 45658.0 + seconds / f64::from(SECONDS_PER_DAY);
        assert_eq!(
            excel_serial_to_datetime(serial(35_999.999_9)),
            Some(at(1, 10, 0, 0))
        );
        assert_eq!(
            excel_serial_to_datetime(serial(35_999.4)),
            Some(at(1, 9, 59, 59))
        );
        assert_eq!(
            excel_serial_to_datetime(serial(86_399.999_9)),
            Some(at(2, 0, 0, 0))
        );
    }

    #[test]
    fn excel_row_numbers_in_errors_count_the_header_row() {
        let mut iter = sheet(HEADERS, vec![row(Data::Float(10.5))]);
        let message = iter.next().unwrap().unwrap_err().to_string();
        assert!(message.contains("row 2 "), "{message}");
    }
}
