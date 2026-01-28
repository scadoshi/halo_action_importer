use crate::domain::importer::setup::append_imported_ids_to_cache;
use crate::domain::models::action_object::ActionObject;
use crate::inbound::file::{Reader, csv::Csv, excel::Excel};
use crate::outbound::client::action::ActionClient;
use chrono::Utc;
use csv::Writer;
use serde::Serialize;
use std::collections::HashMap;
use std::collections::HashSet;
use std::fs::File;
use std::path::Path;
use std::time::Instant;
use tracing::{error, info, warn};

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
    existing_ids: &'a HashSet<String>,
    action_client: Option<&'a ActionClient>,
    sheet_times: &'a mut Vec<f64>,
    file_name: &'a str,
    sheet_number: usize,
    total_sheets: usize,
    only_parse: bool,
    missing_tickets: &'a mut HashSet<u32>,
    batch_size: usize,
}

fn is_not_found_error(error_str: &str) -> bool {
    error_str.contains("not found")
        || error_str.contains("Not Found")
        || error_str.contains("404")
        || error_str.contains("does not exist")
        || error_str.contains("doesn't exist")
}

fn extract_error_type(error_str: &str) -> String {
    if is_not_found_error(error_str) {
        "Ticket not found".to_string()
    } else if error_str.contains("status 400") {
        "Validation error".to_string()
    } else if error_str.contains("status 401") {
        "Authentication error".to_string()
    } else if error_str.contains("status 504") {
        "Gateway timeout".to_string()
    } else if error_str.contains("status 500") {
        "Server error".to_string()
    } else if error_str.contains("Action client not available") {
        "Client unavailable".to_string()
    } else {
        "Unknown error".to_string()
    }
}

struct TicketGroupedResult {
    missing_tickets: HashSet<u32>,
    imported_actions: Vec<ActionObject>,
    failed_actions: Vec<FailedAction>,
}

