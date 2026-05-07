use std::time::{SystemTime, UNIX_EPOCH};

use tokio::process::Command;
use tokio::time::{timeout, Duration};

use crate::error::ApiError;
use crate::types::{
    ContentBlockDelta, ContentBlockDeltaEvent, ContentBlockStartEvent, ContentBlockStopEvent,
    InputContentBlock, MessageDelta, MessageDeltaEvent, MessageRequest, MessageResponse,
    MessageStartEvent, MessageStopEvent, OutputContentBlock, StreamEvent, ToolResultContentBlock,
    Usage,
};

const DEFAULT_CODEX_COMMAND: &str = "codex";
const DEFAULT_EXEC_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Debug, Clone, Copy)]
struct CodexExecCapabilities {
    supports_sandbox: bool,
    supports_ask_for_approval: bool,
}

#[derive(Debug, Clone)]
struct CodexExecPolicy {
    sandbox_mode: String,
    approval_policy: String,
}

#[derive(Debug, Clone)]
pub struct CodexBridgeClient {
    codex_command: String,
    exec_timeout: Duration,
    capabilities: CodexExecCapabilities,
}

impl CodexBridgeClient {
    #[must_use]
    pub fn new(codex_command: impl Into<String>) -> Self {
        let codex_command = codex_command.into();
        Self {
            capabilities: detect_exec_capabilities(&codex_command),
            codex_command,
            exec_timeout: DEFAULT_EXEC_TIMEOUT,
        }
    }

    pub fn from_env() -> Result<Self, ApiError> {
        let command = std::env::var("ELAI_CODEX_BRIDGE_COMMAND")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_CODEX_COMMAND.to_string());
        if command.trim().is_empty() {
            return Err(ApiError::Auth(
                "ELAI_CODEX_BRIDGE_COMMAND is empty".to_string(),
            ));
        }
        Ok(Self::new(command))
    }

    #[must_use]
    pub fn with_timeout(mut self, timeout_duration: Duration) -> Self {
        self.exec_timeout = timeout_duration;
        self
    }

    pub async fn send_message(&self, request: &MessageRequest) -> Result<MessageResponse, ApiError> {
        let prompt = render_exec_prompt(request);
        let output = self
            .run_exec_with_fallback(&request.model, &prompt)
            .await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            return Err(ApiError::Auth(format!(
                "codex exec bridge failed (status {}): {}",
                output.status,
                if stderr.is_empty() {
                    "no stderr output".to_string()
                } else {
                    stderr
                }
            )));
        }

        let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if text.is_empty() {
            return Err(ApiError::Auth(
                "codex exec bridge returned empty response".to_string(),
            ));
        }
        Ok(build_bridge_response(&request.model, text))
    }

    pub async fn stream_message(&self, request: &MessageRequest) -> Result<MessageStream, ApiError> {
        let response = self.send_message(request).await?;
        Ok(MessageStream::from_response(response))
    }
}

impl CodexBridgeClient {
    async fn run_exec_with_fallback(
        &self,
        model: &str,
        prompt: &str,
    ) -> Result<std::process::Output, ApiError> {
        let policy = resolve_exec_policy();
        let variants = exec_arg_variants(self.capabilities, model, prompt, &policy);
        let mut last_output: Option<std::process::Output> = None;
        for args in variants {
            let output = self.run_exec_once(&args).await?;
            if output.status.success() {
                return Ok(output);
            }
            let stderr = String::from_utf8_lossy(&output.stderr);
            let unsupported_argument = stderr.contains("unexpected argument");
            last_output = Some(output);
            if !unsupported_argument {
                break;
            }
        }
        let Some(output) = last_output else {
            return Err(ApiError::Auth(
                "codex exec bridge failed before running command".to_string(),
            ));
        };
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(ApiError::Auth(format!(
            "codex exec bridge failed (status {}): {}",
            output.status,
            if stderr.is_empty() {
                "no stderr output".to_string()
            } else {
                stderr
            }
        )))
    }

    async fn run_exec_once(&self, args: &[String]) -> Result<std::process::Output, ApiError> {
        let mut command = Command::new(&self.codex_command);
        command.args(args);
        timeout(self.exec_timeout, command.output())
            .await
            .map_err(|_| {
                ApiError::Auth(format!(
                    "codex bridge timeout after {}s",
                    self.exec_timeout.as_secs()
                ))
            })?
            .map_err(ApiError::from)
    }
}

