//! Setup, the run over the input files, and what a run writes at the end.

pub mod format;
pub mod processor;
pub mod setup;
pub mod summary;

pub use processor::{FailedAction, ProcessConfig, ProcessingStats, Run, RunTotals, UnparsedRow};
pub use setup::{Cache, SetupOptions, SetupResult};
pub use summary::ImportSummary;
