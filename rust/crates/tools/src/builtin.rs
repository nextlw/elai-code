use runtime::PermissionMode;

use crate::ToolSpec;

const BUILTIN_TOOLS_TOML: &str = include_str!("../assets/builtin_tools.toml");

#[derive(serde::Deserialize)]
struct BuiltinToolToml {
    id: String,
    #[allow(dead_code)]
    title: String,
    description: String,
    #[allow(dead_code)]
    category: String,
    #[allow(dead_code)]
    priority: i32,
    required_permission: String,
    #[serde(default)]
    #[allow(dead_code)]
    embedding_hints: Vec<String>,
    input_schema_json: String,
}

#[derive(serde::Deserialize)]
struct BuiltinCatalog {
    tool: Vec<BuiltinToolToml>,
}

fn load_builtin_specs() -> Vec<ToolSpec> {
    let catalog: BuiltinCatalog = toml::from_str(BUILTIN_TOOLS_TOML)
        .expect("builtin_tools.toml is invalid — this is a compile-time equivalent error");
    catalog
        .tool
        .into_iter()
        .map(|t| {
            let input_schema: serde_json::Value = serde_json::from_str(&t.input_schema_json)
                .unwrap_or_else(|e| {
                    panic!(
                        "input_schema_json is invalid for tool '{}': {e}",
                        t.id
                    )
                });
            let required_permission = match t.required_permission.as_str() {
                "ReadOnly" => PermissionMode::ReadOnly,
                "WorkspaceWrite" => PermissionMode::WorkspaceWrite,
                "DangerFullAccess" => PermissionMode::DangerFullAccess,
                other => panic!("invalid required_permission value: '{other}'"),
            };
            // ToolSpec uses &'static str — leak the strings so they live for the
            // duration of the process. This is intentional: builtin specs are
            // loaded once at startup and never change.
            let name: &'static str = Box::leak(t.id.into_boxed_str());
            let description: &'static str = Box::leak(t.description.into_boxed_str());
            ToolSpec {
                name,
                description,
                input_schema,
                required_permission,
            }
        })
        .collect()
}

#[must_use]
#[allow(clippy::too_many_lines)]
pub fn mvp_tool_specs() -> Vec<ToolSpec> {
    load_builtin_specs()
}
