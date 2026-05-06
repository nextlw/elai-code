use axum::Json;
use serde::Serialize;
use serde_json::Value;
use tools::GlobalToolRegistry;

#[derive(Debug, Serialize)]
pub struct ToolEntry {
    pub name: String,
    pub description: Option<String>,
    pub input_schema: Value,
}

#[derive(Debug, Serialize)]
pub struct ListToolsResponse {
    pub tools: Vec<ToolEntry>,
}

pub async fn list_tools() -> Json<ListToolsResponse> {
    let registry = GlobalToolRegistry::builtin();
    let definitions = registry.definitions(None);
    let tools = definitions
        .into_iter()
        .map(|def| ToolEntry {
            name: def.name,
            description: def.description,
            input_schema: def.input_schema,
        })
        .collect();
    Json(ListToolsResponse { tools })
}
