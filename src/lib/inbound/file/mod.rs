pub mod csv;
pub mod excel;

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
