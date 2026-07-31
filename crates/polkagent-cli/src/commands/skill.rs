//! `polkagent skill` — skill management subcommands.
//!
//! Skills are standalone capability modules that extend agent behaviour.
//! The registry is persisted in the main SQLite database under the `skills`
//! table.  When the table does not yet exist (e.g. on a fresh install), all
//! commands degrade gracefully and advise the user to run `polkagent init`.

use anyhow::Result;
use tracing::info;

use polkagent_store_sqlite::SqlitePool;

use crate::cli::{
    SkillCmd, SkillInstallCmd, SkillListCmd, SkillRemoveCmd, SkillShowCmd, SkillUpdateCmd,
};

/// Dispatch the skill subcommand.
pub fn run(cmd: &SkillCmd, pool: &SqlitePool) -> Result<()> {
    match cmd {
        SkillCmd::List(c)    => list(c, pool),
        SkillCmd::Install(c) => install(c, pool),
        SkillCmd::Update(c)  => update(c, pool),
        SkillCmd::Remove(c)  => remove(c, pool),
        SkillCmd::Show(c)    => show(c, pool),
    }
}

// ---------------------------------------------------------------------------
// list
// ---------------------------------------------------------------------------

fn list(cmd: &SkillListCmd, pool: &SqlitePool) -> Result<()> {
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

    // Skills table may not exist on older databases.
    let rows: Vec<(String, String, String, String)> = reader
        .prepare(
            "SELECT name, version, description, installed_at
             FROM skills
             ORDER BY name ASC",
        )
        .and_then(|mut stmt| {
            stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })
            .and_then(|iter| iter.collect::<Result<Vec<_>, _>>())
        })
        .unwrap_or_default();

    if cmd.json {
        let items: Vec<serde_json::Value> = rows
            .iter()
            .map(|(name, version, desc, installed)| {
                serde_json::json!({
                    "name": name,
                    "version": version,
                    "description": desc,
                    "installed_at": installed,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&items)?);
        return Ok(());
    }

    if rows.is_empty() {
        println!("No skills installed.");
        println!("Use `polkagent skill install <path>` to add a skill.");
        return Ok(());
    }

    println!("{:<24}  {:<12}  Description", "Name", "Version");
    println!("{}", "-".repeat(80));
    for (name, version, desc, _installed) in &rows {
        println!("{name:<24}  {version:<12}  {desc}");
    }
    println!();
    println!("{} skill(s) installed", rows.len());

    Ok(())
}

// ---------------------------------------------------------------------------
// install
// ---------------------------------------------------------------------------

fn install(cmd: &SkillInstallCmd, pool: &SqlitePool) -> Result<()> {
    // Resolve and validate the path.
    let path = cmd.path.canonicalize().map_err(|e| {
        anyhow::anyhow!("Cannot access skill path '{}': {e}", cmd.path.display())
    })?;

    // Look for a manifest file.
    let manifest_path = if path.is_dir() {
        let candidate = path.join("skill.toml");
        if !candidate.exists() {
            anyhow::bail!(
                "No skill.toml found in '{}'. A skill directory must contain a skill.toml manifest.",
                path.display()
            );
        }
        candidate
    } else {
        path.clone()
    };

    let manifest_content = std::fs::read_to_string(&manifest_path)
        .map_err(|e| anyhow::anyhow!("Cannot read manifest '{}': {e}", manifest_path.display()))?;

    // Parse as TOML and extract required fields.
    let manifest: toml::Value = toml::from_str(&manifest_content)
        .map_err(|e| anyhow::anyhow!("Invalid skill manifest: {e}"))?;

    let name = manifest
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("skill.toml is missing required field: name"))?
        .to_owned();

    let version = manifest
        .get("version")
        .and_then(|v| v.as_str())
        .unwrap_or("0.1.0")
        .to_owned();

    let description = manifest
        .get("description")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_owned();

    let manifest_json = serde_json::to_string(&manifest_content)?;
    let path_str = path.to_string_lossy().to_string();
    let now = chrono::Utc::now().to_rfc3339();

    // Attempt to upsert into the skills table (graceful if table missing).
    let writer = pool.writer();
    let result = writer.execute(
        "INSERT INTO skills (name, version, description, path, manifest_json, installed_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(name) DO UPDATE SET
           version = excluded.version,
           description = excluded.description,
           path = excluded.path,
           manifest_json = excluded.manifest_json,
           updated_at = excluded.updated_at",
        rusqlite::params![name, version, description, path_str, manifest_json, now, now],
    );

    match result {
        Ok(_) => {
            if cmd.json {
                let out = serde_json::json!({
                    "name": name,
                    "version": version,
                    "description": description,
                    "path": path_str,
                    "status": "installed",
                });
                println!("{}", serde_json::to_string_pretty(&out)?);
            } else {
                println!("Skill installed:");
                println!("  Name:        {name}");
                println!("  Version:     {version}");
                println!("  Description: {description}");
                println!("  Path:        {path_str}");
            }
            info!(skill = %name, version = %version, "skill installed");
        }
        Err(e) => {
            eprintln!("Warning: could not persist skill to database: {e}");
            eprintln!("The skills table may not exist — run `polkagent init` to create it.");
            println!("Skill manifest validated (name={name}, version={version}).");
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// update
// ---------------------------------------------------------------------------

fn update(cmd: &SkillUpdateCmd, pool: &SqlitePool) -> Result<()> {
    let reader = pool.reader().map_err(|e| anyhow::anyhow!("{e}"))?;

    let row: Option<(String, String)> = reader
        .query_row(
            "SELECT path, version FROM skills WHERE name = ?1",
            rusqlite::params![cmd.name],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map(Some)
        .unwrap_or(None);

    let Some((path_str, old_version)) = row else {
        anyhow::bail!("Skill '{}' is not installed.", cmd.name);
    };

    // Re-read and re-validate the manifest from the stored path.
    let path = std::path::PathBuf::from(&path_str);
    let manifest_path = if path.is_dir() {
        path.join("skill.toml")
    } else {
        path.clone()
    };

    let manifest_content = std::fs::read_to_string(&manifest_path).map_err(|e| {
        anyhow::anyhow!(
            "Cannot read manifest '{}': {e}. The skill path may have moved.",
            manifest_path.display()
        )
    })?;

    let manifest: toml::Value = toml::from_str(&manifest_content)
        .map_err(|e| anyhow::anyhow!("Invalid skill manifest: {e}"))?;

    let new_version = manifest
        .get("version")
        .and_then(|v| v.as_str())
        .unwrap_or("0.1.0")
        .to_owned();
    let description = manifest
        .get("description")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_owned();
    let manifest_json = serde_json::to_string(&manifest_content)?;
    let now = chrono::Utc::now().to_rfc3339();

    let writer = pool.writer();
    writer.execute(
        "UPDATE skills SET version = ?1, description = ?2, manifest_json = ?3, updated_at = ?4 WHERE name = ?5",
        rusqlite::params![new_version, description, manifest_json, now, cmd.name],
    )?;

    if cmd.json {
        let out = serde_json::json!({
            "name": cmd.name,
            "old_version": old_version,
            "new_version": new_version,
            "status": "updated",
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else {
        println!("Skill '{}' updated: {} -> {}", cmd.name, old_version, new_version);
    }

    info!(skill = %cmd.name, from = %old_version, to = %new_version, "skill updated");
    Ok(())
}

// ---------------------------------------------------------------------------
// remove
// ---------------------------------------------------------------------------

fn remove(cmd: &SkillRemoveCmd, pool: &SqlitePool) -> Result<()> {
    if !cmd.yes {
        use std::io::{self, Write};
        print!("Remove skill '{}'? [y/N] ", cmd.name);
        io::stdout().flush()?;
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        if !input.trim().eq_ignore_ascii_case("y") {
            println!("Aborted.");
            return Ok(());
        }
    }

    let writer = pool.writer();
    let rows = writer.execute(
        "DELETE FROM skills WHERE name = ?1",
        rusqlite::params![cmd.name],
    );

    match rows {
        Ok(0) => {
            anyhow::bail!("Skill '{}' is not installed.", cmd.name);
        }
        Ok(_) => {
            println!("Skill '{}' removed.", cmd.name);
            info!(skill = %cmd.name, "skill removed");
        }
        Err(e) => {
            eprintln!("Warning: {e}");
            eprintln!("The skills table may not exist — run `polkagent init`.");
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// show
// ---------------------------------------------------------------------------

fn show(cmd: &SkillShowCmd, pool: &SqlitePool) -> Result<()> {
    let reader = pool.reader().map_err(|e| anyhow::anyhow!("{e}"))?;

    let row: Option<(String, String, String, String, String, String)> = reader
        .query_row(
            "SELECT name, version, description, path, manifest_json, installed_at
             FROM skills WHERE name = ?1",
            rusqlite::params![cmd.name],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            },
        )
        .map(Some)
        .unwrap_or(None);

    let Some((name, version, description, path, manifest_json, installed_at)) = row else {
        anyhow::bail!("Skill '{}' is not installed.", cmd.name);
    };

    if cmd.json {
        let manifest: serde_json::Value =
            serde_json::from_str(&manifest_json).unwrap_or(serde_json::Value::Null);
        let out = serde_json::json!({
            "name": name,
            "version": version,
            "description": description,
            "path": path,
            "manifest": manifest,
            "installed_at": installed_at,
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else {
        println!("Skill: {name}");
        println!("  Version:      {version}");
        println!("  Description:  {description}");
        println!("  Path:         {path}");
        println!("  Installed at: {installed_at}");
    }

    Ok(())
}
