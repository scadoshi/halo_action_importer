use crate::{domain::models::action_object::ActionObject, inbound::file::Reader};
use anyhow::Context;
use calamine::{Data, Reader as CalamineReader, open_workbook_auto};
use chrono::{NaiveDate, NaiveDateTime, NaiveTime};
use std::path::Path;

fn excel_serial_to_datetime(serial: f64) -> Option<NaiveDateTime> {
    let excel_epoch = NaiveDate::from_ymd_opt(1899, 12, 30)?;
    let days = serial.floor() as i64;
    let seconds_in_day = (serial.fract() * 86400.0).floor() as u32;
    let time = NaiveTime::from_num_seconds_from_midnight_opt(seconds_in_day, 0)?;
    let date = excel_epoch.checked_add_signed(chrono::Duration::days(days))?;
    Some(NaiveDateTime::new(date, time))
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
                                serde_json::Value::String(s.clone())
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
                            serde_json::Value::Number((*f as u32).into())
                        } else {
                            serde_json::Value::String(f.to_string())
                        }
                    }
                    Data::Int(i) => {
                        has_any_data = true;
                        if is_numeric_field {
                            serde_json::Value::Number((*i as u32).into())
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
                            let dt_str = dt.to_string();
                            if let Ok(serial) = dt_str.parse::<f64>() {
                                match excel_serial_to_datetime(serial) {
                                    Some(ndt) => serde_json::Value::String(
                                        ndt.format("%Y-%m-%dT%H:%M:%S").to_string(),
                                    ),
                                    None => serde_json::Value::Null,
                                }
                            } else {
                                serde_json::Value::Null
                            }
                        } else {
                            serde_json::Value::String(dt.to_string())
                        }
                    }
                    Data::DateTimeIso(dt) => {
                        has_any_data = true;
                        if is_date_field {
                            match NaiveDateTime::parse_from_str(dt, "%Y-%m-%dT%H:%M:%S%.f") {
                                Ok(ndt) => serde_json::Value::String(
                                    ndt.format("%Y-%m-%d %H:%M:%S").to_string(),
                                ),
                                Err(_) => {
                                    match NaiveDateTime::parse_from_str(dt, "%Y-%m-%d %H:%M:%S") {
                                        Ok(ndt) => serde_json::Value::String(
                                            ndt.format("%Y-%m-%d %H:%M:%S").to_string(),
                                        ),
                                        Err(_) => serde_json::Value::String(dt.clone()),
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
            } else {
                if is_date_field {
                    serde_json::Value::Null
                } else {
                    serde_json::Value::String(String::new())
                }
            };
            record.insert(header.clone(), cell_value);
        }
        self.row_num += 1;
        if !has_any_data {
            return self.next();
        }
        let json_value = serde_json::Value::Object(record.clone());
        let available_fields: Vec<String> = record.keys().cloned().collect();
        match serde_json::from_value::<ActionObject>(json_value.clone()) {
            Ok(action_object) => Some(Ok(action_object)),
            Err(e) => {
                let json_str = serde_json::to_string(&json_value)
                    .unwrap_or_else(|_| "failed to serialize".to_string());
                let error_str = e.to_string();
                let field_info = if error_str.contains("missing field") {
                    format!(
                        "Available fields: [{}]. Error: {}",
                        available_fields.join(", "),
                        error_str
                    )
                } else {
                    format!(
                        "Available fields: [{}]. Error: {}",
                        available_fields.join(", "),
                        error_str
                    )
                };
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
        let mut iter = Self::excel_action_iter(path)?;
        let mut output = Vec::new();
        while let Some(result) = iter.next() {
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
