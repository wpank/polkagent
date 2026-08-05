//! Built-in tool implementations.
//!
//! This module provides the default tool set available to every Polkagent agent:
//!
//! - **File tools** ([`mod@file`]): read, write, and list directory contents.
//! - **Shell tool** ([`shell`]): execute shell commands with timeout enforcement.
//! - **Search tool** ([`search`]): search agent memory.
//!
//! Use [`register_builtins`] to add all built-in tools to a [`ToolRegistry`]
//! in one call.

pub mod chain_knowledge;
pub mod file;
pub mod search;
pub mod shell;

use std::sync::Arc;

use polkagent_memory::store::MemoryStore;

use crate::registry::ToolRegistry;

/// Register all built-in tools into the given registry.
///
/// This adds file tools, the shell tool, and the memory search tool. A
/// [`MemoryStore`] must be provided so the search tool can be constructed.
///
/// Call this once when setting up a new run or agent context.
pub fn register_builtins(registry: &mut ToolRegistry, memory_store: Arc<dyn MemoryStore>) {
    // File tools.
    registry.register(Box::new(file::ReadFileTool));
    registry.register(Box::new(file::WriteFileTool));
    registry.register(Box::new(file::ListDirTool));

    // Shell tool.
    registry.register(Box::new(shell::ShellTool::default()));

    // Memory search tool.
    registry.register(Box::new(search::SearchMemoryTool::new(
        memory_store.clone(),
    )));

    // Chain knowledge tool (metadata-grounded RAG).
    registry.register(Box::new(chain_knowledge::ChainKnowledgeTool::new(
        memory_store,
    )));
}
