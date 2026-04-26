use std::sync::{Arc, Mutex};

use api::ToolDefinition;
use runtime::{McpServerManager, PermissionMode};

use crate::ToolSource;

/// A [`ToolSource`] implementation that exposes MCP-discovered tools to the
/// [`crate::GlobalToolRegistry`].
///
/// This provides **read-only** access to the tools already discovered by
/// [`McpServerManager::discover_tools`]. The `execute()` path for MCP tools
/// continues to be routed through `McpServerManager` directly in the CLI layer —
/// this source only affects what definitions the LLM sees in `definitions()`.
pub struct McpToolSource {
    manager: Arc<Mutex<McpServerManager>>,
}

impl McpToolSource {
    #[must_use]
    pub fn new(manager: Arc<Mutex<McpServerManager>>) -> Self {
        Self { manager }
    }
}

impl ToolSource for McpToolSource {
    fn definitions(&self) -> Vec<ToolDefinition> {
        let mgr = self.manager.lock().unwrap_or_else(|e| e.into_inner());
        mgr.all_managed_tools()
            .filter(|t| mgr.healthy(&t.server_name))
            .map(|t| ToolDefinition {
                name: t.qualified_name.clone(),
                description: t.tool.description.clone(),
                input_schema: t.tool.input_schema.clone().unwrap_or_else(|| {
                    serde_json::json!({"type": "object", "properties": {}})
                }),
            })
            .collect()
    }

    fn permissions(&self) -> Vec<(String, PermissionMode)> {
        // MCP tools require explicit user approval — DangerFullAccess is the correct default.
        self.definitions()
            .into_iter()
            .map(|d| (d.name, PermissionMode::DangerFullAccess))
            .collect()
    }
}
