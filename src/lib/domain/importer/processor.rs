use std::collections::{BTreeMap, HashMap, HashSet};
use std::ffi::OsStr;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::time::Instant;

use chrono::Utc;
use csv::Writer;
use serde::Serialize;
use tracing::{error, info, warn};

use super::format::{format_duration, format_eta, format_number};
use crate::domain::importer::setup::Cache;
use crate::domain::models::action_object::{ActionId, ActionObject};
use crate::inbound::file::csv::Csv;
use crate::inbound::file::excel::Excel;
use crate::inbound::file::{Reader, RowError};
use crate::outbound::client::action::{ActionClient, PostError};

pub struct FailedAction {
    pub action: ActionObject,
    pub error_type: String,
}

pub struct ProcessingStats {
    pub processed: usize,
    pub imported: usize,
    pub skipped: usize,
    /// The skips that were for a ticket Halo answered 404 for; the rest were
    /// ids already in Halo.
    pub skipped_missing_ticket: usize,
    pub failed: Vec<FailedAction>,
}

struct TicketGroupedResult {
    missing_tickets: HashSet<u32>,
    imported_actions: Vec<ActionObject>,
    failed_actions: Vec<FailedAction>,
}

/// Efficiently identify missing tickets and import valid actions.
/// Strategy: Group actions by ticket_id and retry each ticket group, in ticket order.
/// When a ticket group fails, all actions for that ticket are marked as failed.
async fn retry_by_ticket_groups(
    client: &ActionClient,
    batch: &[ActionObject],
) -> TicketGroupedResult {
    let mut result = TicketGroupedResult {
        missing_tickets: HashSet::new(),
        imported_actions: Vec::new(),
        failed_actions: Vec::new(),
    };

    // Group actions by ticket_id
    let mut ticket_groups: BTreeMap<u32, Vec<ActionObject>> = BTreeMap::new();
    for action in batch {
        ticket_groups
            .entry(action.ticket_id)
            .or_default()
            .push(action.clone());
    }

    info!(
        "Batch failed with 'not found' error - retrying {} ticket groups",
        ticket_groups.len()
    );

    // Try each ticket group
    for (ticket_id, actions) in ticket_groups {
        match client.post_action_objects(actions.clone()).await {
            Ok(()) => {
                // All actions for this ticket imported successfully
                result.imported_actions.extend(actions);
            }
            Err(PostError::TicketNotFound { .. }) => {
                // This ticket doesn't exist, mark all its actions as failed
                result.missing_tickets.insert(ticket_id);
                warn!(
                    "Ticket ID: {} not found - marking {} action(s) as failed",
                    ticket_id,
                    actions.len()
                );
                for action in actions {
                    result.failed_actions.push(FailedAction {
                        action,
                        error_type: "Ticket not found".to_string(),
                    });
                }
            }
            Err(e) => {
                // Other error type, mark all actions in this group as failed
                error!(
                    "Failed to import {} action(s) for ticket {}: {}",
                    actions.len(),
                    ticket_id,
                    e
                );
                for action in actions {
                    result.failed_actions.push(FailedAction {
                        action,
                        error_type: e.error_type().to_string(),
                    });
                }
            }
        }
    }

    info!(
        "Ticket group retry complete: recovered {}/{} actions, identified {} missing ticket(s)",
        result.imported_actions.len(),
        batch.len(),
        result.missing_tickets.len()
    );

    result
}

/// What a batch Halo did not take comes to.
struct FailedBatch {
    /// Actions a ticket by ticket retry got in after all.
    imported: Vec<ActionObject>,
    failed: Vec<FailedAction>,
    /// Whether the batch was posted again ticket by ticket.
    regrouped: bool,
}

/// Settles a batch whose post failed with `error`. A 404 marks its tickets
/// missing; a 404 on a batch of more than one action posts it again ticket by
/// ticket first, so one missing ticket fails only its own actions. Every other
/// error fails the whole batch. `label` names the batch in the log.
async fn settle_failed_batch(
    client: &ActionClient,
    batch: &[ActionObject],
    error: PostError,
    missing_tickets: &mut HashSet<u32>,
    label: &str,
) -> FailedBatch {
    let not_found = matches!(error, PostError::TicketNotFound { .. });

    if not_found && batch.len() > 1 {
        // Use ticket-grouped retry to identify missing tickets
        warn!(
            "{} of {} actions failed with 'not found' error - retrying by ticket groups",
            label,
            batch.len()
        );

        let search_result = retry_by_ticket_groups(client, batch).await;

        // Add identified missing tickets to skip set
        for ticket_id in &search_result.missing_tickets {
            missing_tickets.insert(*ticket_id);
            warn!(
                "Ticket ID: {} not found - will skip future actions for this ticket",
                ticket_id
            );
        }

        for failed_action in &search_result.failed_actions {
            error!(
                "Failed to import action ID: {} (ticket ID: {})",
                failed_action.action.action_id(),
                failed_action.action.ticket_id
            );
        }

        info!(
            "Ticket group retry complete: recovered {}/{} actions, identified {} missing ticket(s)",
            search_result.imported_actions.len(),
            batch.len(),
            search_result.missing_tickets.len()
        );

        return FailedBatch {
            imported: search_result.imported_actions,
            failed: search_result.failed_actions,
            regrouped: true,
        };
    }

    // Not a "not found" error, or batch size is 1
    let mut failed = Vec::new();
    for action in batch {
        if not_found {
            missing_tickets.insert(action.ticket_id);
            warn!(
                "Ticket ID: {} not found - will skip future actions for this ticket",
                action.ticket_id
            );
        }
        error!(
            "Failed to import action ID: {} (ticket ID: {}): {}",
            action.action_id(),
            action.ticket_id,
            error
        );
        failed.push(FailedAction {
            action: action.clone(),
            error_type: error.error_type().to_string(),
        });
    }
    FailedBatch {
        imported: Vec::new(),
        failed,
        regrouped: false,
    }
}

