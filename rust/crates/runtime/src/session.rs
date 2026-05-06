use std::collections::BTreeMap;
use std::fmt::{Display, Formatter};
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::json::{JsonError, JsonValue};
use crate::usage::TokenUsage;

/// Gera um título curto e legível a partir do texto da primeira mensagem do usuário.
///
/// Regras:
/// - Pega o texto bruto da primeira mensagem com `role == User`.
/// - Remove prefixos comuns de comandos (`/`, `.`) e interjeições.
/// - Trunca em palavra completa até `max_len` (padrão 40).
/// - Capitaliza a primeira letra.
/// - Retorna `None` se não houver mensagem de usuário ou o texto for vazio.
#[must_use]
pub fn generate_session_title(session: &Session, max_len: usize) -> Option<String> {
    let first_user_text = session
        .messages
        .iter()
        .find(|m| m.role == MessageRole::User)
        .and_then(|m| m.blocks.iter().find_map(|b| match b {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        }))?;

    let trimmed = first_user_text.trim();
    if trimmed.is_empty() {
        return None;
    }

    // Remove prefixos comuns de comandos e interjeições
    let without_prefix = trimmed
        .trim_start_matches('/')
        .trim_start_matches('.')
        .trim_start();

    // Remove interjeições iniciais comuns (case-insensitive), char-based
    let cleaned = strip_prefix_ci_chars(without_prefix, "hey ")
        .or_else(|| strip_prefix_ci_chars(without_prefix, "hi "))
        .or_else(|| strip_prefix_ci_chars(without_prefix, "hello "))
        .or_else(|| strip_prefix_ci_chars(without_prefix, "ok "))
        .or_else(|| strip_prefix_ci_chars(without_prefix, "so "))
        .or_else(|| strip_prefix_ci_chars(without_prefix, "please "))
        .or_else(|| strip_prefix_ci_chars(without_prefix, "can you "))
        .or_else(|| strip_prefix_ci_chars(without_prefix, "could you "))
        .or_else(|| strip_prefix_ci_chars(without_prefix, "i need "))
        .or_else(|| strip_prefix_ci_chars(without_prefix, "i want "))
        .unwrap_or(without_prefix);

    let cleaned = cleaned.trim();
    if cleaned.is_empty() {
        return None;
    }

    // Trunca em palavra completa até max_len
    let truncated = if cleaned.chars().count() <= max_len {
        cleaned.to_string()
    } else {
        let mut result = String::new();
        let mut count = 0;
        for word in cleaned.split_whitespace() {
            let word_len = word.chars().count();
            let sep = if count > 0 { 1 } else { 0 };
            if count + sep + word_len > max_len {
                break;
            }
            if !result.is_empty() {
                result.push(' ');
            }
            result.push_str(word);
            count += sep + word_len;
        }
        result
    };

    if truncated.is_empty() {
        return None;
    }

    // Capitaliza a primeira letra
    let mut chars = truncated.chars();
    let first = chars.next()?;
    Some(format!("{}{}", first.to_uppercase(), chars.as_str()))
}

