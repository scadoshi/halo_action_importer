//! Builds small .xlsx workbooks for tests: one worksheet, cells written inline.

use std::io::{Cursor, Write};

use zip::ZipWriter;
use zip::write::SimpleFileOptions;

/// One cell of a test worksheet.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Cell<'a> {
    Text(&'a str),
    Number(f64),
    Empty,
}

/// A workbook whose only sheet, `Sheet1`, holds `rows`, the first being the
/// header.
pub(crate) fn xlsx(rows: &[Vec<Cell<'_>>]) -> Vec<u8> {
    let mut sheet = String::from(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData>"#,
    );
    for (r, row) in rows.iter().enumerate() {
        let number = r + 1;
        sheet.push_str(&format!(r#"<row r="{number}">"#));
        for (c, cell) in row.iter().enumerate() {
            let column = char::from(b'A' + u8::try_from(c).unwrap());
            let reference = format!("{column}{number}");
            match cell {
                Cell::Text(text) => sheet.push_str(&format!(
                    r#"<c r="{reference}" t="inlineStr"><is><t>{}</t></is></c>"#,
                    escape(text)
                )),
                Cell::Number(value) => {
                    sheet.push_str(&format!(r#"<c r="{reference}"><v>{value}</v></c>"#));
                }
                Cell::Empty => {}
            }
        }
        sheet.push_str("</row>");
    }
    sheet.push_str("</sheetData></worksheet>");

    let parts = [
        (
            "[Content_Types].xml",
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/></Types>"#.to_string(),
        ),
        (
            "_rels/.rels",
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#.to_string(),
        ),
        (
            "xl/workbook.xml",
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="Sheet1" sheetId="1" r:id="rId1"/></sheets></workbook>"#.to_string(),
        ),
        (
            "xl/_rels/workbook.xml.rels",
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/></Relationships>"#.to_string(),
        ),
        ("xl/worksheets/sheet1.xml", sheet),
    ];
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (name, contents) in parts {
        zip.start_file(name, options).unwrap();
        zip.write_all(contents.as_bytes()).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

/// `text` with the characters XML reserves escaped.
fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}
