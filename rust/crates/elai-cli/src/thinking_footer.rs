//! Frases rotativas da linha “thinking” no painel de chat da TUI (JSON em `rust/locales`).

use std::borrow::Cow;
use std::sync::Mutex;

use serde_json::Value;

const PT_BR_JSON: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../locales/pt-BR.json"
));
const EN_JSON: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../locales/en.json"
));

#[derive(Clone)]
struct PhraseLists {
    normal: Vec<String>,
    ultra: Vec<String>,
}

struct CacheCell {
    locale_key: String,
    lists: PhraseLists,
}

static FOOTER_CACHE: Mutex<Option<CacheCell>> = Mutex::new(None);

fn json_blob_for_locale(locale: &str) -> &'static str {
    if locale == "en" {
        EN_JSON
    } else {
        PT_BR_JSON
    }
}

fn thinking_footer_string_array(
    key: &str,
    tf: &serde_json::Map<String, Value>,
) -> Result<Vec<String>, String> {
    let arr = tf
        .get(key)
        .and_then(Value::as_array)
        .ok_or_else(|| format!("locale: missing thinking_footer.{key} array"))?;
    let v: Vec<String> = arr
        .iter()
        .filter_map(|x| x.as_str().map(std::borrow::ToOwned::to_owned))
        .collect();
    if v.is_empty() {
        return Err(format!("locale: empty thinking_footer.{key}"));
    }
    Ok(v)
}

fn parse_footer_lists(json_text: &str) -> Result<PhraseLists, String> {
    let root: Value = serde_json::from_str(json_text).map_err(|e| e.to_string())?;
    let tui = root
        .get("tui")
        .and_then(Value::as_object)
        .ok_or_else(|| "locale: missing \"tui\" object".to_string())?;
    let tf = tui
        .get("thinking_footer")
        .and_then(Value::as_object)
        .ok_or_else(|| "locale: missing \"thinking_footer\"".to_string())?;

    Ok(PhraseLists {
        normal: thinking_footer_string_array("normal", tf)?,
        ultra: thinking_footer_string_array("ultra", tf)?,
    })
}

fn lists_for_current_locale_cached() -> PhraseLists {
    let locale_raw = rust_i18n::locale();
    let locale_key = locale_raw.to_string();

    let mut guard = FOOTER_CACHE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);

    if let Some(existing) = guard.as_ref() {
        if existing.locale_key == locale_key {
            return existing.lists.clone();
        }
    }

    let blob = json_blob_for_locale(&locale_key);
    let lists = match parse_footer_lists(blob) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("elai: thinking_footer phrases load failed ({e}); fallback en");
            parse_footer_lists(EN_JSON).expect("thinking_footer phrases (en) must validate")
        }
    };

    *guard = Some(CacheCell {
        locale_key: locale_key.clone(),
        lists: lists.clone(),
    });
    lists
}

#[must_use]
pub(crate) fn thinking_footer_caption(ultra: bool, frame: usize) -> String {
    let lists = lists_for_current_locale_cached();
    let choices = if ultra {
        lists.ultra.as_slice()
    } else {
        lists.normal.as_slice()
    };

    debug_assert!(
        !choices.is_empty(),
        "thinking_footer lists validated non-empty"
    );

    let i = frame % choices.len();
    choices[i].clone()
}

#[must_use]
/// Trunca por **caracteres Unicode escalares** (sem crate extra); sufixo `…`.
pub(crate) fn truncate_graphemes(s: &str, max_visual_chars: usize) -> Cow<'_, str> {
    let count = s.chars().count();
    if max_visual_chars == 0 {
        return Cow::Borrowed("");
    }
    if count <= max_visual_chars {
        Cow::Borrowed(s)
    } else if max_visual_chars == 1 {
        Cow::Borrowed("…")
    } else {
        let clipped: String = s.chars().take(max_visual_chars.saturating_sub(1)).collect();
        Cow::Owned(format!("{clipped}…"))
    }
}

#[cfg(test)]
mod thinking_footer_tests {
    use super::*;

    #[test]
    fn truncate_respects_budget() {
        assert_eq!(truncate_graphemes("abcdef", 4).as_ref(), "abc…");
        assert!(truncate_graphemes("a", 0).as_ref().is_empty());
    }

    #[test]
    fn caption_rotates_and_ultra_prefix() {
        rust_i18n::set_locale("pt-BR");
        let a = thinking_footer_caption(false, 0);
        let b = thinking_footer_caption(false, 1);
        assert!(!a.is_empty());
        assert!(!b.is_empty());
        let u = thinking_footer_caption(true, 2);
        assert!(
            u.starts_with("Ultra "),
            "ultra list must use Ultra prefix, got: {u}"
        );
    }

    #[test]
    fn caption_respects_en_locale() {
        rust_i18n::set_locale("en");
        let s = thinking_footer_caption(false, 0);
        assert!(!s.is_empty());
        rust_i18n::set_locale("pt-BR");
    }
}
