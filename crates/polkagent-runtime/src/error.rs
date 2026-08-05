//! Runtime construction errors.

use std::path::PathBuf;

use thiserror::Error;

/// Failure while constructing a shared Polkagent runtime.
#[derive(Debug, Error)]
pub enum RuntimeError {
    /// The configured workdir is missing or is not a directory.
    #[error("runtime workdir is not a directory: {path}")]
    InvalidWorkdir {
        /// Invalid resolved path.
        path: PathBuf,
    },

    /// A configuration file could not be read.
    #[error("failed to read configuration file {path}: {source}")]
    ConfigRead {
        /// Configuration file path.
        path: PathBuf,
        /// Underlying filesystem error.
        #[source]
        source: std::io::Error,
    },

    /// Configuration loading or merging failed.
    #[error("failed to load configuration from {path}: {message}")]
    ConfigLoad {
        /// Configuration source or synthetic layer name.
        path: PathBuf,
        /// Safe parser/loader diagnostic.
        message: String,
    },

    /// The resolved configuration failed schema validation.
    #[error("configuration validation failed: {message}")]
    ConfigValidation {
        /// Joined validation diagnostics.
        message: String,
    },

    /// The selected database backend is not supported by this factory yet.
    #[error("runtime database backend is not supported yet: {backend}")]
    UnsupportedDatabase {
        /// Configured backend name.
        backend: String,
    },

    /// A durable database cannot use an in-memory path.
    #[error("durable runtime requires a file-backed SQLite database")]
    InMemoryDatabase,

    /// The database parent directory could not be created.
    #[error("failed to create database directory {path}: {source}")]
    DatabaseDirectory {
        /// Directory path.
        path: PathBuf,
        /// Underlying filesystem error.
        #[source]
        source: std::io::Error,
    },

    /// Opening or migrating `SQLite` failed.
    #[error("database initialization failed at {path}: {message}")]
    Database {
        /// Resolved database file path.
        path: PathBuf,
        /// Safe store diagnostic.
        message: String,
    },

    /// No concrete executor or harness was available under strict policy.
    #[error("no execution backend is available; configure a provider or harness")]
    NoExecutionBackend,

    /// A caller-selected provider could not be constructed.
    #[error("provider '{provider}' is unavailable: {reason}")]
    ProviderUnavailable {
        /// Requested provider identifier.
        provider: String,
        /// Safe failure reason.
        reason: String,
    },

    /// Provider registry composition failed.
    #[error("provider composition failed: {message}")]
    Provider {
        /// Safe composition diagnostic.
        message: String,
    },

    /// A caller-selected harness could not be constructed.
    #[error("harness '{harness}' is unavailable: {reason}")]
    HarnessUnavailable {
        /// Requested harness identifier.
        harness: String,
        /// Safe failure reason.
        reason: String,
    },

    /// Memory initialization failed.
    #[error("memory store initialization failed: {message}")]
    Memory {
        /// Safe memory adapter diagnostic.
        message: String,
    },

    /// Startup recovery failed.
    #[error("startup recovery failed: {message}")]
    Recovery {
        /// Safe recovery diagnostic.
        message: String,
    },

    /// Reading persisted agents failed.
    #[error("persisted agent lookup failed: {message}")]
    AgentStore {
        /// Safe store diagnostic.
        message: String,
    },

    /// A persisted active agent cannot be rehydrated safely.
    #[error("persisted agent '{agent_id}' is invalid: {message}")]
    AgentSpec {
        /// Stored agent identifier.
        agent_id: String,
        /// Safe normalization or validation diagnostic.
        message: String,
    },

    /// The service facade could not be built.
    #[error("application service composition failed: {message}")]
    Service {
        /// Safe service diagnostic.
        message: String,
    },
}
