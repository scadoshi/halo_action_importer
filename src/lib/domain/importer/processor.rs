use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::File;
use std::path::Path;
use std::time::Instant;

use chrono::Utc;
use csv::Writer;
use serde::Serialize;
use tracing::{error, info, warn};

use super::format::{format_duration, format_eta, format_number};
use crate::domain::importer::setup::Cache;
use crate::domain::models::action_object::{ActionId, ActionObject};
use crate::inbound::file::Reader;
use crate::inbound::file::csv::Csv;
use crate::inbound::file::excel::Excel;
use crate::outbound::client::action::{ActionClient, PostError};

pub struct FailedAction {
    pub action: ActionObject,
    pub error_type: String,
}

pub struct ProcessingStats {
    pub processed: usize,
    pub imported: usize,
    pub skipped: usize,
    pub failed: Vec<FailedAction>,
}

struct ProcessConfig<'a> {
    /// Ids already in Halo, from the cache plus every id this run has posted.
    existing_ids: &'a mut HashSet<ActionId>,
    action_client: Option<&'a ActionClient>,
    sheet_times: &'a mut Vec<f64>,
    file_name: &'a str,
    sheet_number: usize,
    total_sheets: usize,
    only_parse: bool,
    missing_tickets: &'a mut HashSet<u32>,
    batch_size: usize,
    cache: &'a Cache,
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

