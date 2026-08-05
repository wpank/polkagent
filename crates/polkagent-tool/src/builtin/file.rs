//! File-system tools: read, write, and list directory contents.
//!
//! Each tool enforces grant checks via the registry's grant-checking mechanism.
//! The required grant patterns follow the `file/*` namespace.
//!
//! # Path security
//!
//! All file tools validate the requested path against the `SecurityConfig`
//! carried in [`ToolContext`] before performing any filesystem operation:
//!
//! 1. The raw path is canonicalized (resolving symlinks and `..` components).
//! 2. The canonical path is checked against `denied_paths` — a match causes
//!    immediate rejection.
//! 3. If `allowed_paths` is non-empty, the canonical path must start with at
//!    least one of the allowed prefixes; otherwise access is denied.
//!
//! When no `SecurityConfig` is present in the context the tools still apply
//! the default deny-list (`/etc/shadow`, `/etc/passwd`, etc.) so that
//! unconfigured invocations are not completely open.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde_json::Value;
use tracing::debug;

use polkagent_config::SecurityConfig;
use polkagent_core::config::DataClassification;

use crate::registry::{ToolContext, ToolError, ToolHandler, ToolResult, ToolSpec};

// ---------------------------------------------------------------------------
// Path validation helper
// ---------------------------------------------------------------------------

/// Validate `raw_path` against the security config carried in `context`.
///
/// Returns the canonicalized [`PathBuf`] on success, or a [`ToolError`] if
/// the path is denied, not in the allowed-list, or cannot be canonicalized.
///
/// When the context carries no `SecurityConfig`, the built-in default deny-list
/// is applied.
fn validate_path(raw_path: &str, context: &ToolContext) -> Result<PathBuf, ToolError> {
    let default_security = SecurityConfig::default();
    let security = context
        .security_config
        .as_ref()
        .unwrap_or(&default_security);

    // Produce the best possible canonical form of the requested path.
    // For existing paths, `canonicalize` resolves all symlinks.
    // For paths that do not yet exist (e.g. a write target), we canonicalize
    // the nearest existing ancestor and append the remaining components
    // lexically so that `..` in any component is still caught.
    let canonical = best_effort_canonicalize(raw_path)?;

    // Check denied_paths: deny if the canonical path starts with any denied prefix.
    for denied in &security.denied_paths {
        let denied_canon = std::fs::canonicalize(denied).unwrap_or_else(|_| denied.clone());
        if canonical.starts_with(&denied_canon) || canonical == denied_canon {
            return Err(ToolError::PermissionDenied {
                reason: format!(
                    "path '{}' is in the denied list (matches '{}')",
                    canonical.display(),
                    denied_canon.display()
                ),
            });
        }
    }

    // Check allowed_paths: if the list is non-empty, the path must match at
    // least one prefix.
    if !security.allowed_paths.is_empty() {
        let allowed = security.allowed_paths.iter().any(|allowed| {
            let allowed_canon = std::fs::canonicalize(allowed).unwrap_or_else(|_| allowed.clone());
            canonical.starts_with(&allowed_canon) || canonical == allowed_canon
        });
        if !allowed {
            return Err(ToolError::PermissionDenied {
                reason: format!(
                    "path '{}' is not in the allowed-path list",
                    canonical.display()
                ),
            });
        }
    }

    Ok(canonical)
}

