//! File-system tools: read, write, and list directory contents.
//!
//! Each tool enforces grant checks via the registry's grant-checking mechanism.
//! The required grant patterns follow the `file/*` namespace.

use async_trait::async_trait;
use serde_json::Value;
use tracing::debug;

use polkagent_core::config::DataClassification;

use crate::registry::{ToolContext, ToolError, ToolHandler, ToolResult, ToolSpec};

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

    async fn execute(&self, input: Value, _context: &ToolContext) -> Result<ToolResult, ToolError> {
        let path =
            input
                .get("path")
                .and_then(Value::as_str)
                .ok_or_else(|| ToolError::InvalidInput {
                    reason: "missing or invalid 'path' field".to_string(),
                })?;

        debug!(path, "reading file");

        let contents =
            tokio::fs::read_to_string(path)
                .await
                .map_err(|e| ToolError::ExecutionFailed {
                    reason: format!("failed to read file '{}': {}", path, e),
                })?;

        Ok(ToolResult {
            output: serde_json::json!({
                "path": path,
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

    async fn execute(&self, input: Value, _context: &ToolContext) -> Result<ToolResult, ToolError> {
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

        debug!(path, bytes = content.len(), "writing file");

        // Ensure parent directory exists.
        if let Some(parent) = std::path::Path::new(path).parent() {
            if !parent.as_os_str().is_empty() {
                tokio::fs::create_dir_all(parent).await.map_err(|e| {
                    ToolError::ExecutionFailed {
                        reason: format!(
                            "failed to create parent directories for '{}': {}",
                            path, e
                        ),
                    }
                })?;
            }
        }

        tokio::fs::write(path, content)
            .await
            .map_err(|e| ToolError::ExecutionFailed {
                reason: format!("failed to write file '{}': {}", path, e),
            })?;

        Ok(ToolResult {
            output: serde_json::json!({
                "path": path,
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

    async fn execute(&self, input: Value, _context: &ToolContext) -> Result<ToolResult, ToolError> {
        let path =
            input
                .get("path")
                .and_then(Value::as_str)
                .ok_or_else(|| ToolError::InvalidInput {
                    reason: "missing or invalid 'path' field".to_string(),
                })?;

        debug!(path, "listing directory");

        let mut read_dir =
            tokio::fs::read_dir(path)
                .await
                .map_err(|e| ToolError::ExecutionFailed {
                    reason: format!("failed to read directory '{}': {}", path, e),
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
                        reason: format!("error reading directory entry in '{}': {}", path, e),
                    });
                }
            }
        }

        Ok(ToolResult {
            output: serde_json::json!({
                "path": path,
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
    use polkagent_core::ids::{AgentId, RunId, StepId};

    fn test_context() -> ToolContext {
        ToolContext {
            run_id: RunId::new(),
            agent_id: AgentId::new(),
            step_id: StepId::new(),
            grants: vec![],
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
}
