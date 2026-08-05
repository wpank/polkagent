//! CLI subcommands for migration management.
//!
//! Uses `clap` derive macros to define the `migrate` subcommand tree.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

/// Polkagent database migration manager.
#[derive(Debug, Parser)]
#[command(name = "migrate", about = "Manage database migrations")]
pub struct MigrateCli {
    /// Path to the `SQLite` database file.
    #[arg(short, long, env = "POLKAGENT_DB_PATH")]
    pub database: PathBuf,

    /// Directory containing migration files.
    #[arg(
        short,
        long,
        env = "POLKAGENT_MIGRATIONS_DIR",
        default_value = "migrations"
    )]
    pub migrations_dir: PathBuf,

    /// The subcommand to execute.
    #[command(subcommand)]
    pub command: MigrateCommand,
}

/// Available migration subcommands.
#[derive(Debug, Subcommand)]
pub enum MigrateCommand {
    /// Apply all pending migrations.
    Run(RunArgs),

    /// Show the status of all migrations.
    Status,

    /// Verify checksums of applied migrations.
    Verify,

    /// Generate a new migration file.
    Generate(GenerateArgs),

    /// Rollback the last applied migration (if reversible).
    Rollback(RollbackArgs),
}

/// Arguments for the `migrate run` subcommand.
#[derive(Debug, Args)]
pub struct RunArgs {
    /// Show what would be applied without actually applying.
    #[arg(long)]
    pub dry_run: bool,
}

/// Arguments for the `migrate generate` subcommand.
#[derive(Debug, Args)]
pub struct GenerateArgs {
    /// Name for the new migration (e.g. "`add_users_table`").
    pub name: String,

    /// Generate a reversible migration with a down.sql file.
    #[arg(long)]
    pub reversible: bool,
}

/// Arguments for the `migrate rollback` subcommand.
#[derive(Debug, Args)]
pub struct RollbackArgs {
    /// Show what would be rolled back without actually rolling back.
    #[arg(long)]
    pub dry_run: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn parse_run_command() {
        let cli =
            MigrateCli::try_parse_from(["migrate", "--database", "test.db", "run"]).expect("parse");
        assert!(matches!(cli.command, MigrateCommand::Run(_)));
        assert_eq!(cli.database, PathBuf::from("test.db"));
    }

    #[test]
    fn parse_run_with_dry_run() {
        let cli =
            MigrateCli::try_parse_from(["migrate", "--database", "test.db", "run", "--dry-run"])
                .expect("parse");
        match cli.command {
            MigrateCommand::Run(args) => assert!(args.dry_run),
            other => panic!("expected Run, got {other:?}"),
        }
    }

    #[test]
    fn parse_status_command() {
        let cli = MigrateCli::try_parse_from(["migrate", "--database", "test.db", "status"])
            .expect("parse");
        assert!(matches!(cli.command, MigrateCommand::Status));
    }

    #[test]
    fn parse_verify_command() {
        let cli = MigrateCli::try_parse_from(["migrate", "--database", "test.db", "verify"])
            .expect("parse");
        assert!(matches!(cli.command, MigrateCommand::Verify));
    }

    #[test]
    fn parse_generate_command() {
        let cli = MigrateCli::try_parse_from([
            "migrate",
            "--database",
            "test.db",
            "generate",
            "add_users_table",
        ])
        .expect("parse");
        match cli.command {
            MigrateCommand::Generate(args) => {
                assert_eq!(args.name, "add_users_table");
                assert!(!args.reversible);
            }
            other => panic!("expected Generate, got {other:?}"),
        }
    }

    #[test]
    fn parse_generate_reversible() {
        let cli = MigrateCli::try_parse_from([
            "migrate",
            "--database",
            "test.db",
            "generate",
            "add_users",
            "--reversible",
        ])
        .expect("parse");
        match cli.command {
            MigrateCommand::Generate(args) => assert!(args.reversible),
            other => panic!("expected Generate, got {other:?}"),
        }
    }

    #[test]
    fn parse_rollback_command() {
        let cli = MigrateCli::try_parse_from(["migrate", "--database", "test.db", "rollback"])
            .expect("parse");
        assert!(matches!(cli.command, MigrateCommand::Rollback(_)));
    }

    #[test]
    fn parse_rollback_dry_run() {
        let cli = MigrateCli::try_parse_from([
            "migrate",
            "--database",
            "test.db",
            "rollback",
            "--dry-run",
        ])
        .expect("parse");
        match cli.command {
            MigrateCommand::Rollback(args) => assert!(args.dry_run),
            other => panic!("expected Rollback, got {other:?}"),
        }
    }

    #[test]
    fn parse_custom_migrations_dir() {
        let cli = MigrateCli::try_parse_from([
            "migrate",
            "--database",
            "test.db",
            "--migrations-dir",
            "/custom/path",
            "status",
        ])
        .expect("parse");
        assert_eq!(cli.migrations_dir, PathBuf::from("/custom/path"));
    }

    #[test]
    fn default_migrations_dir() {
        let cli = MigrateCli::try_parse_from(["migrate", "--database", "test.db", "status"])
            .expect("parse");
        assert_eq!(cli.migrations_dir, PathBuf::from("migrations"));
    }

    #[test]
    fn missing_database_fails() {
        let result = MigrateCli::try_parse_from(["migrate", "status"]);
        assert!(result.is_err());
    }

    #[test]
    fn missing_subcommand_fails() {
        let result = MigrateCli::try_parse_from(["migrate", "--database", "test.db"]);
        assert!(result.is_err());
    }
}
