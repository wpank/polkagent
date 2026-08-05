//! Standard exit codes for the `polkagent` CLI.
//!
//! Use these constants with `std::process::exit` to communicate structured
//! outcomes to shell scripts and CI pipelines.
//!
//! All codes are wired into `main::classify_exit_code()` which inspects
//! error messages to select the appropriate code.

/// Command completed successfully.
pub const SUCCESS: i32 = 0;

/// General / unclassified error.
pub const ERROR: i32 = 1;

/// Human approval is required before the operation can proceed.
pub const APPROVAL_REQUIRED: i32 = 2;

/// The operation was denied by a policy check.
pub const POLICY_DENIED: i32 = 3;

/// Configuration file is missing, invalid, or cannot be read.
pub const CONFIG_ERROR: i32 = 4;

/// A required network connection could not be established.
pub const NETWORK_ERROR: i32 = 5;

/// The outcome of the operation is unknown (e.g. daemon unreachable after submit).
pub const UNKNOWN_OUTCOME: i32 = 10;