/// Records `actions` as imported: their ids count as existing for the rest of
/// the run and are appended to the cache straight away.
fn record_imported(cache: &Cache, existing_ids: &mut HashSet<ActionId>, actions: &[ActionObject]) {
    existing_ids.extend(actions.iter().map(ActionObject::action_id));
    let ids: Vec<String> = actions.iter().map(|a| a.action_id().to_string()).collect();
    if let Err(e) = cache.record_imported(&ids) {
        warn!("Failed to update cache with imported IDs: {}", e);
    }
}

/// Settings for a run, the same for every file.
pub struct ProcessConfig<'a> {
    /// Posts the actions. `None` in parse-only mode.
    pub action_client: Option<&'a ActionClient>,
    /// Where each posted id is recorded as it succeeds.
    pub cache: &'a Cache,
    /// Read and count the rows without posting them.
    pub only_parse: bool,
    /// How many actions go in one post.
    pub batch_size: usize,
}

/// What a run over every file came to.
pub struct RunTotals {
    pub processed: usize,
    pub imported: usize,
    pub skipped: usize,
    /// The skips that were for a ticket Halo answered 404 for; the rest were
    /// ids already in Halo.
    pub skipped_missing_ticket: usize,
    pub failed: Vec<FailedAction>,
    /// Each file that could not be read, with the reason.
    pub skipped_files: Vec<String>,
    /// How long each file took, in seconds.
    pub sheet_times: Vec<f64>,
}

/// One run over a list of files. It carries what each file leaves for the
/// next: the ids already in Halo, which grow as posts succeed, and how long
/// each file took.
pub struct Run<'a> {
    config: ProcessConfig<'a>,
    /// Ids already in Halo, from the cache plus every id this run has posted.
    existing_ids: HashSet<ActionId>,
    sheet_times: Vec<f64>,
    total_sheets: usize,
}

/// One file's progress while its rows are read and posted.
struct Sheet {
    total_rows: usize,
    processed: usize,
    imported: usize,
    skipped: usize,
    skipped_missing_ticket: usize,
    failed: Vec<FailedAction>,
    /// Tickets Halo answered 404 for; their later actions are skipped.
    missing_tickets: HashSet<u32>,
    /// Seconds per action of each post, for the ETA.
    row_times: Vec<f64>,
    /// Skips not yet logged.
    pending_skips: usize,
    batch: Vec<ActionObject>,
    batch_start: Instant,
    batch_number: usize,
}

impl Sheet {
    /// Logs the skips counted since the last time, if any.
    fn log_pending_skips(&mut self) {
        if self.pending_skips > 0 {
            info!(
                "Skipped {} entries (already exist)",
                format_number(self.pending_skips)
            );
            self.pending_skips = 0;
        }
    }
}

impl<'a> Run<'a> {
    /// A run of `total_sheets` files that starts out knowing `existing_ids`.
    pub fn new(
        config: ProcessConfig<'a>,
        existing_ids: HashSet<ActionId>,
        total_sheets: usize,
    ) -> Self {
        Self {
            config,
            existing_ids,
            sheet_times: Vec::new(),
            total_sheets,
        }
    }

    /// Imports each `(path, file name)` in turn. A file that cannot be read is
    /// logged and listed in [`RunTotals::skipped_files`], and the run goes on.
    pub async fn process_files(mut self, files: &[(PathBuf, String)]) -> RunTotals {
        let mut totals = RunTotals {
            processed: 0,
            imported: 0,
            skipped: 0,
            skipped_missing_ticket: 0,
            failed: Vec::new(),
            skipped_files: Vec::new(),
            sheet_times: Vec::new(),
        };
        for (index, (path, file_name)) in files.iter().enumerate() {
            match self.process_file(path, file_name, index + 1).await {
                Ok(stats) => {
                    totals.processed += stats.processed;
                    totals.imported += stats.imported;
                    totals.skipped += stats.skipped;
                    totals.skipped_missing_ticket += stats.skipped_missing_ticket;
                    totals.failed.extend(stats.failed);
                }
                Err(e) => {
                    error!("Failed to read file {:?}: {}", file_name, e);
                    totals.skipped_files.push(format!("{:?}: {}", file_name, e));
                }
            }
        }
        totals.sheet_times = self.sheet_times;
        totals
    }

    /// Imports one CSV or Excel file, picked by its extension, as file number
    /// `sheet_number` of the run.
    ///
    /// # Errors
    ///
    /// When the file cannot be opened or read as a whole, or is neither CSV
    /// nor Excel. A row that does not read is logged and the file goes on.
    pub async fn process_file(
        &mut self,
        path: &Path,
        file_name: &str,
        sheet_number: usize,
    ) -> anyhow::Result<ProcessingStats> {
        let extension = path
            .extension()
            .and_then(OsStr::to_str)
            .map(str::to_lowercase);
        match extension.as_deref() {
            Some("csv") => {
                let rows = <Reader as Csv>::csv_action_iter(path)?;
                let total_rows = rows.total_rows();
                let source = format!("CSV file '{file_name}'");
                Ok(self
                    .process_actions(&source, rows, total_rows, sheet_number)
                    .await)
            }
            Some("xlsx" | "xls") => {
                let rows = <Reader as Excel>::excel_action_iter(path)?;
                let total_rows = rows.total_rows();
                let source = format!("Excel file '{file_name}', sheet '{}'", rows.sheet_name());
                Ok(self
                    .process_actions(&source, rows, total_rows, sheet_number)
                    .await)
            }
            _ => anyhow::bail!("not a CSV or Excel file"),
        }
    }

