//! `polkagent memory` — agent memory management subcommands.

use std::sync::Arc;

use anyhow::Result;
use tracing::info;
use uuid::Uuid;

use polkagent_core::ids::AgentId;
use polkagent_memory::retention::{RetentionPolicy, RetentionSweeper};
use polkagent_memory::sqlite::SqliteMemoryStore;
use polkagent_memory::store::MemoryStore;
use polkagent_memory::types::{MemoryId, MemoryType};
use polkagent_memory::MemoryService;

use crate::cli::{
    MemoryCmd, MemoryExportCmd, MemoryForgetCmd, MemoryImportCmd, MemoryListCmd, MemorySearchCmd,
    MemoryStatsCmd, MemorySweepCmd,
};

/// Dispatch the memory subcommand.
pub fn run(cmd: &MemoryCmd) -> Result<()> {
    // Open the memory store from the default path.
    let store = open_memory_store()?;
    let svc = MemoryService::new(Arc::new(store.clone()));

    match cmd {
        MemoryCmd::Search(c) => search(c, &svc, &store),
        MemoryCmd::List(c) => list(c, &store),
        MemoryCmd::Forget(c) => forget(c, &svc),
        MemoryCmd::Stats(c) => stats(c, &store),
        MemoryCmd::Export(c) => export(c, &svc),
        MemoryCmd::Import(c) => import(c, &svc),
        MemoryCmd::Sweep(c) => sweep(c, &store),
    }
}

// ---------------------------------------------------------------------------
// search
// ---------------------------------------------------------------------------