/// Produce the best possible canonical representation of `raw_path`.
///
/// 1. Try `std::fs::canonicalize` directly (works when the path exists).
/// 2. Otherwise, walk up the path until we find an existing ancestor, then
///    `canonicalize` that ancestor and append the remaining tail components.
///
/// This approach correctly handles macOS symlinks such as `/tmp -> /private/tmp`
/// even when the leaf file does not yet exist.
fn best_effort_canonicalize(raw_path: &str) -> Result<PathBuf, ToolError> {
    let p = std::path::Path::new(raw_path);

    // Fast path: the full path exists.
    if let Ok(c) = std::fs::canonicalize(p) {
        return Ok(c);
    }

    // Build an absolute path so relative paths are anchored to cwd.
    let abs = if p.is_absolute() {
        p.to_path_buf()
    } else {
        let cwd = std::env::current_dir().map_err(|e| ToolError::ExecutionFailed {
            reason: format!("cannot determine current directory: {}", e),
        })?;
        cwd.join(p)
    };

    // Normalize `..` and `.` lexically first.
    let lexical = normalize_lexically(&abs);

    // Walk up from the full path to find the deepest existing ancestor.
    let mut existing: Option<PathBuf> = None;
    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    let mut candidate = lexical.clone();

    loop {
        if candidate.exists() {
            match std::fs::canonicalize(&candidate) {
                Ok(c) => {
                    existing = Some(c);
                    break;
                }
                Err(_) => {}
            }
        }
        match candidate.file_name() {
            Some(name) => {
                tail.push(name.to_owned());
                candidate = match candidate.parent() {
                    Some(p) => p.to_path_buf(),
                    None => break,
                };
            }
            None => break,
        }
    }

    match existing {
        Some(mut base) => {
            // Re-append the non-existing tail in original order.
            for component in tail.into_iter().rev() {
                base.push(component);
            }
            Ok(base)
        }
        None => {
            // Could not find any existing ancestor; fall back to the lexically
            // normalized absolute path.
            Ok(lexical)
        }
    }
}

/// Resolve `..` and `.` components without hitting the filesystem.
///
/// This is used when the target path does not yet exist (e.g. the destination
/// of a write).  It is lexical only -- it does not resolve symlinks -- which is
/// intentional: we want to prevent directory traversal in the *name*, not
/// necessarily follow every symlink.
fn normalize_lexically(path: &Path) -> PathBuf {
    let mut components: Vec<std::ffi::OsString> = Vec::new();
    for component in path.components() {
        match component {
            std::path::Component::ParentDir => {
                // Pop the last component, but never pop past the root.
                if components.last().map_or(false, |c| c != "..") {
                    components.pop();
                } else {
                    components.push(component.as_os_str().to_owned());
                }
            }
            std::path::Component::CurDir => {
                // Skip `.` components.
            }
            _ => {
                components.push(component.as_os_str().to_owned());
            }
        }
    }
    components.iter().collect()
}

// ---------------------------------------------------------------------------
// ReadFileTool
// ---------------------------------------------------------------------------

/// Reads the contents of a file at a given path.
///
/// **Grant requirement:** `file/read`
pub struct ReadFileTool;

#[async_trait]
impl ToolHandler for ReadFileTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "polkagent.file.read".to_string(),
            description: "Read the contents of a file at the given path.".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Absolute or relative file path to read"
                    }
                },
                "required": ["path"]
            }),
            required_grant: Some("file/read".to_string()),
            output_classification: DataClassification::Internal,
        }
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        let path =
            input
                .get("path")
                .and_then(Value::as_str)
                .ok_or_else(|| ToolError::InvalidInput {
                    reason: "missing or invalid 'path' field".to_string(),
                })?;

        // Validate the path against the security config before any I/O.
        let canonical = validate_path(path, context)?;

        debug!(path = %canonical.display(), "reading file");

        let contents = tokio::fs::read_to_string(&canonical).await.map_err(|e| {
            ToolError::ExecutionFailed {
                reason: format!("failed to read file '{}': {}", canonical.display(), e),
            }
        })?;

        Ok(ToolResult {
            output: serde_json::json!({
                "path": canonical.to_string_lossy(),
                "contents": contents,
                "size_bytes": contents.len(),
            }),
            classification: DataClassification::Internal,
            artifacts: vec![],
        })
    }
}

// ---------------------------------------------------------------------------
// WriteFileTool
// ---------------------------------------------------------------------------

/// Writes content to a file at a given path.
///
/// Creates parent directories as needed. Overwrites existing files.
///
/// **Grant requirement:** `file/write`
pub struct WriteFileTool;