fn detect_exec_capabilities(codex_command: &str) -> CodexExecCapabilities {
    let exec_help = std::process::Command::new(codex_command)
        .arg("exec")
        .arg("--help")
        .output();
    let Ok(exec_help) = exec_help else {
        return CodexExecCapabilities {
            supports_sandbox: true,
            supports_ask_for_approval: true,
        };
    };
    let exec_help_text = String::from_utf8_lossy(&exec_help.stdout);
    // `--ask-for-approval` pode estar no help global e não no subcomando `exec`.
    let global_help = std::process::Command::new(codex_command)
        .arg("--help")
        .output()
        .ok()
        .map(|out| String::from_utf8_lossy(&out.stdout).to_string())
        .unwrap_or_default();
    CodexExecCapabilities {
        supports_sandbox: exec_help_text.contains("--sandbox"),
        supports_ask_for_approval: exec_help_text.contains("--ask-for-approval")
            || global_help.contains("--ask-for-approval"),
    }
}

fn resolve_exec_policy() -> CodexExecPolicy {
    let sandbox_mode = std::env::var("ELAI_CODEX_BRIDGE_SANDBOX")
        .ok()
        .unwrap_or_else(|| "read-only".to_string());
    let sandbox_mode = match sandbox_mode.as_str() {
        "read-only" | "workspace-write" | "danger-full-access" => sandbox_mode,
        _ => "read-only".to_string(),
    };

    let approval_policy = std::env::var("ELAI_CODEX_BRIDGE_APPROVAL")
        .ok()
        .unwrap_or_else(|| "never".to_string());
    let approval_policy = match approval_policy.as_str() {
        "untrusted" | "on-failure" | "on-request" | "never" => approval_policy,
        _ => "never".to_string(),
    };

    CodexExecPolicy {
        sandbox_mode,
        approval_policy,
    }
}

fn exec_arg_variants(
    caps: CodexExecCapabilities,
    model: &str,
    prompt: &str,
    policy: &CodexExecPolicy,
) -> Vec<Vec<String>> {
    let mut global_first = vec![];
    if caps.supports_ask_for_approval {
        global_first.push("--ask-for-approval".to_string());
        global_first.push(policy.approval_policy.clone());
    }
    global_first.extend(vec![
        "exec".to_string(),
        "--model".to_string(),
        model.to_string(),
    ]);
    if caps.supports_sandbox {
        global_first.push("--sandbox".to_string());
        global_first.push(policy.sandbox_mode.clone());
    }
    global_first.push(prompt.to_string());

    let mut first = vec![
        "exec".to_string(),
        "--model".to_string(),
        model.to_string(),
    ];
    if caps.supports_sandbox {
        first.push("--sandbox".to_string());
        first.push(policy.sandbox_mode.clone());
    }
    if caps.supports_ask_for_approval {
        first.push("--ask-for-approval".to_string());
        first.push(policy.approval_policy.clone());
    }
    first.push(prompt.to_string());

    let mut second = vec![
        "exec".to_string(),
        "--model".to_string(),
        model.to_string(),
    ];
    if caps.supports_sandbox {
        second.push("--sandbox".to_string());
        second.push(policy.sandbox_mode.clone());
    }
    second.push(prompt.to_string());

    let third = vec![
        "exec".to_string(),
        "--model".to_string(),
        model.to_string(),
        prompt.to_string(),
    ];

    vec![global_first, first, second, third]
}

#[derive(Debug)]
pub struct MessageStream {
    events: Vec<StreamEvent>,
    index: usize,
    request_id: Option<String>,
}

impl MessageStream {
    #[allow(clippy::needless_pass_by_value)]
    fn from_response(response: MessageResponse) -> Self {
        let request_id = response.request_id.clone();
        let mut events = Vec::new();

        events.push(StreamEvent::MessageStart(MessageStartEvent {
            message: response.clone(),
        }));

        for (index, block) in response.content.iter().enumerate() {
            #[allow(clippy::cast_possible_truncation)]
            let index = index as u32;
            events.push(StreamEvent::ContentBlockStart(ContentBlockStartEvent {
                index,
                content_block: block.clone(),
            }));
            if let OutputContentBlock::Text { text } = block {
                events.push(StreamEvent::ContentBlockDelta(ContentBlockDeltaEvent {
                    index,
                    delta: ContentBlockDelta::TextDelta { text: text.clone() },
                }));
            }
            events.push(StreamEvent::ContentBlockStop(ContentBlockStopEvent {
                index,
            }));
        }

        events.push(StreamEvent::MessageDelta(MessageDeltaEvent {
            delta: MessageDelta {
                stop_reason: response.stop_reason.clone(),
                stop_sequence: response.stop_sequence.clone(),
            },
            usage: response.usage.clone(),
        }));
        events.push(StreamEvent::MessageStop(MessageStopEvent {}));

        Self {
            events,
            index: 0,
            request_id,
        }
    }