#[allow(clippy::too_many_arguments)]
pub async fn process_csv_file(
    file_path: &Path,
    existing_ids: &mut HashSet<ActionId>,
    action_client: Option<&ActionClient>,
    sheet_times: &mut Vec<f64>,
    file_name: &str,
    sheet_number: usize,
    total_sheets: usize,
    only_parse: bool,
    batch_size: usize,
    cache: &Cache,
) -> anyhow::Result<ProcessingStats> {
    let mut missing_tickets: HashSet<u32> = HashSet::new();
    let config = ProcessConfig {
        existing_ids,
        action_client,
        sheet_times,
        file_name,
        sheet_number,
        total_sheets,
        only_parse,
        missing_tickets: &mut missing_tickets,
        batch_size,
        cache,
    };
    let iter = <Reader as Csv>::csv_action_iter(file_path)?;
    let total_rows = iter.total_rows();
    let mut processed = 0;
    let mut imported = 0;
    let mut skipped = 0;
    let mut failed = Vec::new();
    let sheet_start = Instant::now();
    let mut row_times: Vec<f64> = Vec::new();
    let mut pending_skips = 0usize;
    let mut batch: Vec<crate::domain::models::action_object::ActionObject> = Vec::new();
    let mut batch_start = Instant::now();
    let mut batch_number = 0;
    if let Some(total) = total_rows {
        info!(
            "Processing sheet {} of {}: CSV file '{}' ({} rows)",
            config.sheet_number,
            config.total_sheets,
            config.file_name,
            format_number(total)
        );
    } else {
        info!(
            "Processing sheet {} of {}: CSV file '{}'",
            config.sheet_number, config.total_sheets, config.file_name
        );
    }
    for action_result in iter {
        let action = match action_result {
            Ok(a) => a,
            Err(e) => {
                if pending_skips > 0 {
                    info!(
                        "Skipped {} entries (already exist)",
                        format_number(pending_skips)
                    );
                    pending_skips = 0;
                }
                let error_msg = format!(
                    "Failed to deserialize row in CSV file '{}': {}",
                    config.file_name, e
                );
                error!("{}", error_msg);
                // Don't add to failed vec - no ActionObject to retry
                continue;
            }
        };
        processed += 1;
        let action_id = action.action_id();
        let ticket_id = action.ticket_id;
        if config.only_parse {
            // Counted as a real run would: a repeated id is skipped.
            if config.existing_ids.insert(action_id) {
                imported += 1;
            } else {
                skipped += 1;
            }
        } else if config.existing_ids.contains(&action_id)
            || config.missing_tickets.contains(&ticket_id)
            || batch.iter().any(|queued| queued.action_id() == action_id)
        {
            skipped += 1;
            pending_skips += 1;
        } else {
            batch.push(action.clone());
            if batch.len() >= config.batch_size {
                if pending_skips > 0 {
                    info!(
                        "Skipped {} entries (already exist)",
                        format_number(pending_skips)
                    );
                    pending_skips = 0;
                }
                if let Some(client) = config.action_client {
                    match client.post_action_objects(batch.clone()).await {
                        Ok(_) => {
                            let batch_count = batch.len();
                            imported += batch_count;
                            batch_number += 1;

                            // Calculate timing stats
                            let batch_time = batch_start.elapsed().as_secs_f64();
                            let per_row_time = batch_time / batch_count as f64;
                            row_times.push(per_row_time);

                            let avg_row_time = if !row_times.is_empty() {
                                row_times.iter().sum::<f64>() / row_times.len() as f64
                            } else {
                                0.0
                            };
                            let remaining_rows = total_rows.unwrap_or(processed) - processed;

                            if config.batch_size == 1 {
                                info!(
                                    "Imported action {}/{} (ID: {}, ticket: {}) | {} total skipped | {:.2}s/row | ETA: {}",
                                    imported,
                                    total_rows.unwrap_or(0),
                                    batch[0].action_id(),
                                    batch[0].ticket_id,
                                    format_number(skipped),
                                    avg_row_time,
                                    format_eta(remaining_rows, avg_row_time)
                                );
                            } else {
                                let ticket_ids: Vec<u32> = {
                                    let mut ids: Vec<u32> =
                                        batch.iter().map(|a| a.ticket_id).collect();
                                    ids.sort_unstable();
                                    ids.dedup();
                                    ids
                                };
                                let ticket_ids_str = ticket_ids
                                    .iter()
                                    .map(|id| id.to_string())
                                    .collect::<Vec<_>>()
                                    .join(", ");

                                let total_batches = total_rows
                                    .map(|t| (t as f64 / config.batch_size as f64).ceil() as usize)
                                    .unwrap_or(0);
                                info!(
                                    "Imported batch {}/{} | {}/{} actions (tickets: {}) | {} total skipped | {:.2}s/batch, {:.2}s/action | ETA: {}",
                                    batch_number,
                                    total_batches,
                                    imported,
                                    total_rows.unwrap_or(0),
                                    ticket_ids_str,
                                    format_number(skipped),
                                    batch_time,
                                    avg_row_time,
                                    format_eta(remaining_rows, avg_row_time)
                                );
                            }

                            record_imported(config.cache, config.existing_ids, &batch);

                            batch.clear();
                            batch_start = Instant::now();
                        }
                        Err(e) => {
                            let outcome = settle_failed_batch(
                                client,
                                &batch,
                                e,
                                config.missing_tickets,
                                "Batch",
                            )
                            .await;
                            if outcome.regrouped {
                                batch_number += 1;
                            }
                            let recovered = outcome.imported.len();
                            imported += recovered;
                            if recovered > 0 {
                                record_imported(
                                    config.cache,
                                    config.existing_ids,
                                    &outcome.imported,
                                );

                                // Track timing for recovered actions
                                let avg_action_time =
                                    batch_start.elapsed().as_secs_f64() / batch.len() as f64;
                                for _ in 0..recovered {
                                    row_times.push(avg_action_time);
                                }
                            }
                            failed.extend(outcome.failed);

                            batch.clear();
                            batch_start = Instant::now();
                        }
                    }
                } else {
                    let error_type = "Client unavailable".to_string();
                    for action in &batch {
                        let action_id = action.action_id().to_string();
                        let ticket_id = action.ticket_id;
                        let error_msg = format!(
                            "Action client not available for action ID: {} (ticket ID: {})",
                            action_id, ticket_id
                        );
                        error!("{}", error_msg);
                        failed.push(FailedAction {
                            action: action.clone(),
                            error_type: error_type.clone(),
                        });
                    }
                    batch.clear();
                    batch_start = Instant::now();
                }
            }
        }
    }
    if !batch.is_empty() {
        if pending_skips > 0 {
            info!(
                "Skipped {} entries (already exist)",
                format_number(pending_skips)
            );
            pending_skips = 0;
        }
        if let Some(client) = config.action_client {
            match client.post_action_objects(batch.clone()).await {
                Ok(_) => {
                    let batch_count = batch.len();
                    imported += batch_count;
                    batch_number += 1;

                    // Calculate timing stats
                    let batch_time = batch_start.elapsed().as_secs_f64();
                    let per_row_time = batch_time / batch_count as f64;
                    row_times.push(per_row_time);

                    let avg_row_time = if !row_times.is_empty() {
                        row_times.iter().sum::<f64>() / row_times.len() as f64
                    } else {
                        0.0
                    };
                    let remaining_rows = 0; // Final batch, no remaining rows

                    if config.batch_size == 1 {
                        info!(
                            "Imported action {}/{} (ID: {}, ticket: {}) | {} total skipped | {:.2}s/row | ETA: {}",
                            imported,
                            total_rows.unwrap_or(0),
                            batch[0].action_id(),
                            batch[0].ticket_id,
                            format_number(skipped),
                            avg_row_time,
                            format_eta(remaining_rows, avg_row_time)
                        );
                    } else {
                        let ticket_ids: Vec<u32> = {
                            let mut ids: Vec<u32> = batch.iter().map(|a| a.ticket_id).collect();
                            ids.sort_unstable();
                            ids.dedup();
                            ids
                        };
                        let ticket_ids_str = ticket_ids
                            .iter()
                            .map(|id| id.to_string())
                            .collect::<Vec<_>>()
                            .join(", ");

                        let total_batches = total_rows
                            .map(|t| (t as f64 / config.batch_size as f64).ceil() as usize)
                            .unwrap_or(0);
                        info!(
                            "Imported batch {}/{} | {}/{} actions (tickets: {}) | {} total skipped | {:.2}s/batch, {:.2}s/action | ETA: {}",
                            batch_number,
                            total_batches,
                            imported,
                            total_rows.unwrap_or(0),
                            ticket_ids_str,
                            format_number(skipped),
                            batch_time,
                            avg_row_time,
                            format_eta(remaining_rows, avg_row_time)
                        );
                    }

                    record_imported(config.cache, config.existing_ids, &batch);
                }
                Err(e) => {
                    let outcome = settle_failed_batch(
                        client,
                        &batch,
                        e,
                        config.missing_tickets,
                        "Final batch",
                    )
                    .await;
                    let recovered = outcome.imported.len();
                    imported += recovered;
                    if recovered > 0 {
                        record_imported(config.cache, config.existing_ids, &outcome.imported);

                        // Track timing for recovered actions
                        let avg_action_time =
                            batch_start.elapsed().as_secs_f64() / batch.len() as f64;
                        for _ in 0..recovered {
                            row_times.push(avg_action_time);
                        }
                    }
                    failed.extend(outcome.failed);
                }
            }
        } else {
            let error_type = "Client unavailable".to_string();
            for action in &batch {
                let action_id = action.action_id().to_string();
                let ticket_id = action.ticket_id;
                let error_msg = format!(
                    "Action client not available for action ID: {} (ticket ID: {})",
                    action_id, ticket_id
                );
                error!("{}", error_msg);
                failed.push(FailedAction {
                    action: action.clone(),
                    error_type: error_type.clone(),
                });
            }
        }
    }
    if pending_skips > 0 {
        info!(
            "Skipped {} entries (already exist)",
            format_number(pending_skips)
        );
    }
    let sheet_duration = sheet_start.elapsed().as_secs_f64();
    config.sheet_times.push(sheet_duration);
    let avg_sheet_time = config.sheet_times.iter().sum::<f64>() / config.sheet_times.len() as f64;
    info!(
        "Completed sheet {} of {}: CSV file '{}' | {} processed, {} imported, {} skipped in {:.1}s | avg sheet time: {:.1}s",
        config.sheet_number,
        config.total_sheets,
        config.file_name,
        format_number(processed),
        format_number(imported),
        format_number(skipped),
        sheet_duration,
        avg_sheet_time
    );
    Ok(ProcessingStats {
        processed,
        imported,
        skipped,
        failed,
    })
}