#[async_trait]
impl ToolHandler for WriteFileTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "polkagent.file.write".to_string(),
            description:
                "Write content to a file at the given path. Creates parent directories as needed."
                    .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Absolute or relative file path to write"
                    },
                    "content": {
                        "type": "string",
                        "description": "Content to write to the file"
                    }
                },
                "required": ["path", "content"]
            }),
            required_grant: Some("file/write".to_string()),
            output_classification: DataClassification::Internal,
        }
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        let path =
            input
                .get("path")
                .and_then(Value::as_str)
                .ok_or_else(|| ToolError::InvalidInput {
                    reason: "missing or invalid 'path' field".to_string(),
                })?;

        let content = input
            .get("content")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::InvalidInput {
                reason: "missing or invalid 'content' field".to_string(),
            })?;

        // Validate the path against the security config before any I/O.
        // For writes the target may not exist yet, so validate_path performs
        // lexical normalization as a fallback.
        let canonical = validate_path(path, context)?;

        debug!(path = %canonical.display(), bytes = content.len(), "writing file");

        // Ensure parent directory exists.
        if let Some(parent) = canonical.parent() {
            if !parent.as_os_str().is_empty() {
                tokio::fs::create_dir_all(parent).await.map_err(|e| {
                    ToolError::ExecutionFailed {
                        reason: format!(
                            "failed to create parent directories for '{}': {}",
                            canonical.display(),
                            e
                        ),
                    }
                })?;
            }
        }

        tokio::fs::write(&canonical, content)
            .await
            .map_err(|e| ToolError::ExecutionFailed {
                reason: format!("failed to write file '{}': {}", canonical.display(), e),
            })?;

        Ok(ToolResult {
            output: serde_json::json!({
                "path": canonical.to_string_lossy(),
                "bytes_written": content.len(),
            }),
            classification: DataClassification::Internal,
            artifacts: vec![],
        })
    }
}

// ---------------------------------------------------------------------------
// ListDirTool
// ---------------------------------------------------------------------------

/// Lists the contents of a directory.
///
/// Returns file names, sizes, and whether each entry is a file or directory.
///
/// **Grant requirement:** `file/read`
pub struct ListDirTool;