    #[must_use]
    pub fn request_id(&self) -> Option<&str> {
        self.request_id.as_deref()
    }

    #[allow(clippy::unused_async)]
    pub async fn next_event(&mut self) -> Result<Option<StreamEvent>, ApiError> {
        if self.index < self.events.len() {
            let event = self.events[self.index].clone();
            self.index += 1;
            Ok(Some(event))
        } else {
            Ok(None)
        }
    }
}

fn build_bridge_response(model: &str, text: String) -> MessageResponse {
    MessageResponse {
        id: format!(
            "msg_codex_bridge_{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis()
        ),
        kind: "message".to_string(),
        role: "assistant".to_string(),
        content: vec![OutputContentBlock::Text { text }],
        model: model.to_string(),
        stop_reason: Some("end_turn".to_string()),
        stop_sequence: None,
        usage: Usage {
            input_tokens: 0,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 0,
            output_tokens: 0,
        },
        request_id: None,
    }
}

fn render_exec_prompt(request: &MessageRequest) -> String {
    let mut lines = vec![
        "Você está atuando como bridge de resposta para outra CLI.".to_string(),
        "Responda APENAS com a mensagem final do assistente para a conversa abaixo.".to_string(),
        "Não execute comandos de shell nem chame ferramentas externas.".to_string(),
    ];

    if let Some(system) = &request.system {
        if !system.trim().is_empty() {
            lines.push(String::new());
            lines.push("### System".to_string());
            lines.push(system.trim().to_string());
        }
    }

    lines.push(String::new());
    lines.push("### Conversa".to_string());
    for message in &request.messages {
        lines.push(format!("{}:", message.role));
        for block in &message.content {
            lines.push(render_input_block(block));
        }
    }
    lines.push(String::new());
    lines.push("### Instrução".to_string());
    lines.push("Forneça somente o texto final de resposta do assistente.".to_string());
    lines.join("\n")
}

fn render_input_block(block: &InputContentBlock) -> String {
    match block {
        InputContentBlock::Text { text } => text.clone(),
        InputContentBlock::ToolUse { id, name, input } => {
            format!("[tool_use id={id} name={name} input={input}]")
        }
        InputContentBlock::ToolResult {
            tool_use_id,
            content,
            is_error,
        } => {
            let rendered = content
                .iter()
                .map(|entry| match entry {
                    ToolResultContentBlock::Text { text } => text.clone(),
                    ToolResultContentBlock::Json { value } => value.to_string(),
                })
                .collect::<Vec<_>>()
                .join(" ");
            format!(
                "[tool_result tool_use_id={tool_use_id} is_error={is_error}] {rendered}"
            )
        }
        InputContentBlock::Thinking { .. } => String::new(),
        // Codex bridge é text-only: anexos viram um marcador textual para
        // o histórico, mas não são realmente enviados ao modelo. A camada
        // de runtime deve ter rejeitado o request bem antes de chegar aqui.
        InputContentBlock::Image { source } => {
            format!("[image media_type={} ({} bytes b64)]", source.media_type, source.data.len())
        }
        InputContentBlock::Document { source } => {
            format!("[document media_type={} ({} bytes b64)]", source.media_type, source.data.len())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::render_exec_prompt;
    use crate::types::{InputContentBlock, InputMessage, MessageRequest};

    #[test]
    fn render_exec_prompt_includes_system_and_messages() {
        let request = MessageRequest {
            model: "gpt-5.5".to_string(),
            max_tokens: 4096,
            messages: vec![
                InputMessage {
                    role: "user".to_string(),
                    content: vec![InputContentBlock::Text {
                        text: "Oi".to_string(),
                    }],
                },
                InputMessage {
                    role: "assistant".to_string(),
                    content: vec![InputContentBlock::Text {
                        text: "Olá!".to_string(),
                    }],
                },
            ],
            system: Some("Seja breve".to_string()),
            tools: None,
            tool_choice: None,
            stream: false,
            thinking: None,
            reasoning_effort: None,
            output_config: None,
        };

        let prompt = render_exec_prompt(&request);
        assert!(prompt.contains("### System"));
        assert!(prompt.contains("Seja breve"));
        assert!(prompt.contains("user:"));
        assert!(prompt.contains("assistant:"));
        assert!(prompt.contains("Forneça somente o texto final"));
    }
}