/// Efficiently identify missing tickets and import valid actions.
/// Strategy: Group actions by ticket_id and retry each ticket group.
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
    let mut ticket_groups: HashMap<u32, Vec<ActionObject>> = HashMap::new();
    for action in batch {
        ticket_groups
            .entry(action.ticket_id)
            .or_insert_with(Vec::new)
            .push(action.clone());
    }

    info!(
        "Batch failed with 'not found' error - retrying {} ticket groups",
        ticket_groups.len()
    );

    // Try each ticket group
    for (ticket_id, actions) in ticket_groups {
        match client.post_action_objects(actions.clone()).await {
            Ok(_) => {
                // All actions for this ticket imported successfully
                result.imported_actions.extend(actions);
            }
            Err(e) => {
                let err_str = e.to_string();
                if is_not_found_error(&err_str) {
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
                } else {
                    // Other error type, mark all actions in this group as failed
                    let error_type = extract_error_type(&err_str);
                    error!(
                        "Failed to import {} action(s) for ticket {}: {}",
                        actions.len(),
                        ticket_id,
                        err_str
                    );
                    for action in actions {
                        result.failed_actions.push(FailedAction {
                            action,
                            error_type: error_type.clone(),
                        });
                    }
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

#[allow(clippy::too_many_arguments)]
pub async fn process_csv_file(
    file_path: &Path,
    existing_ids: &HashSet<String>,
    action_client: Option<&ActionClient>,
    sheet_times: &mut Vec<f64>,
    file_name: &str,
    sheet_number: usize,
    total_sheets: usize,
    only_parse: bool,
    batch_size: usize,
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
        let action_id = action.action_id().to_string();
        let ticket_id = action.ticket_id;
        if config.only_parse {
            if config.existing_ids.contains(&action_id) {
                skipped += 1;
            } else {
                imported += 1;
            }
        } else if config.existing_ids.contains(&action_id)
            || config.missing_tickets.contains(&ticket_id)
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
                                    let mut ids: Vec<u32> = batch.iter().map(|a| a.ticket_id).collect();
                                    ids.sort_unstable();
                                    ids.dedup();
                                    ids
                                };
                                let ticket_ids_str = ticket_ids.iter().map(|id| id.to_string()).collect::<Vec<_>>().join(", ");

                                let total_batches = total_rows.map(|t| (t as f64 / config.batch_size as f64).ceil() as usize).unwrap_or(0);
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

                            let imported_ids: Vec<String> =
                                batch.iter().map(|a| a.action_id().to_string()).collect();
                            if let Err(e) = append_imported_ids_to_cache(&imported_ids) {
                                warn!("Failed to update cache with imported IDs: {}", e);
                            }

                            batch.clear();
                            batch_start = Instant::now();
                        }
                        Err(e) => {
                            let error_str = e.to_string();

                            if is_not_found_error(&error_str) && batch.len() > 1 {
                                // Use ticket-grouped retry to identify missing tickets
                                warn!(
                                    "Batch of {} actions failed with 'not found' error - retrying by ticket groups",
                                    batch.len()
                                );

                                let search_result = retry_by_ticket_groups(client, &batch).await;

                                // Add identified missing tickets to skip set
                                for ticket_id in &search_result.missing_tickets {
                                    config.missing_tickets.insert(*ticket_id);
                                    warn!(
                                        "Ticket ID: {} not found - will skip future actions for this ticket",
                                        ticket_id
                                    );
                                }

                                // Update imported count and cache
                                let recovered = search_result.imported_actions.len();
                                imported += recovered;
                                batch_number += 1;

                                if !search_result.imported_actions.is_empty() {
                                    let imported_ids: Vec<String> = search_result
                                        .imported_actions
                                        .iter()
                                        .map(|a| a.action_id().to_string())
                                        .collect();
                                    if let Err(e) = append_imported_ids_to_cache(&imported_ids) {
                                        warn!("Failed to update cache: {}", e);
                                    }

                                    // Track timing for recovered actions
                                    let avg_action_time = batch_start.elapsed().as_secs_f64() / batch.len() as f64;
                                    for _ in 0..recovered {
                                        row_times.push(avg_action_time);
                                    }
                                }

                                // Add failed actions to the failed list
                                for failed_action in search_result.failed_actions {
                                    error!(
                                        "Failed to import action ID: {} (ticket ID: {})",
                                        failed_action.action.action_id(),
                                        failed_action.action.ticket_id
                                    );
                                    failed.push(failed_action);
                                }

                                info!(
                                    "Ticket group retry complete: recovered {}/{} actions, identified {} missing ticket(s)",
                                    recovered,
                                    batch.len(),
                                    search_result.missing_tickets.len()
                                );
                            } else {
                                // Not a "not found" error, or batch size is 1
                                // Mark all as failed (existing behavior for other errors)
                                let error_type = extract_error_type(&error_str);

                                for action in &batch {
                                    let action_id = action.action_id().to_string();
                                    let ticket_id = action.ticket_id;

                                    if is_not_found_error(&error_str) {
                                        config.missing_tickets.insert(ticket_id);
                                        warn!(
                                            "Ticket ID: {} not found - will skip future actions for this ticket",
                                            ticket_id
                                        );
                                    }

                                    let error_msg = format!(
                                        "Failed to import action ID: {} (ticket ID: {}): {}",
                                        action_id, ticket_id, e
                                    );
                                    error!("{}", error_msg);

                                    failed.push(FailedAction {
                                        action: action.clone(),
                                        error_type: error_type.clone(),
                                    });
                                }
                            }

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
                        let ticket_ids_str = ticket_ids.iter().map(|id| id.to_string()).collect::<Vec<_>>().join(", ");

                        let total_batches = total_rows.map(|t| (t as f64 / config.batch_size as f64).ceil() as usize).unwrap_or(0);
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

                    let imported_ids: Vec<String> =
                        batch.iter().map(|a| a.action_id().to_string()).collect();
                    if let Err(e) = append_imported_ids_to_cache(&imported_ids) {
                        warn!("Failed to update cache with imported IDs: {}", e);
                    }
                }
                Err(e) => {
                    let error_str = e.to_string();
                    let is_not_found = error_str.contains("not found")
                        || error_str.contains("Not Found")
                        || error_str.contains("404")
                        || error_str.contains("does not exist")
                        || error_str.contains("doesn't exist");

                    if is_not_found && batch.len() > 1 {
                        // Use ticket-grouped retry to identify missing tickets
                        warn!(
                            "Final batch of {} actions failed with 'not found' error - retrying by ticket groups",
                            batch.len()
                        );

                        let search_result = retry_by_ticket_groups(client, &batch).await;

                        // Add identified missing tickets to skip set
                        for ticket_id in &search_result.missing_tickets {
                            config.missing_tickets.insert(*ticket_id);
                            warn!(
                                "Ticket ID: {} not found - will skip future actions for this ticket",
                                ticket_id
                            );
                        }

                        // Update imported count and cache
                        let recovered = search_result.imported_actions.len();
                        imported += recovered;

                        if !search_result.imported_actions.is_empty() {
                            let imported_ids: Vec<String> = search_result
                                .imported_actions
                                .iter()
                                .map(|a| a.action_id().to_string())
                                .collect();
                            if let Err(e) = append_imported_ids_to_cache(&imported_ids) {
                                warn!("Failed to update cache: {}", e);
                            }

                            // Track timing for recovered actions
                            let avg_action_time = batch_start.elapsed().as_secs_f64() / batch.len() as f64;
                            for _ in 0..recovered {
                                row_times.push(avg_action_time);
                            }
                        }

                        // Add failed actions to the failed list
                        for failed_action in search_result.failed_actions {
                            error!(
                                "Failed to import action ID: {} (ticket ID: {})",
                                failed_action.action.action_id(),
                                failed_action.action.ticket_id
                            );
                            failed.push(failed_action);
                        }

                        info!(
                            "Ticket group retry complete: recovered {}/{} actions, identified {} missing ticket(s)",
                            recovered,
                            batch.len(),
                            search_result.missing_tickets.len()
                        );
                    } else {
                        // Not a "not found" error, or batch size is 1
                        // Mark all as failed (existing behavior for other errors)
                        let error_type = extract_error_type(&error_str);

                        for action in &batch {
                            let action_id = action.action_id().to_string();
                            let ticket_id = action.ticket_id;

                            if is_not_found {
                                config.missing_tickets.insert(ticket_id);
                                warn!(
                                    "Ticket ID: {} not found - will skip future actions for this ticket",
                                    ticket_id
                                );
                            }

                            let error_msg = format!(
                                "Failed to import action ID: {} (ticket ID: {}): {}",
                                action_id, ticket_id, e
                            );
                            error!("{}", error_msg);

                            failed.push(FailedAction {
                                action: action.clone(),
                                error_type: error_type.clone(),
                            });
                        }
                    }
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
    existing_ids: &HashSet<String>,
    action_client: Option<&ActionClient>,
    sheet_times: &mut Vec<f64>,
    sheet_number: usize,
    total_sheets: usize,
    only_parse: bool,
    batch_size: usize,
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
        let action_id = action.action_id().to_string();
        let ticket_id = action.ticket_id;
        if config.only_parse {
            if config.existing_ids.contains(&action_id) {
                skipped += 1;
            } else {
                imported += 1;
            }
        } else if config.existing_ids.contains(&action_id)
            || config.missing_tickets.contains(&ticket_id)
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
                                    let mut ids: Vec<u32> = batch.iter().map(|a| a.ticket_id).collect();
                                    ids.sort_unstable();
                                    ids.dedup();
                                    ids
                                };
                                let ticket_ids_str = ticket_ids.iter().map(|id| id.to_string()).collect::<Vec<_>>().join(", ");

                                let total_batches = (total_rows as f64 / config.batch_size as f64).ceil() as usize;
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

                            let imported_ids: Vec<String> =
                                batch.iter().map(|a| a.action_id().to_string()).collect();
                            if let Err(e) = append_imported_ids_to_cache(&imported_ids) {
                                warn!("Failed to update cache with imported IDs: {}", e);
                            }

                            batch.clear();
                            batch_start = Instant::now();
                        }
                        Err(e) => {
                            let error_str = e.to_string();

                            if is_not_found_error(&error_str) && batch.len() > 1 {
                                // Use ticket-grouped retry to identify missing tickets
                                warn!(
                                    "Batch of {} actions failed with 'not found' error - retrying by ticket groups",
                                    batch.len()
                                );

                                let search_result = retry_by_ticket_groups(client, &batch).await;

                                // Add identified missing tickets to skip set
                                for ticket_id in &search_result.missing_tickets {
                                    config.missing_tickets.insert(*ticket_id);
                                    warn!(
                                        "Ticket ID: {} not found - will skip future actions for this ticket",
                                        ticket_id
                                    );
                                }

                                // Update imported count and cache
                                let recovered = search_result.imported_actions.len();
                                imported += recovered;
                                batch_number += 1;

                                if !search_result.imported_actions.is_empty() {
                                    let imported_ids: Vec<String> = search_result
                                        .imported_actions
                                        .iter()
                                        .map(|a| a.action_id().to_string())
                                        .collect();
                                    if let Err(e) = append_imported_ids_to_cache(&imported_ids) {
                                        warn!("Failed to update cache: {}", e);
                                    }

                                    // Track timing for recovered actions
                                    let avg_action_time = batch_start.elapsed().as_secs_f64() / batch.len() as f64;
                                    for _ in 0..recovered {
                                        row_times.push(avg_action_time);
                                    }
                                }

                                // Add failed actions to the failed list
                                for failed_action in search_result.failed_actions {
                                    error!(
                                        "Failed to import action ID: {} (ticket ID: {})",
                                        failed_action.action.action_id(),
                                        failed_action.action.ticket_id
                                    );
                                    failed.push(failed_action);
                                }

                                info!(
                                    "Ticket group retry complete: recovered {}/{} actions, identified {} missing ticket(s)",
                                    recovered,
                                    batch.len(),
                                    search_result.missing_tickets.len()
                                );
                            } else {
                                // Not a "not found" error, or batch size is 1
                                // Mark all as failed (existing behavior for other errors)
                                let error_type = extract_error_type(&error_str);

                                for action in &batch {
                                    let action_id = action.action_id().to_string();
                                    let ticket_id = action.ticket_id;

                                    if is_not_found_error(&error_str) {
                                        config.missing_tickets.insert(ticket_id);
                                        warn!(
                                            "Ticket ID: {} not found - will skip future actions for this ticket",
                                            ticket_id
                                        );
                                    }

                                    let error_msg = format!(
                                        "Failed to import action ID: {} (ticket ID: {}): {}",
                                        action_id, ticket_id, e
                                    );
                                    error!("{}", error_msg);

                                    failed.push(FailedAction {
                                        action: action.clone(),
                                        error_type: error_type.clone(),
                                    });
                                }
                            }

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
                        let ticket_ids_str = ticket_ids.iter().map(|id| id.to_string()).collect::<Vec<_>>().join(", ");

                        let total_batches = (total_rows as f64 / config.batch_size as f64).ceil() as usize;
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

                    let imported_ids: Vec<String> =
                        batch.iter().map(|a| a.action_id().to_string()).collect();
                    if let Err(e) = append_imported_ids_to_cache(&imported_ids) {
                        warn!("Failed to update cache with imported IDs: {}", e);
                    }
                }
                Err(e) => {
                    let error_str = e.to_string();
                    let is_not_found = error_str.contains("not found")
                        || error_str.contains("Not Found")
                        || error_str.contains("404")
                        || error_str.contains("does not exist")
                        || error_str.contains("doesn't exist");

                    if is_not_found && batch.len() > 1 {
                        // Use ticket-grouped retry to identify missing tickets
                        warn!(
                            "Final batch of {} actions failed with 'not found' error - retrying by ticket groups",
                            batch.len()
                        );

                        let search_result = retry_by_ticket_groups(client, &batch).await;

                        // Add identified missing tickets to skip set
                        for ticket_id in &search_result.missing_tickets {
                            config.missing_tickets.insert(*ticket_id);
                            warn!(
                                "Ticket ID: {} not found - will skip future actions for this ticket",
                                ticket_id
                            );
                        }

                        // Update imported count and cache
                        let recovered = search_result.imported_actions.len();
                        imported += recovered;

                        if !search_result.imported_actions.is_empty() {
                            let imported_ids: Vec<String> = search_result
                                .imported_actions
                                .iter()
                                .map(|a| a.action_id().to_string())
                                .collect();
                            if let Err(e) = append_imported_ids_to_cache(&imported_ids) {
                                warn!("Failed to update cache: {}", e);
                            }

                            // Track timing for recovered actions
                            let avg_action_time = batch_start.elapsed().as_secs_f64() / batch.len() as f64;
                            for _ in 0..recovered {
                                row_times.push(avg_action_time);
                            }
                        }

                        // Add failed actions to the failed list
                        for failed_action in search_result.failed_actions {
                            error!(
                                "Failed to import action ID: {} (ticket ID: {})",
                                failed_action.action.action_id(),
                                failed_action.action.ticket_id
                            );
                            failed.push(failed_action);
                        }

                        info!(
                            "Ticket group retry complete: recovered {}/{} actions, identified {} missing ticket(s)",
                            recovered,
                            batch.len(),
                            search_result.missing_tickets.len()
                        );
                    } else {
                        // Not a "not found" error, or batch size is 1
                        // Mark all as failed (existing behavior for other errors)
                        let error_type = extract_error_type(&error_str);

                        for action in &batch {
                            let action_id = action.action_id().to_string();
                            let ticket_id = action.ticket_id;

                            if is_not_found {
                                config.missing_tickets.insert(ticket_id);
                                warn!(
                                    "Ticket ID: {} not found - will skip future actions for this ticket",
                                    ticket_id
                                );
                            }

                            let error_msg = format!(
                                "Failed to import action ID: {} (ticket ID: {}): {}",
                                action_id, ticket_id, e
                            );
                            error!("{}", error_msg);

                            failed.push(FailedAction {
                                action: action.clone(),
                                error_type: error_type.clone(),
                            });
                        }
                    }
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

fn format_number(n: usize) -> String {
    let s = n.to_string();
    let mut result = String::new();
    let chars: Vec<char> = s.chars().collect();
    for (i, &ch) in chars.iter().enumerate() {
        if i > 0 && (chars.len() - i).is_multiple_of(3) {
            result.push(',');
        }
        result.push(ch);
    }
    result
}

fn format_duration(seconds: f64) -> String {
    let total_seconds = seconds as u64;
    let days = total_seconds / 86400;
    let hours = (total_seconds % 86400) / 3600;
    let minutes = (total_seconds % 3600) / 60;
    let secs = total_seconds % 60;

    let mut parts = Vec::new();
    if days > 0 {
        parts.push(format!("{}d", days));
    }
    if hours > 0 {
        parts.push(format!("{}h", hours));
    }
    if minutes > 0 {
        parts.push(format!("{}m", minutes));
    }
    if secs > 0 || parts.is_empty() {
        parts.push(format!("{}s", secs));
    }

    parts.join(" ")
}

fn format_eta(remaining_rows: usize, avg_time_per_row: f64) -> String {
    if avg_time_per_row == 0.0 {
        return "unknown".to_string();
    }
    let remaining_secs = remaining_rows as f64 * avg_time_per_row;
    format_duration(remaining_secs)
}

pub fn write_retry_csv(log_dir: &str, failed: &[FailedAction]) -> anyhow::Result<()> {
    let csv_path = format!("{}/retry.csv", log_dir);
    let file = File::create(&csv_path)?;
    let mut writer = Writer::from_writer(file);

    // Write header
    writer.write_record(&[
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
        let date_str = action.actiondate
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
    let missing_ticket_count = failed.iter()
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
        error_map.entry(failed_action.error_type.clone())
            .or_insert_with(Vec::new)
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