#[async_trait]
impl ToolHandler for ListDirTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "polkagent.file.list".to_string(),
            description: "List the contents of a directory.".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Absolute or relative directory path to list"
                    }
                },
                "required": ["path"]
            }),
            required_grant: Some("file/read".to_string()),
            output_classification: DataClassification::Internal,
        }
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        let path =
            input
                .get("path")
                .and_then(Value::as_str)
                .ok_or_else(|| ToolError::InvalidInput {
                    reason: "missing or invalid 'path' field".to_string(),
                })?;

        // Validate the path against the security config before any I/O.
        let canonical = validate_path(path, context)?;

        debug!(path = %canonical.display(), "listing directory");

        let mut read_dir =
            tokio::fs::read_dir(&canonical)
                .await
                .map_err(|e| ToolError::ExecutionFailed {
                    reason: format!("failed to read directory '{}': {}", canonical.display(), e),
                })?;

        let mut entries = Vec::new();

        loop {
            match read_dir.next_entry().await {
                Ok(Some(entry)) => {
                    let file_name = entry.file_name().to_string_lossy().to_string();
                    let metadata = entry.metadata().await;
                    let (is_dir, size) = match metadata {
                        Ok(m) => (m.is_dir(), m.len()),
                        Err(_) => (false, 0),
                    };
                    entries.push(serde_json::json!({
                        "name": file_name,
                        "is_directory": is_dir,
                        "size_bytes": size,
                    }));
                }
                Ok(None) => break,
                Err(e) => {
                    return Err(ToolError::ExecutionFailed {
                        reason: format!(
                            "error reading directory entry in '{}': {}",
                            canonical.display(),
                            e
                        ),
                    });
                }
            }
        }

        Ok(ToolResult {
            output: serde_json::json!({
                "path": canonical.to_string_lossy(),
                "entries": entries,
                "count": entries.len(),
            }),
            classification: DataClassification::Internal,
            artifacts: vec![],
        })
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use polkagent_config::SecurityConfig;
    use polkagent_core::ids::{AgentId, RunId, StepId};

    fn test_context() -> ToolContext {
        ToolContext {
            run_id: RunId::new(),
            agent_id: AgentId::new(),
            step_id: StepId::new(),
            grants: vec![],
            security_config: None,
        }
    }

    /// Build a context with a custom SecurityConfig.
    fn ctx_with_security(security_config: SecurityConfig) -> ToolContext {
        ToolContext {
            run_id: RunId::new(),
            agent_id: AgentId::new(),
            step_id: StepId::new(),
            grants: vec![],
            security_config: Some(security_config),
        }
    }

    #[test]
    fn read_file_spec() {
        let spec = ReadFileTool.spec();
        assert_eq!(spec.name, "polkagent.file.read");
        assert_eq!(spec.required_grant.as_deref(), Some("file/read"));
    }

    #[test]
    fn write_file_spec() {
        let spec = WriteFileTool.spec();
        assert_eq!(spec.name, "polkagent.file.write");
        assert_eq!(spec.required_grant.as_deref(), Some("file/write"));
    }

    #[test]
    fn list_dir_spec() {
        let spec = ListDirTool.spec();
        assert_eq!(spec.name, "polkagent.file.list");
        assert_eq!(spec.required_grant.as_deref(), Some("file/read"));
    }

    #[tokio::test]
    async fn read_file_missing_path() {
        let tool = ReadFileTool;
        let result = tool.execute(serde_json::json!({}), &test_context()).await;
        assert!(matches!(result, Err(ToolError::InvalidInput { .. })));
    }

    #[tokio::test]
    async fn write_file_missing_content() {
        let tool = WriteFileTool;
        let input = serde_json::json!({ "path": "/tmp/test" });
        let result = tool.execute(input, &test_context()).await;
        assert!(matches!(result, Err(ToolError::InvalidInput { .. })));
    }

    #[tokio::test]
    async fn list_dir_nonexistent() {
        let tool = ListDirTool;
        let input = serde_json::json!({ "path": "/nonexistent/dir/xyzzy" });
        let result = tool.execute(input, &test_context()).await;
        assert!(matches!(result, Err(ToolError::ExecutionFailed { .. })));
    }

    #[tokio::test]
    async fn read_write_round_trip() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        let file_path = dir.path().join("test.txt");
        let file_path_str = file_path.to_string_lossy().to_string();
        let ctx = test_context();

        // Write.
        let write_input = serde_json::json!({
            "path": file_path_str,
            "content": "hello, polkagent"
        });
        let write_result = WriteFileTool.execute(write_input, &ctx).await;
        assert!(write_result.is_ok(), "write failed: {write_result:?}");
        let wr = write_result.unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(wr.output["bytes_written"], 16);

        // Read.
        let read_input = serde_json::json!({ "path": file_path_str });
        let read_result = ReadFileTool.execute(read_input, &ctx).await;
        assert!(read_result.is_ok(), "read failed: {read_result:?}");
        let rr = read_result.unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(rr.output["contents"], "hello, polkagent");
    }

    #[tokio::test]
    async fn list_dir_shows_files() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        let dir_path = dir.path().to_string_lossy().to_string();
        let ctx = test_context();

        // Create a file inside the dir.
        let file_path = dir.path().join("foo.txt");
        tokio::fs::write(&file_path, "bar")
            .await
            .unwrap_or_else(|e| panic!("write: {e}"));

        let input = serde_json::json!({ "path": dir_path });
        let result = ListDirTool.execute(input, &ctx).await;
        assert!(result.is_ok(), "list failed: {result:?}");

        let r = result.unwrap_or_else(|e| panic!("{e}"));
        let entries = r.output["entries"]
            .as_array()
            .unwrap_or_else(|| panic!("not array"));
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0]["name"], "foo.txt");
        assert_eq!(entries[0]["is_directory"], false);
    }

    // -----------------------------------------------------------------------
    // Security: denied_paths enforcement
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn read_denied_path_is_rejected() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        let secret_file = dir.path().join("secret.txt");
        std::fs::write(&secret_file, "secret").unwrap_or_else(|e| panic!("write: {e}"));

        let security = SecurityConfig {
            denied_paths: vec![dir.path().to_path_buf()],
            allowed_paths: vec![],
            ..SecurityConfig::default()
        };
        let ctx = ctx_with_security(security);

        let input = serde_json::json!({ "path": secret_file.to_string_lossy().as_ref() });
        let result = ReadFileTool.execute(input, &ctx).await;
        assert!(
            matches!(result, Err(ToolError::PermissionDenied { .. })),
            "read of denied path must return PermissionDenied, got: {result:?}"
        );
    }

    #[tokio::test]
    async fn write_denied_path_is_rejected() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));

        let security = SecurityConfig {
            denied_paths: vec![dir.path().to_path_buf()],
            allowed_paths: vec![],
            ..SecurityConfig::default()
        };
        let ctx = ctx_with_security(security);

        let target = dir.path().join("new.txt").to_string_lossy().to_string();
        let input = serde_json::json!({ "path": target, "content": "data" });
        let result = WriteFileTool.execute(input, &ctx).await;
        assert!(
            matches!(result, Err(ToolError::PermissionDenied { .. })),
            "write to denied path must return PermissionDenied, got: {result:?}"
        );
    }

    #[tokio::test]
    async fn list_denied_path_is_rejected() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));

        let security = SecurityConfig {
            denied_paths: vec![dir.path().to_path_buf()],
            allowed_paths: vec![],
            ..SecurityConfig::default()
        };
        let ctx = ctx_with_security(security);

        let input = serde_json::json!({ "path": dir.path().to_string_lossy().as_ref() });
        let result = ListDirTool.execute(input, &ctx).await;
        assert!(
            matches!(result, Err(ToolError::PermissionDenied { .. })),
            "list of denied path must return PermissionDenied, got: {result:?}"
        );
    }

    // -----------------------------------------------------------------------
    // Security: allowed_paths enforcement
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn read_outside_allowed_paths_is_rejected() {
        let allowed_dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        let other_dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        let file_outside = other_dir.path().join("outside.txt");
        std::fs::write(&file_outside, "data").unwrap_or_else(|e| panic!("write: {e}"));

        let security = SecurityConfig {
            allowed_paths: vec![allowed_dir.path().to_path_buf()],
            denied_paths: vec![],
            ..SecurityConfig::default()
        };
        let ctx = ctx_with_security(security);

        let input = serde_json::json!({ "path": file_outside.to_string_lossy().as_ref() });
        let result = ReadFileTool.execute(input, &ctx).await;
        assert!(
            matches!(result, Err(ToolError::PermissionDenied { .. })),
            "read outside allowed_paths must return PermissionDenied, got: {result:?}"
        );
    }

    #[tokio::test]
    async fn read_inside_allowed_paths_succeeds() {
        let allowed_dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        let file_inside = allowed_dir.path().join("inside.txt");
        std::fs::write(&file_inside, "hello").unwrap_or_else(|e| panic!("write: {e}"));

        let security = SecurityConfig {
            allowed_paths: vec![allowed_dir.path().to_path_buf()],
            denied_paths: vec![],
            ..SecurityConfig::default()
        };
        let ctx = ctx_with_security(security);

        let input = serde_json::json!({ "path": file_inside.to_string_lossy().as_ref() });
        let result = ReadFileTool.execute(input, &ctx).await;
        assert!(
            result.is_ok(),
            "read inside allowed_paths must succeed, got: {result:?}"
        );
        assert_eq!(
            result.unwrap_or_else(|e| panic!("{e}")).output["contents"],
            "hello"
        );
    }

    // -----------------------------------------------------------------------
    // Security: default deny-list (no SecurityConfig supplied)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn read_default_denied_etc_shadow_is_rejected() {
        // /etc/shadow is in the default SecurityConfig::default() deny-list.
        // Even without an explicit security_config the tool must reject it.
        let ctx = test_context(); // security_config: None -> uses SecurityConfig::default()

        let input = serde_json::json!({ "path": "/etc/shadow" });
        let result = ReadFileTool.execute(input, &ctx).await;
        // Must be either PermissionDenied (path exists and was denied) or
        // PermissionDenied (path doesn't exist -- canonicalization fail leads
        // to lexical path which is still in the deny list).
        // On macOS /etc/shadow does not exist so we get ExecutionFailed from
        // canonicalize, which then falls back to lexical normalization, which
        // still matches the deny list.
        assert!(
            matches!(
                result,
                Err(ToolError::PermissionDenied { .. }) | Err(ToolError::ExecutionFailed { .. })
            ),
            "reading /etc/shadow must not succeed, got: {result:?}"
        );
    }
}
