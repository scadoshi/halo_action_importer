use std::path::Path;

use anyhow::Context;
use calamine::{Data, Reader as CalamineReader, open_workbook_auto};
use chrono::{NaiveDate, NaiveDateTime, NaiveTime};

use crate::domain::models::action_object::ActionObject;
use crate::inbound::file::Reader;

fn excel_serial_to_datetime(serial: f64) -> Option<NaiveDateTime> {
    let excel_epoch = NaiveDate::from_ymd_opt(1899, 12, 30)?;
    let days = serial.floor() as i64;
    let seconds_in_day = (serial.fract() * 86400.0).floor() as u32;
    let time = NaiveTime::from_num_seconds_from_midnight_opt(seconds_in_day, 0)?;
    let date = excel_epoch.checked_add_signed(chrono::Duration::days(days))?;
    Some(NaiveDateTime::new(date, time))
}

/// A ticket ID from a numeric cell: a whole number that fits a `u32`.
fn ticket_id_from_float(value: f64) -> Option<u32> {
    let in_range = (0.0..=f64::from(u32::MAX)).contains(&value);
    if in_range && value.fract() == 0.0 {
        Some(value as u32)
    } else {
        None
    }
}

pub trait Excel {
    fn try_excel_to_action_objects(path: &Path) -> anyhow::Result<Vec<ActionObject>>;
    fn excel_action_iter(path: &Path) -> anyhow::Result<ExcelActionIterator>;
}

pub struct ExcelActionIterator {
    rows: Vec<Vec<Data>>,
    headers: Vec<String>,
    file_name: String,
    sheet_name: String,
    row_num: usize,
}

impl ExcelActionIterator {
    pub fn total_rows(&self) -> usize {
        self.rows.len()
    }

    pub fn sheet_name(&self) -> &str {
        &self.sheet_name
    }
}