    /// Posts `rows` in batches, skipping ids already in Halo and actions of
    /// tickets found missing. `source` names the file in the log.
    async fn process_actions(
        &mut self,
        source: &str,
        rows: impl Iterator<Item = Result<ActionObject, RowError>>,
        total_rows: usize,
        sheet_number: usize,
    ) -> ProcessingStats {
        let mut sheet = Sheet {
            total_rows,
            processed: 0,
            imported: 0,
            skipped: 0,
            skipped_missing_ticket: 0,
            failed: Vec::new(),
            missing_tickets: HashSet::new(),
            row_times: Vec::new(),
            pending_skips: 0,
            batch: Vec::new(),
            batch_start: Instant::now(),
            batch_number: 0,
        };
        let sheet_start = Instant::now();
        info!(
            "Processing sheet {} of {}: {} ({} rows)",
            sheet_number,
            self.total_sheets,
            source,
            format_number(total_rows)
        );
        for row in rows {
            let action = match row {
                Ok(action) => action,
                Err(e) => {
                    sheet.log_pending_skips();
                    error!("Failed to deserialize row in {}: {}", source, e);
                    // Don't add to failed vec - no ActionObject to retry
                    continue;
                }
            };
            sheet.processed += 1;
            let action_id = action.action_id();
            if self.config.only_parse {
                // Counted as a real run would: a repeated id is skipped.
                if self.existing_ids.insert(action_id) {
                    sheet.imported += 1;
                } else {
                    sheet.skipped += 1;
                }
            } else if self.existing_ids.contains(&action_id)
                || sheet
                    .batch
                    .iter()
                    .any(|queued| queued.action_id() == action_id)
            {
                sheet.skipped += 1;
                sheet.pending_skips += 1;
            } else if sheet.missing_tickets.contains(&action.ticket_id) {
                sheet.skipped += 1;
                sheet.skipped_missing_ticket += 1;
                sheet.pending_skips += 1;
            } else {
                sheet.batch.push(action);
                if sheet.batch.len() >= self.config.batch_size {
                    let remaining_rows = total_rows.saturating_sub(sheet.processed);
                    self.post_batch(&mut sheet, "Batch", remaining_rows).await;
                }
            }
        }
        if !sheet.batch.is_empty() {
            self.post_batch(&mut sheet, "Final batch", 0).await;
        }
        sheet.log_pending_skips();
        let sheet_duration = sheet_start.elapsed().as_secs_f64();
        self.sheet_times.push(sheet_duration);
        let avg_sheet_time = self.sheet_times.iter().sum::<f64>() / self.sheet_times.len() as f64;
        info!(
            "Completed sheet {} of {}: {} | {} processed, {} imported, {} skipped in {:.1}s | avg sheet time: {:.1}s",
            sheet_number,
            self.total_sheets,
            source,
            format_number(sheet.processed),
            format_number(sheet.imported),
            format_number(sheet.skipped),
            sheet_duration,
            avg_sheet_time
        );
        ProcessingStats {
            processed: sheet.processed,
            imported: sheet.imported,
            skipped: sheet.skipped,
            skipped_missing_ticket: sheet.skipped_missing_ticket,
            failed: sheet.failed,
        }
    }

    /// Posts the sheet's queued batch and empties it. `label` names the batch
    /// if it fails; `remaining_rows` feeds the ETA.
    async fn post_batch(&mut self, sheet: &mut Sheet, label: &str, remaining_rows: usize) {
        sheet.log_pending_skips();
        let batch = std::mem::take(&mut sheet.batch);
        let Some(client) = self.config.action_client else {
            for action in batch {
                error!(
                    "Action client not available for action ID: {} (ticket ID: {})",
                    action.action_id(),
                    action.ticket_id
                );
                sheet.failed.push(FailedAction {
                    action,
                    error_type: "Client unavailable".to_string(),
                });
            }
            sheet.batch_start = Instant::now();
            return;
        };
        match client.post_action_objects(batch.clone()).await {
            Ok(()) => {
                let batch_count = batch.len();
                sheet.imported += batch_count;
                sheet.batch_number += 1;

                let batch_time = sheet.batch_start.elapsed().as_secs_f64();
                sheet.row_times.push(batch_time / batch_count as f64);
                let avg_row_time =
                    sheet.row_times.iter().sum::<f64>() / sheet.row_times.len() as f64;

                if let [action] = batch.as_slice()
                    && self.config.batch_size == 1
                {
                    info!(
                        "Imported action {}/{} (ID: {}, ticket: {}) | {} total skipped | {:.2}s/row | ETA: {}",
                        sheet.imported,
                        sheet.total_rows,
                        action.action_id(),
                        action.ticket_id,
                        format_number(sheet.skipped),
                        avg_row_time,
                        format_eta(remaining_rows, avg_row_time)
                    );
                } else {
                    let mut ticket_ids: Vec<u32> = batch.iter().map(|a| a.ticket_id).collect();
                    ticket_ids.sort_unstable();
                    ticket_ids.dedup();
                    let ticket_ids = ticket_ids
                        .iter()
                        .map(u32::to_string)
                        .collect::<Vec<_>>()
                        .join(", ");
                    let total_batches =
                        (sheet.total_rows as f64 / self.config.batch_size as f64).ceil() as usize;
                    info!(
                        "Imported batch {}/{} | {}/{} actions (tickets: {}) | {} total skipped | {:.2}s/batch, {:.2}s/action | ETA: {}",
                        sheet.batch_number,
                        total_batches,
                        sheet.imported,
                        sheet.total_rows,
                        ticket_ids,
                        format_number(sheet.skipped),
                        batch_time,
                        avg_row_time,
                        format_eta(remaining_rows, avg_row_time)
                    );
                }

                record_imported(self.config.cache, &mut self.existing_ids, &batch);
            }
            Err(e) => {
                let outcome =
                    settle_failed_batch(client, &batch, e, &mut sheet.missing_tickets, label).await;
                if outcome.regrouped {
                    sheet.batch_number += 1;
                }
                let recovered = outcome.imported.len();
                sheet.imported += recovered;
                if recovered > 0 {
                    record_imported(self.config.cache, &mut self.existing_ids, &outcome.imported);
                    let avg_action_time =
                        sheet.batch_start.elapsed().as_secs_f64() / batch.len() as f64;
                    sheet
                        .row_times
                        .extend(std::iter::repeat_n(avg_action_time, recovered));
                }
                sheet.failed.extend(outcome.failed);
            }
        }
        sheet.batch_start = Instant::now();
    }
}

