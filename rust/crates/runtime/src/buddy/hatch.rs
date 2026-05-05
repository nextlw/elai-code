//! Companion hatching — LLM generates name + personality on first run,
//! then persists `StoredCompanion` to `~/.elai/companion.json`.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use super::types::{Companion, CompanionBones, CompanionSoul, StoredCompanion};
use super::generator::roll_bones;

// ── Storage helpers ───────────────────────────────────────────────────────────

fn companion_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".elai").join("companion.json"))
}

/// Loads a previously-hatched companion from disk.  Returns `None` if not found
/// or if the file fails to parse (triggering a fresh hatch).
#[must_use]
pub fn load_stored_companion() -> Option<StoredCompanion> {
    let path = companion_path()?;
    let raw = std::fs::read_to_string(&path).ok()?;
    serde_json::from_str(&raw)
        .map_err(|e| eprintln!("[elai-buddy] companion.json parse error: {e}"))
        .ok()
}

/// Persists `StoredCompanion` to `~/.elai/companion.json`.
pub fn save_stored_companion(stored: &StoredCompanion) -> std::io::Result<()> {
    let path = companion_path().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::NotFound, "home dir unavailable")
    })?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(stored)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    std::fs::write(&path, json)
}

// ── Hatch ─────────────────────────────────────────────────────────────────────

fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn hatch_prompt(bones: &CompanionBones) -> String {
    format!(
        "You are hatching a companion for an AI coding assistant named Elai.\n\
         The companion is a {species} of {rarity} rarity.\n\
         Give it a short, creative name (1-2 words) and a one-sentence personality description.\n\
         Respond ONLY in JSON: {{\"name\": \"...\", \"personality\": \"...\"}}\n\
         Keep the name under 20 characters and the personality under 80 characters.",
        species = bones.species.as_str(),
        rarity = bones.rarity.as_str(),
    )
}

/// Hatch a companion using the provided LLM callback.
///
/// `call_llm(prompt) -> Result<String, E>` should return the raw LLM response.
/// On JSON parse failure, falls back to a default name/personality.
pub fn hatch_with_llm<E, F>(user_id: &str, call_llm: F) -> Result<Companion, E>
where
    F: FnOnce(&str) -> Result<String, E>,
{
    let bones = roll_bones(user_id);
    let prompt = hatch_prompt(&bones);
    let raw = call_llm(&prompt)?;
    let soul = parse_soul_from_response(&raw, &bones);
    let hatched_at = now_unix_secs();
    let stored = StoredCompanion {
        soul: soul.clone(),
        hatched_at,
    };
    // Best-effort persist — ignore write errors in the hatch path.
    let _ = save_stored_companion(&stored);
    Ok(Companion::from_parts(bones, soul, hatched_at))
}

fn parse_soul_from_response(raw: &str, bones: &CompanionBones) -> CompanionSoul {
    // Extract the first JSON object from the response (model may prefix with prose).
    if let Some(start) = raw.find('{') {
        if let Some(end) = raw[start..].find('}') {
            let json_str = &raw[start..=start + end];
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(json_str) {
                let name = value["name"]
                    .as_str()
                    .filter(|s| !s.trim().is_empty())
                    .map(str::to_string);
                let personality = value["personality"]
                    .as_str()
                    .filter(|s| !s.trim().is_empty())
                    .map(str::to_string);
                if let (Some(name), Some(personality)) = (name, personality) {
                    return CompanionSoul { name, personality };
                }
            }
        }
    }
    // Fallback when JSON parsing fails.
    default_soul(bones)
}

fn default_soul(bones: &CompanionBones) -> CompanionSoul {
    CompanionSoul {
        name: format!("{}-{}", bones.species.as_str(), bones.rarity.as_str()),
        personality: "A quiet companion who watches the code flow.".to_string(),
    }
}

/// Load existing companion or create one using the provided LLM callback.
///
/// If a `StoredCompanion` already exists on disk, returns it (regenerating bones
/// deterministically from `user_id`).  Otherwise calls `call_llm` to hatch.
pub fn load_or_hatch<E, F>(user_id: &str, call_llm: F) -> Result<Companion, E>
where
    F: FnOnce(&str) -> Result<String, E>,
{
    let bones = roll_bones(user_id);
    if let Some(stored) = load_stored_companion() {
        return Ok(Companion::from_parts(bones, stored.soul, stored.hatched_at));
    }
    hatch_with_llm(user_id, call_llm)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buddy::generator::roll_bones;

    #[test]
    fn parse_soul_extracts_name_and_personality() {
        let bones = roll_bones("test");
        let raw = r#"{"name": "Capri", "personality": "A bold debugger."}"#;
        let soul = parse_soul_from_response(raw, &bones);
        assert_eq!(soul.name, "Capri");
        assert_eq!(soul.personality, "A bold debugger.");
    }

    #[test]
    fn parse_soul_falls_back_on_invalid_json() {
        let bones = roll_bones("test");
        let soul = parse_soul_from_response("not json at all", &bones);
        assert!(!soul.name.is_empty());
        assert!(!soul.personality.is_empty());
    }

    #[test]
    fn parse_soul_handles_prose_prefix() {
        let bones = roll_bones("test");
        let raw = r#"Here you go: {"name": "Noodle", "personality": "Calm and wise."}"#;
        let soul = parse_soul_from_response(raw, &bones);
        assert_eq!(soul.name, "Noodle");
    }

    #[test]
    fn hatch_with_llm_uses_callback() {
        let companion = hatch_with_llm("user-abc", |_prompt| {
            Ok::<_, String>(
                r#"{"name": "Orb", "personality": "Loves refactoring."}"#.to_string(),
            )
        })
        .unwrap();
        assert_eq!(companion.name, "Orb");
        assert_eq!(companion.personality, "Loves refactoring.");
    }
}
