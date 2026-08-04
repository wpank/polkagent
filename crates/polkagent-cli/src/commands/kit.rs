//! `polkagent kit` — product kit management subcommands.
//!
//! Product kits bundle multiple skills into a single installable unit.
//! The registry is persisted in the main SQLite database under the `kits`
//! table.

use anyhow::Result;
use tracing::info;

use polkagent_store_sqlite::SqlitePool;

use crate::cli::{KitCmd, KitInstallCmd, KitListCmd, KitUninstallCmd};

/// Ensure the `kits` table exists (self-healing for fresh / older databases).
fn ensure_kits_table(pool: &SqlitePool) -> Result<()> {
    let writer = pool.writer();
    writer.execute_batch(
        "CREATE TABLE IF NOT EXISTS kits (
            name          TEXT PRIMARY KEY,
            version       TEXT NOT NULL DEFAULT '0.1.0',
            description   TEXT NOT NULL DEFAULT '',
            path          TEXT NOT NULL,
            skill_names   TEXT NOT NULL DEFAULT '[]',
            manifest_json TEXT NOT NULL DEFAULT '{}',
            installed_at  TEXT NOT NULL,
            updated_at    TEXT NOT NULL
        )",
    )?;
    Ok(())
}

/// Dispatch the kit subcommand.
pub fn run(cmd: &KitCmd, pool: &SqlitePool) -> Result<()> {
    ensure_kits_table(pool)?;
    match cmd {
        KitCmd::Install(c) => install(c, pool),
        KitCmd::Uninstall(c) => uninstall(c, pool),
        KitCmd::List(c) => list(c, pool),
    }
}

// ---------------------------------------------------------------------------
// install
// ---------------------------------------------------------------------------