pub fn write_retry_csv(log_dir: &str, failed: &[FailedAction]) -> anyhow::Result<()> {
    let csv_path = format!("{}/retry.csv", log_dir);
    let file = File::create(&csv_path)?;
    let mut writer = Writer::from_writer(file);

    // Write header
    writer.write_record([
        "ticket_id",
        "action_id",
        "actiondate",
        "actionwho",
        "note",
        "outcome",
        "error_type",
    ])?;

    // Write each failed action
    for failed_action in failed {
        let action = &failed_action.action;

        // Format date for CSV (keep in Arizona time as input expects)
        let date_str = action
            .actiondate
            .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
            .unwrap_or_default();

        writer.write_record(&[
            action.ticket_id.to_string(),
            action.action_id().to_string(),
            date_str,
            action.actionwho.clone(),
            action.note.clone(),
            action.outcome.clone(),
            failed_action.error_type.clone(),
        ])?;
    }

    writer.flush()?;
    info!("Wrote {} failed actions to {}", failed.len(), csv_path);
    Ok(())
}

#[derive(Serialize)]
struct SummaryJson {
    timestamp: String,
    runtime_seconds: f64,
    runtime_formatted: String,
    total_processed: usize,
    total_imported: usize,
    total_skipped: usize,
    total_failed: usize,
    skipped_breakdown: SkippedBreakdown,
    performance: Performance,
    files_processed: usize,
    files_skipped: usize,
    error_summary: Vec<ErrorTypeSummary>,
}

#[derive(Serialize)]
struct SkippedBreakdown {
    /// Actions skipped because their id is already in Halo or came earlier in the run.
    already_exists: usize,
    /// Actions skipped because Halo answered 404 for their ticket.
    missing_ticket: usize,
}

#[derive(Serialize)]
struct Performance {
    rows_per_second: f64,
    actions_per_minute: f64,
    average_time_per_action_seconds: f64,
}

#[derive(Serialize)]
struct ErrorTypeSummary {
    error_type: String,
    count: usize,
    affected_tickets: Vec<u32>,
}

