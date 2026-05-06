use axum::Json;
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct CommandEntry {
    pub name: &'static str,
    pub summary: String,
    pub category: &'static str,
    pub argument_hint: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ListCommandsResponse {
    pub commands: Vec<CommandEntry>,
}

pub async fn list_commands() -> Json<ListCommandsResponse> {
    let specs = commands::slash_command_specs();
    let commands = specs
        .iter()
        .filter(|s| !s.hidden && (s.is_enabled)())
        .map(|s| CommandEntry {
            name: s.name,
            summary: s.summary(),
            category: s.category.label(),
            argument_hint: s.argument_hint(),
        })
        .collect();
    Json(ListCommandsResponse { commands })
}