#[allow(clippy::too_many_arguments)]
pub async fn process_excel_file(
    file_path: &Path,
    existing_ids: &mut HashSet<ActionId>,
    action_client: Option<&ActionClient>,
    sheet_times: &mut Vec<f64>,
    sheet_number: usize,
    total_sheets: usize,
    only_parse: bool,
    batch_size: usize,
    cache: &Cache,
) -> anyhow::Result<ProcessingStats> {
    let mut missing_tickets: HashSet<u32> = HashSet::new();
    let config = ProcessConfig {
        existing_ids,
        action_client,
        sheet_times,
        file_name: file_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown file"),
        sheet_number,
        total_sheets,
        only_parse,
        missing_tickets: &mut missing_tickets,
        batch_size,
        cache,
    };
    let iter = <Reader as Excel>::excel_action_iter(file_path)?;
    let total_rows = iter.total_rows();
    let sheet_name = iter.sheet_name().to_string();
    let mut processed = 0;
    let mut imported = 0;
    let mut skipped = 0;
    let mut failed = Vec::new();
    let sheet_start = Instant::now();
    let mut row_times: Vec<f64> = Vec::new();
    let mut pending_skips = 0usize;
    let mut batch: Vec<crate::domain::models::action_object::ActionObject> = Vec::new();
    let mut batch_start = Instant::now();
    let mut batch_number = 0;
    info!(
        "Processing sheet {} of {}: Excel file '{}', sheet '{}' ({} rows)",
        config.sheet_number,
        config.total_sheets,
        config.file_name,
        sheet_name,
        format_number(total_rows)
    );
    for action_result in iter {
        let action = match action_result {
            Ok(a) => a,
            Err(e) => {
                if pending_skips > 0 {
                    info!(
                        "Skipped {} entries (already exist)",
                        format_number(pending_skips)
                    );
                    pending_skips = 0;
                }
                let error_msg = format!(
                    "Failed to deserialize row in Excel file '{}', sheet '{}': {}",
                    config.file_name, sheet_name, e
                );
                error!("{}", error_msg);
                // Don't add to failed vec - no ActionObject to retry
                continue;
            }
        };
        processed += 1;
        let action_id = action.action_id();
        let ticket_id = action.ticket_id;
        if config.only_parse {
            // Counted as a real run would: a repeated id is skipped.
            if config.existing_ids.insert(action_id) {
                imported += 1;
            } else {
                skipped += 1;
            }
        } else if config.existing_ids.contains(&action_id)
            || config.missing_tickets.contains(&ticket_id)
            || batch.iter().any(|queued| queued.action_id() == action_id)
        {
            skipped += 1;
            pending_skips += 1;
        } else {
            batch.push(action.clone());
            if batch.len() >= config.batch_size {
                if pending_skips > 0 {
                    info!(
                        "Skipped {} entries (already exist)",
                        format_number(pending_skips)
                    );
                    pending_skips = 0;
                }
                if let Some(client) = config.action_client {
                    match client.post_action_objects(batch.clone()).await {
                        Ok(_) => {
                            let batch_count = batch.len();
                            imported += batch_count;

                            // Calculate timing stats
                            let batch_time = batch_start.elapsed().as_secs_f64();
                            let per_row_time = batch_time / batch_count as f64;
                            row_times.push(per_row_time);

                            let avg_row_time = if !row_times.is_empty() {
                                row_times.iter().sum::<f64>() / row_times.len() as f64
                            } else {
                                0.0
                            };
                            let remaining_rows = total_rows - processed;

                            if config.batch_size == 1 {
                                info!(
                                    "Imported action {}/{} (ID: {}, ticket: {}) | {} total skipped | {:.2}s/row | ETA: {}",
                                    imported,
                                    total_rows,
                                    batch[0].action_id(),
                                    batch[0].ticket_id,
                                    format_number(skipped),
                                    avg_row_time,
                                    format_eta(remaining_rows, avg_row_time)
                                );
                            } else {
                                let ticket_ids: Vec<u32> = {
                                    let mut ids: Vec<u32> =
                                        batch.iter().map(|a| a.ticket_id).collect();
                                    ids.sort_unstable();
                                    ids.dedup();
                                    ids
                                };
                                let ticket_ids_str = ticket_ids
                                    .iter()
                                    .map(|id| id.to_string())
                                    .collect::<Vec<_>>()
                                    .join(", ");

                                let total_batches =
                                    (total_rows as f64 / config.batch_size as f64).ceil() as usize;
                                info!(
                                    "Imported batch {}/{} | {}/{} actions (tickets: {}) | {} total skipped | {:.2}s/batch, {:.2}s/action | ETA: {}",
                                    batch_number,
                                    total_batches,
                                    imported,
                                    total_rows,
                                    ticket_ids_str,
                                    format_number(skipped),
                                    batch_time,
                                    avg_row_time,
                                    format_eta(remaining_rows, avg_row_time)
                                );
                            }

                            record_imported(config.cache, config.existing_ids, &batch);

                            batch.clear();
                            batch_start = Instant::now();
                        }
                        Err(e) => {
                            let outcome = settle_failed_batch(
                                client,
                                &batch,
                                e,
                                config.missing_tickets,
                                "Batch",
                            )
                            .await;
                            if outcome.regrouped {
                                batch_number += 1;
                            }
                            let recovered = outcome.imported.len();
                            imported += recovered;
                            if recovered > 0 {
                                record_imported(
                                    config.cache,
                                    config.existing_ids,
                                    &outcome.imported,
                                );

                                // Track timing for recovered actions
                                let avg_action_time =
                                    batch_start.elapsed().as_secs_f64() / batch.len() as f64;
                                for _ in 0..recovered {
                                    row_times.push(avg_action_time);
                                }
                            }
                            failed.extend(outcome.failed);

                            batch.clear();
                            batch_start = Instant::now();
                        }
                    }
                } else {
                    let error_type = "Client unavailable".to_string();
                    for action in &batch {
                        let action_id = action.action_id().to_string();
                        let ticket_id = action.ticket_id;
                        let error_msg = format!(
                            "Action client not available for action ID: {} (ticket ID: {})",
                            action_id, ticket_id
                        );
                        error!("{}", error_msg);
                        failed.push(FailedAction {
                            action: action.clone(),
                            error_type: error_type.clone(),
                        });
                    }
                    batch.clear();
                    batch_start = Instant::now();
                }
            }
        }
    }
    if !batch.is_empty() {
        if pending_skips > 0 {
            info!(
                "Skipped {} entries (already exist)",
                format_number(pending_skips)
            );
            pending_skips = 0;
        }
        if let Some(client) = config.action_client {
            match client.post_action_objects(batch.clone()).await {
                Ok(_) => {
                    let batch_count = batch.len();
                    imported += batch_count;
                    batch_number += 1;

                    // Calculate timing stats
                    let batch_time = batch_start.elapsed().as_secs_f64();
                    let per_row_time = batch_time / batch_count as f64;
                    row_times.push(per_row_time);

                    let avg_row_time = if !row_times.is_empty() {
                        row_times.iter().sum::<f64>() / row_times.len() as f64
                    } else {
                        0.0
                    };
                    let remaining_rows = 0; // Final batch, no remaining rows

                    if config.batch_size == 1 {
                        info!(
                            "Imported action {}/{} (ID: {}, ticket: {}) | {} total skipped | {:.2}s/row | ETA: {}",
                            imported,
                            total_rows,
                            batch[0].action_id(),
                            batch[0].ticket_id,
                            format_number(skipped),
                            avg_row_time,
                            format_eta(remaining_rows, avg_row_time)
                        );
                    } else {
                        let ticket_ids: Vec<u32> = {
                            let mut ids: Vec<u32> = batch.iter().map(|a| a.ticket_id).collect();
                            ids.sort_unstable();
                            ids.dedup();
                            ids
                        };
                        let ticket_ids_str = ticket_ids
                            .iter()
                            .map(|id| id.to_string())
                            .collect::<Vec<_>>()
                            .join(", ");

                        let total_batches =
                            (total_rows as f64 / config.batch_size as f64).ceil() as usize;
                        info!(
                            "Imported batch {}/{} | {}/{} actions (tickets: {}) | {} total skipped | {:.2}s/batch, {:.2}s/action | ETA: {}",
                            batch_number,
                            total_batches,
                            imported,
                            total_rows,
                            ticket_ids_str,
                            format_number(skipped),
                            batch_time,
                            avg_row_time,
                            format_eta(remaining_rows, avg_row_time)
                        );
                    }

                    record_imported(config.cache, config.existing_ids, &batch);
                }
                Err(e) => {
                    let outcome = settle_failed_batch(
                        client,
                        &batch,
                        e,
                        config.missing_tickets,
                        "Final batch",
                    )
                    .await;
                    let recovered = outcome.imported.len();
                    imported += recovered;
                    if recovered > 0 {
                        record_imported(config.cache, config.existing_ids, &outcome.imported);

                        // Track timing for recovered actions
                        let avg_action_time =
                            batch_start.elapsed().as_secs_f64() / batch.len() as f64;
                        for _ in 0..recovered {
                            row_times.push(avg_action_time);
                        }
                    }
                    failed.extend(outcome.failed);
                }
            }
        } else {
            let error_type = "Client unavailable".to_string();
            for action in &batch {
                let action_id = action.action_id().to_string();
                let ticket_id = action.ticket_id;
                let error_msg = format!(
                    "Action client not available for action ID: {} (ticket ID: {})",
                    action_id, ticket_id
                );
                error!("{}", error_msg);
                failed.push(FailedAction {
                    action: action.clone(),
                    error_type: error_type.clone(),
                });
            }
        }
    }
    if pending_skips > 0 {
        info!(
            "Skipped {} entries (already exist)",
            format_number(pending_skips)
        );
    }
    let sheet_duration = sheet_start.elapsed().as_secs_f64();
    config.sheet_times.push(sheet_duration);
    let avg_sheet_time = config.sheet_times.iter().sum::<f64>() / config.sheet_times.len() as f64;
    info!(
        "Completed sheet {} of {}: Excel file '{}', sheet '{}' | {} processed, {} imported, {} skipped in {:.1}s | avg sheet time: {:.1}s",
        config.sheet_number,
        config.total_sheets,
        config.file_name,
        sheet_name,
        format_number(processed),
        format_number(imported),
        format_number(skipped),
        sheet_duration,
        avg_sheet_time
    );
    Ok(ProcessingStats {
        processed,
        imported,
        skipped,
        failed,
    })
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
    already_exists: usize,
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

#[allow(clippy::too_many_arguments)]
pub fn write_summary_json(
    log_dir: &str,
    total_processed: usize,
    total_imported: usize,
    total_skipped: usize,
    failed: &[FailedAction],
    runtime_secs: f64,
    skipped_files: &[String],
    total_sheets: usize,
) -> anyhow::Result<()> {
    let json_path = format!("{}/summary.json", log_dir);

    // Calculate skipped breakdown
    let missing_ticket_count = failed
        .iter()
        .filter(|f| f.error_type == "Ticket not found")
        .count();
    let already_exists = total_skipped.saturating_sub(missing_ticket_count);

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
            already_exists,
            missing_ticket: missing_ticket_count,
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
        let mut existing_ids = HashSet::new();
        let mut sheet_times = Vec::new();
        let mut stats = Vec::new();
        for (number, (file_name, csv)) in files.iter().enumerate() {
            let path = dir.join(file_name);
            std::fs::write(&path, csv).unwrap();
            let file_stats = tokio::time::timeout(
                HUNG,
                process_csv_file(
                    &path,
                    &mut existing_ids,
                    Some(&client),
                    &mut sheet_times,
                    file_name,
                    number + 1,
                    files.len(),
                    false,
                    batch_size,
                    &cache,
                ),
            )
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
        let mut existing_ids = HashSet::new();
        let mut sheet_times = Vec::new();
        let (mut processed, mut imported, mut skipped) = (0, 0, 0);
        let mut failed: Vec<FailedAction> = Vec::new();
        let mut skipped_files: Vec<String> = Vec::new();
        let total_sheets = files.len();
        for (number, (file_name, bytes)) in files.iter().enumerate() {
            let path = dir.join(file_name);
            std::fs::write(&path, bytes).unwrap();
            let run = async {
                if file_name.ends_with(".csv") {
                    process_csv_file(
                        &path,
                        &mut existing_ids,
                        client.as_ref(),
                        &mut sheet_times,
                        file_name,
                        number + 1,
                        total_sheets,
                        only_parse,
                        batch_size,
                        &cache,
                    )
                    .await
                } else {
                    process_excel_file(
                        &path,
                        &mut existing_ids,
                        client.as_ref(),
                        &mut sheet_times,
                        number + 1,
                        total_sheets,
                        only_parse,
                        batch_size,
                        &cache,
                    )
                    .await
                }
            };
            match tokio::time::timeout(HUNG, run)
                .await
                .expect("the import hung")
            {
                Ok(stats) => {
                    processed += stats.processed;
                    imported += stats.imported;
                    skipped += stats.skipped;
                    failed.extend(stats.failed);
                }
                Err(e) => skipped_files.push(format!("{file_name:?}: {e}")),
            }
        }
        let code = exit_code(&ImportSummary {
            total_processed: processed,
            total_imported: imported,
            total_skipped: skipped,
            total_failed: failed.len(),
            skipped_files: skipped_files.clone(),
            total_runtime_secs: 1.0,
            sheet_times: sheet_times.clone(),
        });
        if !failed.is_empty() {
            write_retry_csv(log_dir_str, &failed).unwrap();
        }
        write_summary_json(
            log_dir_str,
            processed,
            imported,
            skipped,
            &failed,
            1.0,
            &skipped_files,
            total_sheets,
        )
        .unwrap();
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
            processed,
            imported,
            skipped,
            failed: failed.len(),
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
                "skipped_breakdown": { "already_exists": 2, "missing_ticket": 1 },
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
}