/// Writes summary.json for a run of `total_sheets` files to `log_dir`.
///
/// # Errors
///
/// When the file cannot be written.
pub fn write_summary_json(
    log_dir: &str,
    totals: &RunTotals,
    runtime_secs: f64,
    total_sheets: usize,
) -> anyhow::Result<()> {
    let RunTotals {
        processed: total_processed,
        imported: total_imported,
        skipped: total_skipped,
        skipped_missing_ticket,
        failed,
        skipped_files,
        sheet_times: _,
    } = totals;
    let (total_processed, total_imported, total_skipped) =
        (*total_processed, *total_imported, *total_skipped);
    let json_path = format!("{}/summary.json", log_dir);

    // Calculate performance metrics
    let rows_per_second = if runtime_secs > 0.0 {
        total_processed as f64 / runtime_secs
    } else {
        0.0
    };

    let actions_per_minute = rows_per_second * 60.0;

    let avg_time_per_action = if total_imported > 0 {
        runtime_secs / total_imported as f64
    } else {
        0.0
    };

    // Group errors by type
    let mut error_map: HashMap<String, Vec<u32>> = HashMap::new();
    for failed_action in failed {
        error_map
            .entry(failed_action.error_type.clone())
            .or_default()
            .push(failed_action.action.ticket_id);
    }

    let error_summary: Vec<ErrorTypeSummary> = error_map
        .into_iter()
        .map(|(error_type, mut tickets)| {
            tickets.sort_unstable();
            tickets.dedup();
            ErrorTypeSummary {
                error_type,
                count: tickets.len(),
                affected_tickets: tickets,
            }
        })
        .collect();

    // Format runtime
    let runtime_formatted = format_duration(runtime_secs);

    let summary = SummaryJson {
        timestamp: Utc::now().to_rfc3339(),
        runtime_seconds: runtime_secs,
        runtime_formatted,
        total_processed,
        total_imported,
        total_skipped,
        total_failed: failed.len(),
        skipped_breakdown: SkippedBreakdown {
            already_exists: total_skipped.saturating_sub(*skipped_missing_ticket),
            missing_ticket: *skipped_missing_ticket,
        },
        performance: Performance {
            rows_per_second,
            actions_per_minute,
            average_time_per_action_seconds: avg_time_per_action,
        },
        files_processed: total_sheets,
        files_skipped: skipped_files.len(),
        error_summary,
    };

    let json = serde_json::to_string_pretty(&summary)?;
    std::fs::write(&json_path, json)?;

    info!("Wrote summary to {}", json_path);
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use super::*;
    use crate::domain::importer::summary::{ImportSummary, exit_code};
    use crate::outbound::client::auth::AuthClient;
    use crate::test_server::{Reply, Request, TOKEN, TestServer, config, lines, scratch_dir};
    use crate::test_xlsx::{Cell, xlsx};

    /// Imports each `(name, csv)` file in turn, batch by batch, against
    /// `server`, with a scratch cache, and returns each file's stats.
    async fn import(
        server: &TestServer,
        name: &str,
        files: &[(&str, &str)],
        batch_size: usize,
    ) -> Vec<ProcessingStats> {
        import_with_cache(server, name, files, batch_size).await.0
    }

    /// [`import`], also returning what the run wrote to the imported cache.
    async fn import_with_cache(
        server: &TestServer,
        name: &str,
        files: &[(&str, &str)],
        batch_size: usize,
    ) -> (Vec<ProcessingStats>, String) {
        let dir = scratch_dir(name);
        let cache = Cache::new(dir.join("cache"));
        let config = config(server.base_url());
        let auth_client = Arc::new(AuthClient::new(config.clone()).unwrap());
        let client = ActionClient::new(config, auth_client).unwrap();
        let mut run = Run::new(
            ProcessConfig {
                action_client: Some(&client),
                cache: &cache,
                only_parse: false,
                batch_size,
            },
            HashSet::new(),
            files.len(),
        );
        let mut stats = Vec::new();
        for (number, (file_name, csv)) in files.iter().enumerate() {
            let path = dir.join(file_name);
            std::fs::write(&path, csv).unwrap();
            let file_stats =
                tokio::time::timeout(HUNG, run.process_file(&path, file_name, number + 1))
                    .await
                    .expect("the import hung")
                    .unwrap();
            stats.push(file_stats);
        }
        let cached =
            std::fs::read_to_string(dir.join("cache").join("imported")).unwrap_or_default();
        std::fs::remove_dir_all(&dir).unwrap();
        (stats, cached)
    }

    const HEADER: &str = "ticket_id,actionId,actionwho,note,actiondate\n";

    /// How long a test lets an import run before calling it hung.
    const HUNG: Duration = Duration::from_secs(5);

    const POST_TOKEN: &str = "POST /auth/token HTTP/1.1";
    const POST_ACTIONS: &str = "POST /api/actions HTTP/1.1";
    const CREATED: Reply = Reply::Json(201, "[]");

    fn error_types(stats: &ProcessingStats) -> Vec<(u64, &str)> {
        stats
            .failed
            .iter()
            .map(|f| (f.action.action_id().value(), f.error_type.as_str()))
            .collect()
    }

    #[tokio::test]
    async fn a_400_whose_body_says_not_found_does_not_mark_the_ticket_missing() {
        let server = TestServer::start(vec![
            TOKEN,
            Reply::Json(400, r#"{"error":"user not found"}"#),
            CREATED,
        ])
        .await;
        let csv = format!("{HEADER}1,10,w,n,\n1,11,w,n,\n");

        let stats = import(&server, "proc_400_not_found", &[("a.csv", &csv)], 1).await;

        assert_eq!(
            lines(&server.requests()),
            [POST_TOKEN, POST_ACTIONS, POST_ACTIONS]
        );
        assert_eq!(error_types(&stats[0]), [(10, "Validation error")]);
        assert_eq!((stats[0].imported, stats[0].skipped), (1, 0));
    }

    #[tokio::test]
    async fn a_404_marks_the_ticket_missing_and_skips_its_later_actions() {
        let server = TestServer::start(vec![TOKEN, Reply::Json(404, "{}"), CREATED]).await;
        let csv = format!("{HEADER}1,10,w,n,\n1,11,w,n,\n2,20,w,n,\n");

        let stats = import(&server, "proc_404", &[("a.csv", &csv)], 1).await;

        let requests = server.requests();
        assert_eq!(lines(&requests), [POST_TOKEN, POST_ACTIONS, POST_ACTIONS]);
        assert!(
            requests[2].body.contains(r#""cfactionid":20"#),
            "{}",
            requests[2].body
        );
        assert_eq!(error_types(&stats[0]), [(10, "Ticket not found")]);
        assert_eq!((stats[0].imported, stats[0].skipped), (1, 1));
    }

    #[tokio::test]
    async fn a_batch_answered_404_is_posted_again_ticket_by_ticket() {
        let server = TestServer::start(vec![
            TOKEN,
            Reply::Json(404, "{}"),
            Reply::Json(404, "{}"),
            CREATED,
        ])
        .await;
        let csv = format!("{HEADER}1,10,w,n,\n2,20,w,n,\n");

        let stats = import(&server, "proc_404_batch", &[("a.csv", &csv)], 2).await;

        let requests = server.requests();
        assert_eq!(
            lines(&requests),
            [POST_TOKEN, POST_ACTIONS, POST_ACTIONS, POST_ACTIONS]
        );
        assert!(
            requests[2].body.contains(r#""cfactionid":10"#),
            "{}",
            requests[2].body
        );
        assert!(
            requests[3].body.contains(r#""cfactionid":20"#),
            "{}",
            requests[3].body
        );
        assert_eq!(error_types(&stats[0]), [(10, "Ticket not found")]);
        assert_eq!(stats[0].imported, 1);
    }

    #[tokio::test]
    async fn a_post_answered_504_fails_its_batch_and_the_next_batch_still_goes() {
        let server = TestServer::start(vec![TOKEN, Reply::Json(504, "{}"), CREATED]).await;
        let csv = format!("{HEADER}1,10,w,n,\n1,11,w,n,\n");

        let stats = import(&server, "proc_504", &[("a.csv", &csv)], 1).await;

        assert_eq!(
            lines(&server.requests()),
            [POST_TOKEN, POST_ACTIONS, POST_ACTIONS]
        );
        assert_eq!(error_types(&stats[0]), [(10, "Gateway timeout")]);
        assert_eq!(stats[0].imported, 1);
    }

    #[tokio::test]
    async fn a_post_hit_by_a_network_error_fails_its_batch_as_a_network_error() {
        let server = TestServer::start(vec![TOKEN, Reply::Hangup, CREATED]).await;
        let csv = format!("{HEADER}1,10,w,n,\n1,11,w,n,\n");

        let stats = import(&server, "proc_hangup", &[("a.csv", &csv)], 1).await;

        assert_eq!(
            lines(&server.requests()),
            [POST_TOKEN, POST_ACTIONS, POST_ACTIONS]
        );
        assert_eq!(error_types(&stats[0]), [(10, "Network error")]);
        assert_eq!(stats[0].imported, 1);
    }

    #[tokio::test]
    async fn a_post_answered_401_twice_fails_its_batch_as_an_authentication_error() {
        let server = TestServer::start(vec![
            TOKEN,
            Reply::Json(401, "{}"),
            TOKEN,
            Reply::Json(401, "{}"),
        ])
        .await;
        let csv = format!("{HEADER}1,10,w,n,\n");

        let stats = import(&server, "proc_401_twice", &[("a.csv", &csv)], 1).await;

        assert_eq!(error_types(&stats[0]), [(10, "Authentication error")]);
    }

    #[tokio::test]
    async fn an_id_imported_from_one_file_is_skipped_in_a_later_file() {
        let server = TestServer::start(vec![TOKEN, CREATED]).await;
        let first = format!("{HEADER}1,10,w,n,\n");
        let second = format!("{HEADER}1,10,w,n,\n");

        let stats = import(
            &server,
            "proc_cross_file",
            &[("a.csv", &first), ("b.csv", &second)],
            1,
        )
        .await;

        assert_eq!(lines(&server.requests()), [POST_TOKEN, POST_ACTIONS]);
        assert_eq!((stats[1].imported, stats[1].skipped), (0, 1));
        assert!(stats[1].failed.is_empty());
    }

    #[tokio::test]
    async fn an_id_twice_in_one_file_is_posted_once() {
        for batch_size in [1, 2] {
            let server = TestServer::start(vec![TOKEN, CREATED]).await;
            let csv = format!("{HEADER}1,10,w,n,\n1,10,w,n,\n");

            let stats = import(&server, "proc_same_file", &[("a.csv", &csv)], batch_size).await;

            let requests = server.requests();
            assert_eq!(
                lines(&requests),
                [POST_TOKEN, POST_ACTIONS],
                "batch {batch_size}"
            );
            assert_eq!(
                requests[1].body.matches(r#""cfactionid":10"#).count(),
                1,
                "batch {batch_size}"
            );
            assert_eq!(
                (stats[0].imported, stats[0].skipped),
                (1, 1),
                "batch {batch_size}"
            );
        }
    }

    #[tokio::test]
    async fn each_successful_post_is_appended_to_the_cache() {
        let server = TestServer::start(vec![TOKEN, CREATED, Reply::Json(500, "{}"), CREATED]).await;
        let csv = format!("{HEADER}1,10,w,n,\n1,11,w,n,\n1,12,w,n,\n");

        let (stats, cached) =
            import_with_cache(&server, "proc_cache_append", &[("a.csv", &csv)], 1).await;

        assert_eq!(cached, "10\n12\n");
        assert_eq!(error_types(&stats[0]), [(11, "Server error")]);
    }

    #[tokio::test]
    async fn a_batch_whose_post_got_a_504_is_written_to_the_retry_csv() {
        let server = TestServer::start(vec![TOKEN, Reply::Json(504, "{}")]).await;
        let csv = format!("{HEADER}1,10,w,n,\n");
        let stats = import(&server, "proc_504_retry_csv", &[("a.csv", &csv)], 1).await;
        let dir = scratch_dir("proc_504_retry_csv_log");

        write_retry_csv(dir.to_str().unwrap(), &stats[0].failed).unwrap();

        let written = std::fs::read_to_string(dir.join("retry.csv")).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(lines(&server.requests()), [POST_TOKEN, POST_ACTIONS]);
        assert!(
            written.ends_with("1,10,,w,n,Imported Note,Gateway timeout\n"),
            "{written}"
        );
    }

    #[test]
    fn the_retry_csv_round_trips_through_the_importer_columns() {
        let dir = std::env::temp_dir().join(format!("halo_retry_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let date = chrono::NaiveDate::from_ymd_opt(2026, 1, 15)
            .unwrap()
            .and_hms_opt(10, 0, 0)
            .unwrap();
        let failed = vec![FailedAction {
            action: ActionObject::new(7, Some(date), None, "a, note", "who", ActionId::new(99)),
            error_type: "Server error".to_string(),
        }];

        write_retry_csv(dir.to_str().unwrap(), &failed).unwrap();

        let written = std::fs::read_to_string(dir.join("retry.csv")).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(
            written,
            "ticket_id,action_id,actiondate,actionwho,note,outcome,error_type\n\
             7,99,2026-01-15 10:00:00,who,\"a, note\",Imported Note,Server error\n"
        );
    }

    /// What a run over some files left behind, as `main` writes it.
    struct RunOutput {
        requests: Vec<Request>,
        processed: usize,
        imported: usize,
        skipped: usize,
        failed: usize,
        exit_code: u8,
        retry_csv: Option<String>,
        /// summary.json without its timing fields, which change run to run.
        summary: serde_json::Value,
    }

    /// Runs `files` through the importer the way `main` does, against
    /// `server`, writing retry.csv and summary.json to a scratch log directory.
    async fn run_files(
        server: &TestServer,
        name: &str,
        files: &[(&str, Vec<u8>)],
        batch_size: usize,
        only_parse: bool,
    ) -> RunOutput {
        let dir = scratch_dir(name);
        let log_dir = dir.join("log");
        std::fs::create_dir_all(&log_dir).unwrap();
        let log_dir_str = log_dir.to_str().unwrap();
        let cache = Cache::new(dir.join("cache"));
        let config = config(server.base_url());
        let client = (!only_parse).then(|| {
            let auth_client = Arc::new(AuthClient::new(config.clone()).unwrap());
            ActionClient::new(config, auth_client).unwrap()
        });
        let mut paths = Vec::new();
        for (file_name, bytes) in files {
            let path = dir.join(file_name);
            std::fs::write(&path, bytes).unwrap();
            paths.push((path, (*file_name).to_string()));
        }
        let run = Run::new(
            ProcessConfig {
                action_client: client.as_ref(),
                cache: &cache,
                only_parse,
                batch_size,
            },
            HashSet::new(),
            files.len(),
        );
        let totals = tokio::time::timeout(HUNG, run.process_files(&paths))
            .await
            .expect("the import hung");
        let code = exit_code(&ImportSummary {
            total_processed: totals.processed,
            total_imported: totals.imported,
            total_skipped: totals.skipped,
            total_failed: totals.failed.len(),
            skipped_files: totals.skipped_files.clone(),
            total_runtime_secs: 1.0,
            sheet_times: totals.sheet_times.clone(),
        });
        if !totals.failed.is_empty() {
            write_retry_csv(log_dir_str, &totals.failed).unwrap();
        }
        write_summary_json(log_dir_str, &totals, 1.0, files.len()).unwrap();
        let retry_csv = std::fs::read_to_string(log_dir.join("retry.csv")).ok();
        let mut summary: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(log_dir.join("summary.json")).unwrap())
                .unwrap();
        let fields = summary.as_object_mut().unwrap();
        for timing in [
            "timestamp",
            "runtime_seconds",
            "runtime_formatted",
            "performance",
        ] {
            fields.remove(timing).unwrap();
        }
        std::fs::remove_dir_all(&dir).unwrap();
        RunOutput {
            requests: server.requests(),
            processed: totals.processed,
            imported: totals.imported,
            skipped: totals.skipped,
            failed: totals.failed.len(),
            exit_code: code,
            retry_csv,
            summary,
        }
    }

    /// The action ids each request posted, in order.
    fn posted_ids(requests: &[Request]) -> Vec<Vec<u64>> {
        requests
            .iter()
            .filter(|r| r.line == POST_ACTIONS)
            .map(|r| {
                let body: serde_json::Value = serde_json::from_str(&r.body).unwrap();
                body.as_array()
                    .unwrap()
                    .iter()
                    .map(|action| action["cfactionid"].as_u64().unwrap())
                    .collect()
            })
            .collect()
    }

    /// A CSV file of `(ticket, action)` rows.
    fn csv_file(rows: &[(u32, u64)]) -> Vec<u8> {
        let mut csv = HEADER.to_string();
        for (ticket, action) in rows {
            csv.push_str(&format!("{ticket},{action},w,n,\n"));
        }
        csv.into_bytes()
    }

    /// An Excel file of `(ticket, action)` rows, the ids in numeric cells.
    fn excel_file(rows: &[(u32, u64)]) -> Vec<u8> {
        let mut sheet = vec![vec![
            Cell::Text("requestId"),
            Cell::Text("actionId"),
            Cell::Text("actionwho"),
            Cell::Text("note"),
            Cell::Text("actiondate"),
        ]];
        for (ticket, action) in rows {
            sheet.push(vec![
                Cell::Number(f64::from(*ticket)),
                Cell::Number(*action as f64),
                Cell::Text("w"),
                Cell::Text("n"),
                Cell::Empty,
            ]);
        }
        xlsx(&sheet)
    }

    /// The same rows as a CSV file and as an Excel file.
    fn both_files(rows: &[(u32, u64)]) -> [(&'static str, Vec<u8>); 2] {
        [("a.csv", csv_file(rows)), ("a.xlsx", excel_file(rows))]
    }

    const FIVE_ROWS: &[(u32, u64)] = &[(1, 10), (1, 11), (2, 20), (2, 21), (3, 30)];

    #[tokio::test]
    async fn rows_post_in_batches_of_the_batch_size() {
        for file in both_files(FIVE_ROWS) {
            let server = TestServer::start(vec![TOKEN, CREATED, CREATED, CREATED]).await;

            let run = run_files(
                &server,
                "pin_batches",
                std::slice::from_ref(&file),
                2,
                false,
            )
            .await;

            assert_eq!(
                posted_ids(&run.requests),
                [vec![10, 11], vec![20, 21], vec![30]],
                "{}",
                file.0
            );
            assert_eq!(
                (run.processed, run.imported, run.skipped, run.failed),
                (5, 5, 0, 0),
                "{}",
                file.0
            );
            assert_eq!(run.retry_csv, None, "{}", file.0);
            assert_eq!(run.exit_code, 0, "{}", file.0);
        }
    }

    #[tokio::test]
    async fn a_failed_batch_goes_to_the_retry_csv_and_the_next_batch_still_posts() {
        for file in both_files(FIVE_ROWS) {
            let server =
                TestServer::start(vec![TOKEN, CREATED, Reply::Json(500, "{}"), CREATED]).await;

            let run = run_files(
                &server,
                "pin_retry_csv",
                std::slice::from_ref(&file),
                2,
                false,
            )
            .await;

            assert_eq!(
                posted_ids(&run.requests),
                [vec![10, 11], vec![20, 21], vec![30]],
                "{}",
                file.0
            );
            assert_eq!(
                run.retry_csv.as_deref(),
                Some(
                    "ticket_id,action_id,actiondate,actionwho,note,outcome,error_type\n\
                     2,20,,w,n,Imported Note,Server error\n\
                     2,21,,w,n,Imported Note,Server error\n"
                ),
                "{}",
                file.0
            );
            assert_eq!(
                (run.imported, run.failed, run.exit_code),
                (3, 2, 1),
                "{}",
                file.0
            );
        }
    }

    #[tokio::test]
    async fn a_404_ticket_skips_its_later_actions_in_either_file_type() {
        for file in both_files(FIVE_ROWS) {
            let server = TestServer::start(vec![
                TOKEN,
                CREATED,
                CREATED,
                Reply::Json(404, "{}"),
                CREATED,
            ])
            .await;

            let run = run_files(&server, "pin_404", std::slice::from_ref(&file), 1, false).await;

            assert_eq!(
                posted_ids(&run.requests),
                [vec![10], vec![11], vec![20], vec![30]],
                "{}",
                file.0
            );
            assert_eq!(
                (run.imported, run.skipped, run.failed),
                (3, 1, 1),
                "{}",
                file.0
            );
            assert!(
                run.retry_csv
                    .as_deref()
                    .unwrap()
                    .ends_with("2,20,,w,n,Imported Note,Ticket not found\n"),
                "{}: {:?}",
                file.0,
                run.retry_csv
            );
        }
    }

    #[tokio::test]
    async fn parse_only_posts_nothing_and_counts_repeats_as_skipped() {
        let rows = &[(1, 10), (1, 11), (1, 10)];
        for file in both_files(rows) {
            let server = TestServer::start(Vec::new()).await;

            let run = run_files(
                &server,
                "pin_parse_only",
                std::slice::from_ref(&file),
                2,
                true,
            )
            .await;

            assert!(run.requests.is_empty(), "{}: {:?}", file.0, run.requests);
            assert_eq!(
                (run.processed, run.imported, run.skipped, run.failed),
                (3, 2, 1, 0),
                "{}",
                file.0
            );
            assert_eq!(run.exit_code, 0, "{}", file.0);
        }
    }

    #[tokio::test]
    async fn the_summary_json_counts_a_mixed_run() {
        // Ticket 2 is missing, ticket 3's posts fail, 10 repeats, one row
        // does not parse and the Excel file is not a workbook.
        let csv = "ticket_id,actionId,actionwho,note,actiondate\n\
                   1,10,w,n,\n\
                   1,11,w,n,\n\
                   1,10,w,n,\n\
                   2,20,w,n,\n\
                   2,21,w,n,\n\
                   2,22,w,n,\n\
                   3,30,w,n,\n\
                   3,30,w,n,\n\
                   nope,40,w,n,\n";
        let server = TestServer::start(vec![
            TOKEN,
            CREATED,
            CREATED,
            Reply::Json(404, "{}"),
            Reply::Json(500, "{}"),
            Reply::Json(500, "{}"),
        ])
        .await;

        let mut run = run_files(
            &server,
            "pin_summary",
            &[
                ("a.csv", csv.as_bytes().to_vec()),
                ("broken.xlsx", b"not a workbook".to_vec()),
            ],
            1,
            false,
        )
        .await;

        assert_eq!(
            posted_ids(&run.requests),
            [vec![10], vec![11], vec![20], vec![30], vec![30]]
        );
        // The error summary comes out in no fixed order.
        run.summary["error_summary"]
            .as_array_mut()
            .unwrap()
            .sort_by_key(|entry| entry["error_type"].as_str().unwrap().to_string());
        assert_eq!(
            run.summary,
            serde_json::json!({
                "total_processed": 8,
                "total_imported": 2,
                "total_skipped": 3,
                "total_failed": 3,
                "skipped_breakdown": { "already_exists": 1, "missing_ticket": 2 },
                "files_processed": 2,
                "files_skipped": 1,
                "error_summary": [
                    { "error_type": "Server error", "count": 1, "affected_tickets": [3] },
                    { "error_type": "Ticket not found", "count": 1, "affected_tickets": [2] },
                ],
            })
        );
        assert_eq!(run.exit_code, 1);
    }

    thread_local! {
        /// What this thread has logged since [`capture_logs`], if it was called.
        static LOGS: std::cell::RefCell<Option<Vec<u8>>> = const { std::cell::RefCell::new(None) };
    }

    /// Writes log lines into [`LOGS`] for the thread that logged them.
    struct ThreadLogs;

    impl std::io::Write for ThreadLogs {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            LOGS.with_borrow_mut(|logs| {
                if let Some(logs) = logs {
                    logs.extend_from_slice(buf);
                }
            });
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// Starts collecting what this thread logs. One global subscriber serves
    /// every test, since tracing caches which callsites a thread-local one
    /// wants and other test threads race it.
    fn capture_logs() {
        static INSTALL: std::sync::Once = std::sync::Once::new();
        INSTALL.call_once(|| {
            let subscriber = tracing_subscriber::fmt()
                .with_ansi(false)
                .without_time()
                .with_writer(|| ThreadLogs)
                .finish();
            tracing::subscriber::set_global_default(subscriber).unwrap();
        });
        LOGS.with_borrow_mut(|logs| *logs = Some(Vec::new()));
    }

    /// What this thread logged since [`capture_logs`].
    fn captured_logs() -> String {
        LOGS.with_borrow_mut(|logs| String::from_utf8(logs.take().unwrap_or_default()).unwrap())
    }

    #[tokio::test]
    async fn batches_are_numbered_from_one_in_the_log_in_either_file_type() {
        for file in both_files(FIVE_ROWS) {
            let server = TestServer::start(vec![TOKEN, CREATED, CREATED, CREATED]).await;
            capture_logs();

            run_files(
                &server,
                "batch_numbers",
                std::slice::from_ref(&file),
                2,
                false,
            )
            .await;

            let text = captured_logs();
            let numbers: Vec<&str> = text
                .lines()
                .filter_map(|line| line.split("Imported batch ").nth(1))
                .filter_map(|rest| rest.split(' ').next())
                .collect();
            assert_eq!(numbers, ["1/3", "2/3", "3/3"], "{}", file.0);
        }
    }

    #[tokio::test]
    async fn already_exists_counts_the_ids_skipped_as_already_in_halo() {
        // 10 repeats; ticket 2 is missing, so 21 and 22 are skipped for that.
        let rows = &[(1, 10), (1, 10), (2, 20), (2, 21), (2, 22)];
        let server = TestServer::start(vec![TOKEN, CREATED, Reply::Json(404, "{}")]).await;

        let run = run_files(
            &server,
            "sum_exists",
            &[("a.csv", csv_file(rows))],
            1,
            false,
        )
        .await;

        assert_eq!(run.summary["skipped_breakdown"]["already_exists"], 1);
    }

    #[tokio::test]
    async fn missing_ticket_counts_the_actions_skipped_for_a_missing_ticket() {
        let rows = &[(1, 10), (1, 10), (2, 20), (2, 21), (2, 22)];
        let server = TestServer::start(vec![TOKEN, CREATED, Reply::Json(404, "{}")]).await;

        let run = run_files(
            &server,
            "sum_missing",
            &[("a.csv", csv_file(rows))],
            1,
            false,
        )
        .await;

        assert_eq!(run.summary["skipped_breakdown"]["missing_ticket"], 2);
    }
}