impl Iterator for ExcelActionIterator {
    type Item = anyhow::Result<ActionObject>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.row_num >= self.rows.len() {
            return None;
        }
        let row = &self.rows[self.row_num];
        let row_num_for_error = self.row_num + 1;
        let mut record = serde_json::Map::new();
        let mut has_any_data = false;
        let mut bad_ticket_id: Option<String> = None;
        for (idx, header) in self.headers.iter().enumerate() {
            let header_lower = header.to_lowercase();
            let is_date_field = header_lower == "actiondate"
                || header_lower == "action_date"
                || header == "actionDate"
                || header == "ActionDate"
                || header == "ACTIONDATE";
            let is_numeric_field = header_lower == "requestid"
                || header_lower == "request_id"
                || header == "requestId"
                || header == "RequestId"
                || header == "REQUESTID";
            let cell_value = if idx < row.len() {
                match &row[idx] {
                    Data::Empty => {
                        if is_date_field {
                            serde_json::Value::Null
                        } else {
                            serde_json::Value::String(String::new())
                        }
                    }
                    Data::String(s) => {
                        let trimmed = s.trim();
                        if !trimmed.is_empty() {
                            has_any_data = true;
                        }
                        if is_date_field {
                            if trimmed.is_empty() {
                                serde_json::Value::Null
                            } else if let Ok(serial) = trimmed.parse::<f64>() {
                                match excel_serial_to_datetime(serial) {
                                    Some(dt) => serde_json::Value::String(
                                        dt.format("%Y-%m-%dT%H:%M:%S").to_string(),
                                    ),
                                    None => serde_json::Value::String(s.clone()),
                                }
                            } else {
                                let dt_clean = trimmed.trim_end_matches('Z').trim_end_matches('z');
                                match NaiveDateTime::parse_from_str(
                                    dt_clean,
                                    "%Y-%m-%dT%H:%M:%S%.f",
                                ) {
                                    Ok(ndt) => serde_json::Value::String(
                                        ndt.format("%Y-%m-%dT%H:%M:%S").to_string(),
                                    ),
                                    Err(_) => {
                                        match NaiveDateTime::parse_from_str(
                                            dt_clean,
                                            "%Y-%m-%dT%H:%M:%S",
                                        ) {
                                            Ok(ndt) => serde_json::Value::String(
                                                ndt.format("%Y-%m-%dT%H:%M:%S").to_string(),
                                            ),
                                            Err(_) => serde_json::Value::String(s.clone()),
                                        }
                                    }
                                }
                            }
                        } else if is_numeric_field {
                            if let Ok(num) = trimmed.parse::<u32>() {
                                serde_json::Value::Number(num.into())
                            } else {
                                serde_json::Value::String(s.clone())
                            }
                        } else {
                            serde_json::Value::String(s.clone())
                        }
                    }
                    Data::Float(f) => {
                        has_any_data = true;
                        if is_date_field {
                            match excel_serial_to_datetime(*f) {
                                Some(dt) => serde_json::Value::String(
                                    dt.format("%Y-%m-%dT%H:%M:%S").to_string(),
                                ),
                                None => serde_json::Value::Null,
                            }
                        } else if is_numeric_field {
                            match ticket_id_from_float(*f) {
                                Some(id) => serde_json::Value::Number(id.into()),
                                None => {
                                    bad_ticket_id = Some(f.to_string());
                                    serde_json::Value::Null
                                }
                            }
                        } else {
                            serde_json::Value::String(f.to_string())
                        }
                    }
                    Data::Int(i) => {
                        has_any_data = true;
                        if is_numeric_field {
                            match u32::try_from(*i) {
                                Ok(id) => serde_json::Value::Number(id.into()),
                                Err(_) => {
                                    bad_ticket_id = Some(i.to_string());
                                    serde_json::Value::Null
                                }
                            }
                        } else {
                            serde_json::Value::String(i.to_string())
                        }
                    }
                    Data::Bool(b) => {
                        has_any_data = true;
                        serde_json::Value::String(b.to_string())
                    }
                    Data::DateTime(dt) => {
                        has_any_data = true;
                        if is_date_field {
                            let serial = dt.as_f64();
                            match excel_serial_to_datetime(serial) {
                                Some(ndt) => {
                                    let date_str = ndt.format("%Y-%m-%dT%H:%M:%S").to_string();
                                    serde_json::Value::String(date_str)
                                }
                                None => {
                                    tracing::warn!(
                                        "Failed to convert Excel DateTime serial {} to NaiveDateTime",
                                        serial
                                    );
                                    serde_json::Value::Null
                                }
                            }
                        } else {
                            serde_json::Value::String(dt.to_string())
                        }
                    }
                    Data::DateTimeIso(dt) => {
                        has_any_data = true;
                        if is_date_field {
                            let dt_clean = dt.trim_end_matches('Z').trim_end_matches('z');
                            match NaiveDateTime::parse_from_str(dt_clean, "%Y-%m-%dT%H:%M:%S%.f") {
                                Ok(ndt) => serde_json::Value::String(
                                    ndt.format("%Y-%m-%dT%H:%M:%S").to_string(),
                                ),
                                Err(_) => {
                                    match NaiveDateTime::parse_from_str(
                                        dt_clean,
                                        "%Y-%m-%dT%H:%M:%S",
                                    ) {
                                        Ok(ndt) => serde_json::Value::String(
                                            ndt.format("%Y-%m-%dT%H:%M:%S").to_string(),
                                        ),
                                        Err(_) => {
                                            match NaiveDateTime::parse_from_str(
                                                dt_clean,
                                                "%Y-%m-%d %H:%M:%S",
                                            ) {
                                                Ok(ndt) => serde_json::Value::String(
                                                    ndt.format("%Y-%m-%dT%H:%M:%S").to_string(),
                                                ),
                                                Err(_) => {
                                                    serde_json::Value::String(dt_clean.to_string())
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        } else {
                            serde_json::Value::String(dt.clone())
                        }
                    }
                    Data::DurationIso(d) => {
                        has_any_data = true;
                        serde_json::Value::String(d.clone())
                    }
                    Data::Error(e) => {
                        has_any_data = true;
                        serde_json::Value::String(format!("{:?}", e))
                    }
                }
            } else if is_date_field {
                serde_json::Value::Null
            } else {
                serde_json::Value::String(String::new())
            };
            let normalized_header = if is_date_field {
                "actionDate".to_string()
            } else {
                header.clone()
            };
            record.insert(normalized_header, cell_value);
        }
        self.row_num += 1;
        if let Some(value) = bad_ticket_id {
            return Some(Err(anyhow::anyhow!(
                "ticket id {} in row {} of worksheet '{}' of excel file '{}' is not a whole number from 0 to {}",
                value,
                row_num_for_error,
                self.sheet_name,
                self.file_name,
                u32::MAX
            )));
        }
        if !has_any_data {
            return self.next();
        }
        let json_value = serde_json::Value::Object(record.clone());
        let available_fields: Vec<String> = record.keys().cloned().collect();
        match serde_json::from_value::<ActionObject>(json_value.clone()) {
            Ok(action_object) => {
                if action_object.actiondate.is_none() {
                    tracing::warn!(
                        "Date field is None after deserialization. Available fields: {:?}",
                        available_fields
                    );
                    if let Some(date_val) = record.get("actionDate") {
                        tracing::warn!("Date value in JSON was: {:?}", date_val);
                    }
                }
                Some(Ok(action_object))
            }
            Err(e) => {
                let json_str = serde_json::to_string(&json_value)
                    .unwrap_or_else(|_| "failed to serialize".to_string());
                let error_str = e.to_string();
                let field_info = format!(
                    "Available fields: [{}]. Error: {}",
                    available_fields.join(", "),
                    error_str
                );
                Some(Err(anyhow::anyhow!(
                    "failed to deserialize row {} in worksheet '{}' of excel file '{}': {} (data: {})",
                    row_num_for_error,
                    self.sheet_name,
                    self.file_name,
                    field_info,
                    json_str
                )))
            }
        }
    }
}

impl Excel for Reader {
    fn try_excel_to_action_objects(path: &Path) -> anyhow::Result<Vec<ActionObject>> {
        let iter = Self::excel_action_iter(path)?;
        let mut output = Vec::new();
        for result in iter {
            output.push(result?);
        }
        Ok(output)
    }

    fn excel_action_iter(path: &Path) -> anyhow::Result<ExcelActionIterator> {
        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown file")
            .to_string();
        let mut workbook = open_workbook_auto(path)
            .with_context(|| format!("failed to open excel file: {}", file_name))?;
        let sheet_names = workbook.sheet_names().to_owned();
        let first_sheet_name = sheet_names
            .first()
            .ok_or_else(|| anyhow::anyhow!("excel file '{}' has no worksheets", file_name))?
            .clone();
        let range = workbook
            .worksheet_range(&first_sheet_name)
            .with_context(|| {
                format!(
                    "failed to read worksheet '{}' from excel file: {}",
                    first_sheet_name, file_name
                )
            })?;
        let mut rows_iter = range.rows();
        let headers: Vec<String> = match rows_iter.next() {
            Some(header_row) => header_row.iter().map(|cell| cell.to_string()).collect(),
            None => {
                return Err(anyhow::anyhow!(
                    "first worksheet '{}' of excel file '{}' has no header row",
                    first_sheet_name,
                    file_name
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
        Ok(ExcelActionIterator {
            rows,
            headers,
            file_name,
            sheet_name: first_sheet_name,
            row_num: 0,
        })
    }
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

    fn sheet(headers: &[&str], rows: Vec<Vec<Data>>) -> ExcelActionIterator {
        ExcelActionIterator {
            rows,
            headers: headers.iter().map(|h| (*h).to_string()).collect(),
            file_name: "book.xlsx".to_string(),
            sheet_name: "Sheet1".to_string(),
            row_num: 0,
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
}