fn install(cmd: &KitInstallCmd, pool: &SqlitePool) -> Result<()> {
    let path = cmd
        .path
        .canonicalize()
        .map_err(|e| anyhow::anyhow!("Cannot access kit path '{}': {e}", cmd.path.display()))?;

    // Validate the kit manifest. For now, grant all capabilities (the
    // grant check is advisory) and do not require pre-installed skills.
    let granted: Vec<String> = vec![
        "chain.query".into(),
        "chain.submit".into(),
        "memory.read".into(),
        "fs.read".into(),
        "fs.write".into(),
        "network.http".into(),
        "tool.execute".into(),
    ];

    // Collect already-installed skill names from the skills table so the
    // validator can check whether required skills are present.
    let available_skills = read_installed_skill_names(pool);

    let validation = polkagent_kit::validate_kit(&path, &granted, &available_skills)
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    if !validation.denied_capabilities.is_empty() {
        eprintln!(
            "Warning: kit requires capabilities not currently granted: {:?}",
            validation.denied_capabilities
        );
    }

    if !validation.missing_skills.is_empty() {
        eprintln!(
            "Warning: kit references skills not yet installed: {:?}",
            validation.missing_skills
        );
        eprintln!("These skills will need to be installed separately.");
    }

    let installed = polkagent_kit::prepare_install(&validation.manifest, &validation.path)
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    let skill_names_json = serde_json::to_string(&installed.skill_names)?;
    let now = chrono::Utc::now().to_rfc3339();

    let writer = pool.writer();
    let result = writer.execute(
        "INSERT INTO kits (name, version, description, path, skill_names, manifest_json, installed_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(name) DO UPDATE SET
           version = excluded.version,
           description = excluded.description,
           path = excluded.path,
           skill_names = excluded.skill_names,
           manifest_json = excluded.manifest_json,
           updated_at = excluded.updated_at",
        rusqlite::params![
            installed.name,
            installed.version,
            installed.description,
            installed.path,
            skill_names_json,
            installed.manifest_json,
            now,
            now,
        ],
    );

    match result {
        Ok(_) => {
            if cmd.json {
                let out = serde_json::json!({
                    "name": installed.name,
                    "version": installed.version,
                    "description": installed.description,
                    "path": installed.path,
                    "skills": installed.skill_names,
                    "status": "installed",
                });
                println!("{}", serde_json::to_string_pretty(&out)?);
            } else {
                println!("Kit installed:");
                println!("  Name:        {}", installed.name);
                println!("  Version:     {}", installed.version);
                println!("  Description: {}", installed.description);
                println!("  Path:        {}", installed.path);
                println!("  Skills:      {}", installed.skill_names.join(", "));
            }
            info!(kit = %installed.name, version = %installed.version, "kit installed");
        }
        Err(e) => {
            eprintln!("Warning: could not persist kit to database: {e}");
            eprintln!("The kits table may not exist — run `polkagent init` to create it.");
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// uninstall
// ---------------------------------------------------------------------------

fn uninstall(cmd: &KitUninstallCmd, pool: &SqlitePool) -> Result<()> {
    // Look up the kit to get its skill list before removal.
    let reader = pool.reader().map_err(|e| anyhow::anyhow!("{e}"))?;

    let row: Option<(String, String)> = match reader.query_row(
        "SELECT name, skill_names FROM kits WHERE name = ?1",
        rusqlite::params![cmd.name],
        |row| Ok((row.get(0)?, row.get(1)?)),
    ) {
        Ok(r) => Some(r),
        Err(e) if e.to_string().contains("no such table") => {
            eprintln!("No kits installed. Use 'polkagent kit install' first.");
            return Ok(());
        }
        Err(_) => None,
    };

    let Some((_name, skill_names_json)) = row else {
        anyhow::bail!("Kit '{}' is not installed.", cmd.name);
    };

    if !cmd.yes {
        use std::io::{self, Write};
        print!("Uninstall kit '{}'? [y/N] ", cmd.name);
        io::stdout().flush()?;
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        if !input.trim().eq_ignore_ascii_case("y") {
            println!("Aborted.");
            return Ok(());
        }
    }

    // Parse the skill names that were registered with this kit.
    let skill_names: Vec<String> = serde_json::from_str(&skill_names_json).unwrap_or_default();

    // Remove from the kits table.
    let writer = pool.writer();
    let rows = writer.execute(
        "DELETE FROM kits WHERE name = ?1",
        rusqlite::params![cmd.name],
    );

    match rows {
        Ok(0) => {
            anyhow::bail!("Kit '{}' is not installed.", cmd.name);
        }
        Ok(_) => {
            // Also remove any skills that were registered as part of this kit.
            let mut removed_skills: Vec<String> = Vec::new();
            for skill_name in &skill_names {
                let deleted = writer.execute(
                    "DELETE FROM skills WHERE name = ?1",
                    rusqlite::params![skill_name],
                );
                if matches!(deleted, Ok(n) if n > 0) {
                    removed_skills.push(skill_name.clone());
                }
            }

            println!("Kit '{}' uninstalled.", cmd.name);
            if !removed_skills.is_empty() {
                println!("  Removed skills: {}", removed_skills.join(", "));
            }
            info!(kit = %cmd.name, "kit uninstalled");
        }
        Err(e) if e.to_string().contains("no such table") => {
            eprintln!("No kits installed. Use 'polkagent kit install' first.");
        }
        Err(e) => {
            eprintln!("Warning: {e}");
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// list
// ---------------------------------------------------------------------------

fn list(cmd: &KitListCmd, pool: &SqlitePool) -> Result<()> {
    let reader = match pool.reader() {
        Ok(r) => r,
        Err(e) => {
            if cmd.json {
                println!("[]");
            } else {
                eprintln!("Cannot read database: {e}");
                eprintln!("Run `polkagent init` to initialise the database.");
            }
            return Ok(());
        }
    };

    let rows: Vec<(String, String, String, String, String)> = reader
        .prepare(
            "SELECT name, version, description, skill_names, installed_at
             FROM kits
             ORDER BY name ASC",
        )
        .and_then(|mut stmt| {
            stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            })
            .and_then(|iter| iter.collect::<Result<Vec<_>, _>>())
        })
        .unwrap_or_default();

    if cmd.json {
        let items: Vec<serde_json::Value> = rows
            .iter()
            .map(|(name, version, desc, skills_json, installed)| {
                let skills: Vec<String> = serde_json::from_str(skills_json).unwrap_or_default();
                serde_json::json!({
                    "name": name,
                    "version": version,
                    "description": desc,
                    "skills": skills,
                    "installed_at": installed,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&items)?);
        return Ok(());
    }

    if rows.is_empty() {
        println!("No kits installed.");
        println!("Use `polkagent kit install <path>` to add a product kit.");
        return Ok(());
    }

    println!("{:<28}  {:<12}  Skills  Description", "Name", "Version");
    println!("{}", "-".repeat(90));
    for (name, version, desc, skills_json, _installed) in &rows {
        let skills: Vec<String> = serde_json::from_str(skills_json).unwrap_or_default();
        println!("{name:<28}  {version:<12}  {:<6}  {desc}", skills.len());
    }
    println!();
    println!("{} kit(s) installed", rows.len());

    Ok(())
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

/// Read installed skill names from the skills table (best effort).
fn read_installed_skill_names(pool: &SqlitePool) -> Vec<String> {
    let reader = match pool.reader() {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };

    reader
        .prepare("SELECT name FROM skills ORDER BY name ASC")
        .and_then(|mut stmt| {
            stmt.query_map([], |row| row.get::<_, String>(0))
                .and_then(|iter| iter.collect::<Result<Vec<_>, _>>())
        })
        .unwrap_or_default()
}