/// `strip_prefix` case-insensitive baseado em caracteres (não bytes), evitando
/// problemas com UTF-8 multi-byte.
fn strip_prefix_ci_chars<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    let prefix_chars: Vec<char> = prefix.chars().collect();
    let text_chars: Vec<char> = text.chars().collect();
    if text_chars.len() < prefix_chars.len() {
        return None;
    }
    let head_matches = text_chars
        .iter()
        .zip(prefix_chars.iter())
        .all(|(a, b)| a.eq_ignore_ascii_case(b));
    if !head_matches {
        return None;
    }
    // Reconstrói a substring a partir da contagem de caracteres do prefixo
    let skip_bytes: usize = prefix.chars().map(char::len_utf8).sum();
    Some(&text[skip_bytes..])
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MessageRole {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: String,
    },
    ToolResult {
        tool_use_id: String,
        tool_name: String,
        output: String,
        is_error: bool,
    },
    Thinking {
        thinking: String,
    },
    /// Imagem persistida como sidecar file (`<session_dir>/attachments/<sha256>`).
    /// O JSON da sessão guarda apenas o hash + metadados — os bytes são
    /// reidratados sob demanda quando construímos o request da Anthropic.
    Image {
        media_type: String,
        sha256: String,
        size: u64,
    },
    /// Documento (atualmente apenas PDF) persistido como sidecar file.
    Document {
        media_type: String,
        sha256: String,
        size: u64,
        name: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConversationMessage {
    pub role: MessageRole,
    pub blocks: Vec<ContentBlock>,
    pub usage: Option<TokenUsage>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Session {
    pub version: u32,
    /// Título humano-legível da sessão, gerado a partir da primeira mensagem do usuário.
    pub title: Option<String>,
    pub messages: Vec<ConversationMessage>,
}

#[derive(Debug)]
pub enum SessionError {
    Io(std::io::Error),
    Json(JsonError),
    Format(String),
}

impl Display for SessionError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "{error}"),
            Self::Json(error) => write!(f, "{error}"),
            Self::Format(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for SessionError {}

impl From<std::io::Error> for SessionError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<JsonError> for SessionError {
    fn from(value: JsonError) -> Self {
        Self::Json(value)
    }
}

impl Session {
    #[must_use]
    pub fn new() -> Self {
        Self {
            version: 1,
            title: None,
            messages: Vec::new(),
        }
    }

    /// Atualiza o título da sessão com base na primeira mensagem do usuário,
    /// mas apenas se ainda não tiver um título definido.
    pub fn auto_title(&mut self) {
        if self.title.is_none() {
            self.title = generate_session_title(self, 40);
        }
    }

    pub fn save_to_path(&self, path: impl AsRef<Path>) -> Result<(), SessionError> {
        fs::write(path, self.to_json().render())?;
        Ok(())
    }

    pub fn load_from_path(path: impl AsRef<Path>) -> Result<Self, SessionError> {
        let contents = fs::read_to_string(path)?;
        Self::from_json(&JsonValue::parse(&contents)?)
    }

    #[must_use]
    pub fn to_json(&self) -> JsonValue {
        let mut object = BTreeMap::new();
        object.insert(
            "version".to_string(),
            JsonValue::Number(i64::from(self.version)),
        );
        if let Some(ref title) = self.title {
            object.insert(
                "title".to_string(),
                JsonValue::String(title.clone()),
            );
        }
        object.insert(
            "messages".to_string(),
            JsonValue::Array(
                self.messages
                    .iter()
                    .map(ConversationMessage::to_json)
                    .collect(),
            ),
        );
        JsonValue::Object(object)
    }

    pub fn from_json(value: &JsonValue) -> Result<Self, SessionError> {
        let object = value
            .as_object()
            .ok_or_else(|| SessionError::Format("session must be an object".to_string()))?;
        let version = object
            .get("version")
            .and_then(JsonValue::as_i64)
            .ok_or_else(|| SessionError::Format("missing version".to_string()))?;
        let version = u32::try_from(version)
            .map_err(|_| SessionError::Format("version out of range".to_string()))?;
        let title = object
            .get("title")
            .and_then(JsonValue::as_str)
            .map(String::from);
        let messages = object
            .get("messages")
            .and_then(JsonValue::as_array)
            .ok_or_else(|| SessionError::Format("missing messages".to_string()))?
            .iter()
            .map(ConversationMessage::from_json)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self { version, title, messages })
    }
}

impl Default for Session {
    fn default() -> Self {
        Self::new()
    }
}

impl ConversationMessage {
    #[must_use]
    pub fn user_text(text: impl Into<String>) -> Self {
        Self::user_with_attachments(text, Vec::new())
    }

    /// Constrói uma mensagem `user` combinando texto e blocos de anexo já
    /// resolvidos (`Image`/`Document`). O bloco de texto é omitido quando o
    /// `text` é vazio — útil quando o usuário envia apenas anexos. A ordem
    /// preserva texto-primeiro seguido dos anexos na ordem fornecida.
    #[must_use]
    pub fn user_with_attachments(
        text: impl Into<String>,
        attachments: Vec<ContentBlock>,
    ) -> Self {
        let text = text.into();
        let mut blocks = Vec::with_capacity(usize::from(!text.is_empty()) + attachments.len());
        if !text.is_empty() {
            blocks.push(ContentBlock::Text { text });
        }
        blocks.extend(attachments);
        Self {
            role: MessageRole::User,
            blocks,
            usage: None,
        }
    }

    #[must_use]
    pub fn assistant(blocks: Vec<ContentBlock>) -> Self {
        Self {
            role: MessageRole::Assistant,
            blocks,
            usage: None,
        }
    }

    #[must_use]
    pub fn assistant_with_usage(blocks: Vec<ContentBlock>, usage: Option<TokenUsage>) -> Self {
        Self {
            role: MessageRole::Assistant,
            blocks,
            usage,
        }
    }

    #[must_use]
    pub fn tool_result(
        tool_use_id: impl Into<String>,
        tool_name: impl Into<String>,
        output: impl Into<String>,
        is_error: bool,
    ) -> Self {
        Self {
            role: MessageRole::Tool,
            blocks: vec![ContentBlock::ToolResult {
                tool_use_id: tool_use_id.into(),
                tool_name: tool_name.into(),
                output: output.into(),
                is_error,
            }],
            usage: None,
        }
    }

    #[must_use]
    pub fn to_json(&self) -> JsonValue {
        let mut object = BTreeMap::new();
        object.insert(
            "role".to_string(),
            JsonValue::String(
                match self.role {
                    MessageRole::System => "system",
                    MessageRole::User => "user",
                    MessageRole::Assistant => "assistant",
                    MessageRole::Tool => "tool",
                }
                .to_string(),
            ),
        );
        object.insert(
            "blocks".to_string(),
            JsonValue::Array(self.blocks.iter().map(ContentBlock::to_json).collect()),
        );
        if let Some(usage) = self.usage {
            object.insert("usage".to_string(), usage_to_json(usage));
        }
        JsonValue::Object(object)
    }

    fn from_json(value: &JsonValue) -> Result<Self, SessionError> {
        let object = value
            .as_object()
            .ok_or_else(|| SessionError::Format("message must be an object".to_string()))?;
        let role = match object
            .get("role")
            .and_then(JsonValue::as_str)
            .ok_or_else(|| SessionError::Format("missing role".to_string()))?
        {
            "system" => MessageRole::System,
            "user" => MessageRole::User,
            "assistant" => MessageRole::Assistant,
            "tool" => MessageRole::Tool,
            other => {
                return Err(SessionError::Format(format!(
                    "unsupported message role: {other}"
                )))
            }
        };
        let blocks = object
            .get("blocks")
            .and_then(JsonValue::as_array)
            .ok_or_else(|| SessionError::Format("missing blocks".to_string()))?
            .iter()
            .map(ContentBlock::from_json)
            .collect::<Result<Vec<_>, _>>()?;
        let usage = object.get("usage").map(usage_from_json).transpose()?;
        Ok(Self {
            role,
            blocks,
            usage,
        })
    }
}

impl ContentBlock {
    #[must_use]
    pub fn to_json(&self) -> JsonValue {
        let mut object = BTreeMap::new();
        match self {
            Self::Text { text } => {
                object.insert("type".to_string(), JsonValue::String("text".to_string()));
                object.insert("text".to_string(), JsonValue::String(text.clone()));
            }
            Self::ToolUse { id, name, input } => {
                object.insert(
                    "type".to_string(),
                    JsonValue::String("tool_use".to_string()),
                );
                object.insert("id".to_string(), JsonValue::String(id.clone()));
                object.insert("name".to_string(), JsonValue::String(name.clone()));
                object.insert("input".to_string(), JsonValue::String(input.clone()));
            }
            Self::ToolResult {
                tool_use_id,
                tool_name,
                output,
                is_error,
            } => {
                object.insert(
                    "type".to_string(),
                    JsonValue::String("tool_result".to_string()),
                );
                object.insert(
                    "tool_use_id".to_string(),
                    JsonValue::String(tool_use_id.clone()),
                );
                object.insert(
                    "tool_name".to_string(),
                    JsonValue::String(tool_name.clone()),
                );
                object.insert("output".to_string(), JsonValue::String(output.clone()));
                object.insert("is_error".to_string(), JsonValue::Bool(*is_error));
            }
            Self::Thinking { thinking } => {
                object.insert("type".to_string(), JsonValue::String("thinking".to_string()));
                object.insert("thinking".to_string(), JsonValue::String(thinking.clone()));
            }
            Self::Image { media_type, sha256, size } => {
                object.insert("type".to_string(), JsonValue::String("image".to_string()));
                object.insert(
                    "media_type".to_string(),
                    JsonValue::String(media_type.clone()),
                );
                object.insert("sha256".to_string(), JsonValue::String(sha256.clone()));
                // JsonValue::Number is i64; cap u64 → i64 for in-memory representation.
                // Sizes for clipboard images/PDFs always fit in i64 (max ~32 MB per cap).
                let size_i64 = i64::try_from(*size).unwrap_or(i64::MAX);
                object.insert("size".to_string(), JsonValue::Number(size_i64));
            }
            Self::Document { media_type, sha256, size, name } => {
                object.insert("type".to_string(), JsonValue::String("document".to_string()));
                object.insert(
                    "media_type".to_string(),
                    JsonValue::String(media_type.clone()),
                );
                object.insert("sha256".to_string(), JsonValue::String(sha256.clone()));
                let size_i64 = i64::try_from(*size).unwrap_or(i64::MAX);
                object.insert("size".to_string(), JsonValue::Number(size_i64));
                if let Some(name) = name {
                    object.insert("name".to_string(), JsonValue::String(name.clone()));
                }
            }
        }
        JsonValue::Object(object)
    }

    fn from_json(value: &JsonValue) -> Result<Self, SessionError> {
        let object = value
            .as_object()
            .ok_or_else(|| SessionError::Format("block must be an object".to_string()))?;
        match object
            .get("type")
            .and_then(JsonValue::as_str)
            .ok_or_else(|| SessionError::Format("missing block type".to_string()))?
        {
            "text" => Ok(Self::Text {
                text: required_string(object, "text")?,
            }),
            "tool_use" => Ok(Self::ToolUse {
                id: required_string(object, "id")?,
                name: required_string(object, "name")?,
                input: required_string(object, "input")?,
            }),
            "tool_result" => Ok(Self::ToolResult {
                tool_use_id: required_string(object, "tool_use_id")?,
                tool_name: required_string(object, "tool_name")?,
                output: required_string(object, "output")?,
                is_error: object
                    .get("is_error")
                    .and_then(JsonValue::as_bool)
                    .ok_or_else(|| SessionError::Format("missing is_error".to_string()))?,
            }),
            "thinking" => Ok(Self::Thinking {
                thinking: required_string(object, "thinking")?,
            }),
            "image" => Ok(Self::Image {
                media_type: required_string(object, "media_type")?,
                sha256: required_string(object, "sha256")?,
                size: required_u64(object, "size")?,
            }),
            "document" => Ok(Self::Document {
                media_type: required_string(object, "media_type")?,
                sha256: required_string(object, "sha256")?,
                size: required_u64(object, "size")?,
                name: object
                    .get("name")
                    .and_then(JsonValue::as_str)
                    .map(String::from),
            }),
            other => Err(SessionError::Format(format!(
                "unsupported block type: {other}"
            ))),
        }
    }
}

fn usage_to_json(usage: TokenUsage) -> JsonValue {
    let mut object = BTreeMap::new();
    object.insert(
        "input_tokens".to_string(),
        JsonValue::Number(i64::from(usage.input_tokens)),
    );
    object.insert(
        "output_tokens".to_string(),
        JsonValue::Number(i64::from(usage.output_tokens)),
    );
    object.insert(
        "cache_creation_input_tokens".to_string(),
        JsonValue::Number(i64::from(usage.cache_creation_input_tokens)),
    );
    object.insert(
        "cache_read_input_tokens".to_string(),
        JsonValue::Number(i64::from(usage.cache_read_input_tokens)),
    );
    JsonValue::Object(object)
}

fn usage_from_json(value: &JsonValue) -> Result<TokenUsage, SessionError> {
    let object = value
        .as_object()
        .ok_or_else(|| SessionError::Format("usage must be an object".to_string()))?;
    Ok(TokenUsage {
        input_tokens: required_u32(object, "input_tokens")?,
        output_tokens: required_u32(object, "output_tokens")?,
        cache_creation_input_tokens: required_u32(object, "cache_creation_input_tokens")?,
        cache_read_input_tokens: required_u32(object, "cache_read_input_tokens")?,
    })
}

fn required_string(
    object: &BTreeMap<String, JsonValue>,
    key: &str,
) -> Result<String, SessionError> {
    object
        .get(key)
        .and_then(JsonValue::as_str)
        .map(ToOwned::to_owned)
        .ok_or_else(|| SessionError::Format(format!("missing {key}")))
}

fn required_u32(object: &BTreeMap<String, JsonValue>, key: &str) -> Result<u32, SessionError> {
    let value = object
        .get(key)
        .and_then(JsonValue::as_i64)
        .ok_or_else(|| SessionError::Format(format!("missing {key}")))?;
    u32::try_from(value).map_err(|_| SessionError::Format(format!("{key} out of range")))
}

fn required_u64(object: &BTreeMap<String, JsonValue>, key: &str) -> Result<u64, SessionError> {
    let value = object
        .get(key)
        .and_then(JsonValue::as_i64)
        .ok_or_else(|| SessionError::Format(format!("missing {key}")))?;
    u64::try_from(value).map_err(|_| SessionError::Format(format!("{key} out of range")))
}

#[cfg(test)]
mod tests {
    use super::{generate_session_title, ContentBlock, ConversationMessage, MessageRole, Session};
    use crate::usage::TokenUsage;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn persists_and_restores_session_json() {
        let mut session = Session::new();
        session
            .messages
            .push(ConversationMessage::user_text("hello"));
        session
            .messages
            .push(ConversationMessage::assistant_with_usage(
                vec![
                    ContentBlock::Text {
                        text: "thinking".to_string(),
                    },
                    ContentBlock::ToolUse {
                        id: "tool-1".to_string(),
                        name: "bash".to_string(),
                        input: "echo hi".to_string(),
                    },
                ],
                Some(TokenUsage {
                    input_tokens: 10,
                    output_tokens: 4,
                    cache_creation_input_tokens: 1,
                    cache_read_input_tokens: 2,
                }),
            ));
        session.messages.push(ConversationMessage::tool_result(
            "tool-1", "bash", "hi", false,
        ));

        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("runtime-session-{nanos}.json"));
        session.save_to_path(&path).expect("session should save");
        let restored = Session::load_from_path(&path).expect("session should load");
        fs::remove_file(&path).expect("temp file should be removable");

        assert_eq!(restored, session);
        assert_eq!(restored.messages[2].role, MessageRole::Tool);
        assert_eq!(
            restored.messages[1].usage.expect("usage").total_tokens(),
            17
        );
    }

    #[test]
    fn generate_title_from_first_user_message() {
        let mut session = Session::new();
        session.messages.push(ConversationMessage::user_text(
            "como faço para implementar autenticação JWT?",
        ));
        session.auto_title();
        assert_eq!(
            session.title,
            Some("Como faço para implementar autenticação".to_string())
        );
    }

    #[test]
    fn generate_title_removes_common_prefixes() {
        let mut session = Session::new();
        session.messages.push(ConversationMessage::user_text(
            "hey can you help me refactor the parser code?",
        ));
        session.auto_title();
        assert_eq!(
            session.title,
            Some("Can you help me refactor the parser".to_string())
        );
    }

    #[test]
    fn generate_title_handles_slash_commands() {
        let mut session = Session::new();
        session.messages.push(ConversationMessage::user_text(
            "/diff mostra as diferenças entre os branches",
        ));
        session.auto_title();
        assert_eq!(
            session.title,
            Some("Diff mostra as diferenças entre os".to_string())
        );
    }

    #[test]
    fn generate_title_truncates_at_word_boundary() {
        let mut session = Session::new();
        session.messages.push(ConversationMessage::user_text(
            "esta é uma mensagem extremamente longa que precisa ser truncada corretamente em limite de palavra",
        ));
        session.auto_title();
        let title = session.title.expect("should have title");
        assert!(title.chars().count() <= 40, "title should be <= 40 chars");
        assert!(!title.ends_with(' '));
    }

    #[test]
    fn generate_title_returns_none_for_empty_session() {
        let session = Session::new();
        assert_eq!(generate_session_title(&session, 40), None);
    }

    #[test]
    fn auto_title_does_not_override_existing() {
        let mut session = Session::new();
        session.title = Some("Custom Title".to_string());
        session.messages.push(ConversationMessage::user_text("hello world"));
        session.auto_title();
        assert_eq!(session.title, Some("Custom Title".to_string()));
    }

    #[test]
    fn image_content_block_round_trips_through_session_json() {
        let mut session = Session::new();
        session
            .messages
            .push(ConversationMessage::user_with_attachments(
                "look at this",
                vec![ContentBlock::Image {
                    media_type: "image/png".to_string(),
                    sha256: "deadbeef".repeat(8),
                    size: 1_234_567,
                }],
            ));

        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("runtime-image-block-{nanos}.json"));
        session.save_to_path(&path).expect("save");
        let restored = Session::load_from_path(&path).expect("load");
        fs::remove_file(&path).expect("rm");

        assert_eq!(restored, session);
        let blocks = &restored.messages[0].blocks;
        assert_eq!(blocks.len(), 2);
        assert!(matches!(&blocks[0], ContentBlock::Text { text } if text == "look at this"));
        match &blocks[1] {
            ContentBlock::Image { media_type, sha256, size } => {
                assert_eq!(media_type, "image/png");
                assert_eq!(sha256.len(), 64);
                assert_eq!(*size, 1_234_567);
            }
            other => panic!("expected Image, got {other:?}"),
        }
    }

    #[test]
    fn document_content_block_round_trips_with_optional_name() {
        let mut session = Session::new();
        session
            .messages
            .push(ConversationMessage::user_with_attachments(
                "summarize",
                vec![
                    ContentBlock::Document {
                        media_type: "application/pdf".to_string(),
                        sha256: "cafef00d".repeat(8),
                        size: 9_876,
                        name: Some("contract.pdf".to_string()),
                    },
                    ContentBlock::Document {
                        media_type: "application/pdf".to_string(),
                        sha256: "1a2b3c4d".repeat(8),
                        size: 42,
                        name: None,
                    },
                ],
            ));

        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("runtime-document-block-{nanos}.json"));
        session.save_to_path(&path).expect("save");
        let restored = Session::load_from_path(&path).expect("load");
        fs::remove_file(&path).expect("rm");

        assert_eq!(restored, session);
        let blocks = &restored.messages[0].blocks;
        assert_eq!(blocks.len(), 3);
        match &blocks[1] {
            ContentBlock::Document { name, .. } => {
                assert_eq!(name.as_deref(), Some("contract.pdf"));
            }
            other => panic!("expected Document, got {other:?}"),
        }
        match &blocks[2] {
            ContentBlock::Document { name, size, .. } => {
                assert!(name.is_none());
                assert_eq!(*size, 42);
            }
            other => panic!("expected Document, got {other:?}"),
        }
    }

    #[test]
    fn user_with_attachments_omits_empty_text_block() {
        let msg = ConversationMessage::user_with_attachments(
            "",
            vec![ContentBlock::Image {
                media_type: "image/jpeg".to_string(),
                sha256: "ff".repeat(32),
                size: 16,
            }],
        );
        assert_eq!(msg.role, MessageRole::User);
        assert_eq!(msg.blocks.len(), 1);
        assert!(matches!(&msg.blocks[0], ContentBlock::Image { .. }));
    }

    #[test]
    fn user_with_attachments_keeps_text_first() {
        let msg = ConversationMessage::user_with_attachments(
            "ola",
            vec![ContentBlock::Document {
                media_type: "application/pdf".to_string(),
                sha256: "ab".repeat(32),
                size: 1,
                name: None,
            }],
        );
        assert!(matches!(&msg.blocks[0], ContentBlock::Text { text } if text == "ola"));
        assert!(matches!(&msg.blocks[1], ContentBlock::Document { .. }));
    }
}