fn search(cmd: &MemorySearchCmd, svc: &MemoryService, store: &SqliteMemoryStore) -> Result<()> {
    let rt = tokio::runtime::Handle::current();

    let limit = cmd.limit;
    let query = &cmd.query;

    // Resolve the agent_id: use the provided one, or fall back to a cross-agent
    // direct search when none is given.
    let results = if let Some(ref agent_id_str) = cmd.agent_id {
        let agent_id: AgentId = agent_id_str
            .parse()
            .map_err(|e| anyhow::anyhow!("Invalid agent ID '{}': {e}", agent_id_str))?;
        rt.block_on(async { svc.recall(agent_id, query, limit).await })
    } else {
        // Cross-agent search: use the store directly with a nil agent_id, which
        // in LIKE mode returns entries matching the text across all agents.
        // We also try a direct raw search using the store's search method.
        rt.block_on(async {
            let q = polkagent_memory::types::MemoryQuery {
                agent_id: AgentId::from_uuid(Uuid::nil()),
                query_text: query.clone(),
                memory_types: None,
                limit,
                min_relevance: None,
                since: None,
                episode_id: None,
            };
            store.search(&q).await
        })
    };

    match results {
        Ok(entries) => {
            if cmd.json {
                let items: Vec<serde_json::Value> = entries
                    .iter()
                    .map(|e| {
                        serde_json::json!({
                            "id": e.id.to_string(),
                            "agent_id": e.agent_id.to_string(),
                            "memory_type": e.memory_type.to_string(),
                            "content": e.content,
                            "relevance_score": e.relevance_score,
                            "access_count": e.access_count,
                            "created_at": e.created_at.to_rfc3339(),
                        })
                    })
                    .collect();
                println!("{}", serde_json::to_string_pretty(&items)?);
            } else if entries.is_empty() {
                println!("No memories found matching '{query}'.");
            } else {
                println!("Memory search results for '{query}':");
                println!("{}", "-".repeat(80));
                for entry in &entries {
                    println!("  ID:        {}", entry.id);
                    println!("  Agent:     {}", entry.agent_id);
                    println!("  Type:      {}", entry.memory_type);
                    println!("  Score:     {:.2}", entry.relevance_score);
                    println!("  Content:   {}", truncate(&entry.content, 120));
                    println!("  Created:   {}", entry.created_at);
                    println!();
                }
                println!("{} result(s)", entries.len());
            }
        }
        Err(e) => {
            eprintln!("Memory search failed: {e}");
            eprintln!("Note: The memory store may not be initialized. Run `polkagent init` first.");
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// list
// ---------------------------------------------------------------------------

fn list(cmd: &MemoryListCmd, store: &SqliteMemoryStore) -> Result<()> {
    let rt = tokio::runtime::Handle::current();

    // Build a query that lists memories. We search with empty text to get all.
    let type_filter: Option<Vec<MemoryType>> = cmd.memory_type.as_ref().map(|t| vec![*t]);

    let query = polkagent_memory::types::MemoryQuery {
        agent_id: AgentId::from_uuid(Uuid::nil()),
        query_text: String::new(),
        memory_types: type_filter,
        limit: cmd.limit,
        min_relevance: None,
        since: None,
        episode_id: None,
    };

    let results = rt.block_on(async { store.search(&query).await });

    match results {
        Ok(entries) => {
            if cmd.json {
                let items: Vec<serde_json::Value> = entries
                    .iter()
                    .map(|e| {
                        serde_json::json!({
                            "id": e.id.to_string(),
                            "agent_id": e.agent_id.to_string(),
                            "memory_type": e.memory_type.to_string(),
                            "content": e.content,
                            "relevance_score": e.relevance_score,
                            "created_at": e.created_at.to_rfc3339(),
                        })
                    })
                    .collect();
                println!("{}", serde_json::to_string_pretty(&items)?);
            } else if entries.is_empty() {
                println!("No memories found.");
            } else {
                println!(
                    "{:<36}  {:<12}  {:<8}  Content",
                    "ID", "Type", "Score"
                );
                println!("{}", "-".repeat(100));
                for entry in &entries {
                    println!(
                        "{:<36}  {:<12}  {:<8.2}  {}",
                        entry.id,
                        entry.memory_type,
                        entry.relevance_score,
                        truncate(&entry.content, 60)
                    );
                }
                println!();
                println!(
                    "{} memor{}",
                    entries.len(),
                    if entries.len() == 1 { "y" } else { "ies" }
                );
            }
        }
        Err(e) => {
            eprintln!("Memory list failed: {e}");
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// forget
// ---------------------------------------------------------------------------

fn forget(cmd: &MemoryForgetCmd, svc: &MemoryService) -> Result<()> {
    let rt = tokio::runtime::Handle::current();

    let id: MemoryId = cmd
        .memory_id
        .parse()
        .map_err(|e| anyhow::anyhow!("Invalid memory ID '{}': {e}", cmd.memory_id))?;

    let result = rt.block_on(async { svc.forget(id).await });

    match result {
        Ok(()) => {
            println!("Memory '{}' deleted.", cmd.memory_id);
            info!(memory_id = %cmd.memory_id, "memory deleted via CLI");
        }
        Err(e) => {
            anyhow::bail!("Failed to delete memory '{}': {e}", cmd.memory_id);
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// stats
// ---------------------------------------------------------------------------

fn stats(cmd: &MemoryStatsCmd, store: &SqliteMemoryStore) -> Result<()> {
    let home = std::env::var("HOME").unwrap_or_default();
    let default_path = format!("{home}/.local/share/polkagent/memory.db");
    let db_path = std::env::var("POLKAGENT_MEMORY_DB_PATH").unwrap_or(default_path);
    let expanded = expand_tilde(&db_path);

    let conn = rusqlite::Connection::open_with_flags(
        &expanded,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    );

    match conn {
        Ok(conn) => {
            let total: i64 = conn
                .query_row("SELECT count(*) FROM memories", [], |r| r.get(0))
                .unwrap_or(0);
            let episodic: i64 = conn
                .query_row(
                    "SELECT count(*) FROM memories WHERE memory_type = 'episodic'",
                    [],
                    |r| r.get(0),
                )
                .unwrap_or(0);
            let semantic: i64 = conn
                .query_row(
                    "SELECT count(*) FROM memories WHERE memory_type = 'semantic'",
                    [],
                    |r| r.get(0),
                )
                .unwrap_or(0);
            let procedural: i64 = conn
                .query_row(
                    "SELECT count(*) FROM memories WHERE memory_type = 'procedural'",
                    [],
                    |r| r.get(0),
                )
                .unwrap_or(0);
            let episodes: i64 = conn
                .query_row("SELECT count(*) FROM episodes", [], |r| r.get(0))
                .unwrap_or(0);

            if cmd.json {
                let out = serde_json::json!({
                    "total_memories": total,
                    "episodic": episodic,
                    "semantic": semantic,
                    "procedural": procedural,
                    "episodes": episodes,
                });
                println!("{}", serde_json::to_string_pretty(&out)?);
            } else {
                println!("Memory Statistics");
                println!("{}", "-".repeat(40));
                println!("  Total memories:   {total}");
                println!("    Episodic:       {episodic}");
                println!("    Semantic:       {semantic}");
                println!("    Procedural:     {procedural}");
                println!("  Episodes:         {episodes}");
            }

            // Suppress unused warning.
            let _ = store;
        }
        Err(_) => {
            if cmd.json {
                let out = serde_json::json!({
                    "total_memories": 0,
                    "message": "Memory store not initialized",
                });
                println!("{}", serde_json::to_string_pretty(&out)?);
            } else {
                println!("Memory store not initialized.");
                println!("Run `polkagent init` to create the database.");
            }

            let _ = store;
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// export
// ---------------------------------------------------------------------------

fn export(cmd: &MemoryExportCmd, svc: &MemoryService) -> Result<()> {
    let rt = tokio::runtime::Handle::current();

    let agent_id: AgentId = cmd
        .agent_id
        .parse()
        .map_err(|e| anyhow::anyhow!("Invalid agent ID '{}': {e}", cmd.agent_id))?;

    let path = &cmd.output_path;

    let result = rt.block_on(async { svc.export_agent_memory(&agent_id, path).await });

    match result {
        Ok(count) => {
            let file_size = std::fs::metadata(path)
                .map(|m| m.len())
                .unwrap_or(0);

            if cmd.json {
                let out = serde_json::json!({
                    "agent_id": agent_id.to_string(),
                    "output_path": path.display().to_string(),
                    "records_exported": count,
                    "file_size_bytes": file_size,
                });
                println!("{}", serde_json::to_string_pretty(&out)?);
            } else {
                println!("Export complete.");
                println!("  Agent:    {agent_id}");
                println!("  Output:   {}", path.display());
                println!("  Records:  {count}");
                println!("  Size:     {} bytes", file_size);
            }

            info!(
                agent_id = %agent_id,
                output_path = %path.display(),
                records = count,
                "memory archive exported via CLI"
            );
        }
        Err(e) => {
            anyhow::bail!("Export failed: {e}");
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// import
// ---------------------------------------------------------------------------

fn import(cmd: &MemoryImportCmd, svc: &MemoryService) -> Result<()> {
    let rt = tokio::runtime::Handle::current();

    let path = &cmd.input_path;

    if !path.exists() {
        anyhow::bail!("Input file does not exist: {}", path.display());
    }

    let file_size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);

    let result = rt.block_on(async { svc.import_agent_memory(path).await });

    match result {
        Ok(import_result) => {
            if cmd.json {
                let errors_json: Vec<serde_json::Value> = import_result
                    .errors
                    .iter()
                    .map(|e| serde_json::Value::String(e.clone()))
                    .collect();
                let out = serde_json::json!({
                    "input_path": path.display().to_string(),
                    "file_size_bytes": file_size,
                    "imported": import_result.imported_count,
                    "skipped": import_result.skipped_count,
                    "errors": errors_json,
                });
                println!("{}", serde_json::to_string_pretty(&out)?);
            } else {
                println!("Import complete.");
                println!("  Input:    {}", path.display());
                println!("  Size:     {} bytes", file_size);
                println!("  Imported: {}", import_result.imported_count);
                println!("  Skipped:  {}", import_result.skipped_count);
                if !import_result.errors.is_empty() {
                    println!("  Errors ({}):", import_result.errors.len());
                    for e in &import_result.errors {
                        println!("    - {e}");
                    }
                }
            }

            info!(
                input_path = %path.display(),
                imported = import_result.imported_count,
                skipped = import_result.skipped_count,
                "memory archive imported via CLI"
            );
        }
        Err(e) => {
            anyhow::bail!("Import failed: {e}");
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// sweep
// ---------------------------------------------------------------------------

fn sweep(cmd: &MemorySweepCmd, store: &SqliteMemoryStore) -> Result<()> {
    let rt = tokio::runtime::Handle::current();

    let agent_id: AgentId = cmd
        .agent_id
        .parse()
        .map_err(|e| anyhow::anyhow!("Invalid agent ID '{}': {e}", cmd.agent_id))?;

    let policy = RetentionPolicy {
        max_entries_per_agent: cmd.max_entries,
        max_age_days: cmd.max_age_days,
        min_relevance: cmd.min_relevance,
        sweep_interval_secs: 3600,
    };

    if cmd.dry_run {
        // For a dry-run, query the store to show what *would* be deleted
        // without actually deleting anything.
        let result = rt.block_on(async {
            // Count entries that would be deleted by age.
            let cutoff = chrono::Utc::now()
                - chrono::Duration::days(policy.max_age_days as i64);
            let all_query = polkagent_memory::types::MemoryQuery {
                agent_id,
                query_text: String::new(),
                memory_types: None,
                limit: usize::MAX / 2,
                min_relevance: None,
                since: None,
                episode_id: None,
            };
            let all_entries = store.search(&all_query).await?;

            let by_age = all_entries
                .iter()
                .filter(|e| e.created_at < cutoff)
                .count();

            let remaining_after_age: Vec<_> = all_entries
                .iter()
                .filter(|e| e.created_at >= cutoff)
                .collect();

            let by_relevance = remaining_after_age
                .iter()
                .filter(|e| e.relevance_score < policy.min_relevance)
                .count();

            let remaining_after_rel = remaining_after_age.len() - by_relevance;

            let by_count = if remaining_after_rel > policy.max_entries_per_agent {
                remaining_after_rel - policy.max_entries_per_agent
            } else {
                0
            };

            Ok::<_, polkagent_memory::MemoryError>((by_age, by_relevance, by_count))
        });

        match result {
            Ok((by_age, by_relevance, by_count)) => {
                let total = by_age + by_relevance + by_count;
                if cmd.json {
                    let out = serde_json::json!({
                        "dry_run": true,
                        "agent_id": agent_id.to_string(),
                        "would_delete_total": total,
                        "by_age": by_age,
                        "by_relevance": by_relevance,
                        "by_count_limit": by_count,
                    });
                    println!("{}", serde_json::to_string_pretty(&out)?);
                } else {
                    println!("Retention sweep (dry-run) for agent {agent_id}:");
                    println!("{}", "-".repeat(50));
                    println!("  Would delete by age:        {by_age}");
                    println!("  Would delete by relevance:  {by_relevance}");
                    println!("  Would delete by count cap:  {by_count}");
                    println!("  Total would delete:         {total}");
                    println!();
                    println!("Run without --dry-run to apply.");
                }
            }
            Err(e) => {
                anyhow::bail!("Sweep dry-run failed: {e}");
            }
        }
    } else {
        let sweeper =
            RetentionSweeper::new(Arc::new(store.clone()), policy, agent_id);

        let result = rt.block_on(async { sweeper.sweep().await });

        match result {
            Ok(sweep_result) => {
                if cmd.json {
                    let out = serde_json::json!({
                        "dry_run": false,
                        "agent_id": agent_id.to_string(),
                        "deleted_total": sweep_result.deleted_count,
                        "by_age": sweep_result.by_age,
                        "by_relevance": sweep_result.by_relevance,
                        "by_count_limit": sweep_result.by_count_limit,
                    });
                    println!("{}", serde_json::to_string_pretty(&out)?);
                } else {
                    println!("Retention sweep complete for agent {agent_id}:");
                    println!("{}", "-".repeat(50));
                    println!("  Deleted by age:        {}", sweep_result.by_age);
                    println!("  Deleted by relevance:  {}", sweep_result.by_relevance);
                    println!("  Deleted by count cap:  {}", sweep_result.by_count_limit);
                    println!("  Total deleted:         {}", sweep_result.deleted_count);
                }

                info!(
                    agent_id = %agent_id,
                    deleted = sweep_result.deleted_count,
                    "retention sweep completed via CLI"
                );
            }
            Err(e) => {
                anyhow::bail!("Sweep failed: {e}");
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn open_memory_store() -> Result<SqliteMemoryStore> {
    let home = std::env::var("HOME").unwrap_or_default();
    let default_path = format!("{home}/.local/share/polkagent/memory.db");
    let db_path = std::env::var("POLKAGENT_MEMORY_DB_PATH").unwrap_or(default_path);
    let expanded = expand_tilde(&db_path);

    // Ensure parent directory exists.
    if let Some(parent) = std::path::Path::new(&expanded).parent() {
        std::fs::create_dir_all(parent)?;
    }

    SqliteMemoryStore::open(&expanded)
        .map_err(|e| anyhow::anyhow!("opening memory store at {expanded}: {e}"))
}

fn expand_tilde(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return format!("{home}/{rest}");
        }
    }
    path.to_owned()
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_owned()
    } else {
        format!("{}...", &s[..max.saturating_sub(3)])
    }
}
