//! Builder do prompt de consolidação. Espelha buildConsolidationPrompt do leaked.

use std::path::Path;

#[must_use]
pub fn build_consolidation_prompt(memory_root: &Path, transcript_dir: &Path, extra: &str) -> String {
    format!(
        "You are consolidating session memories.

Memory root: {memory_root}
Transcript dir: {transcript_dir}

Your task:
1. Scan recent sessions (since last consolidation).
2. Identify recurring patterns, decisions, and preferences worth remembering.
3. Update or create files under the memory root with concise, durable knowledge.
4. Avoid redundancy with existing memory entries.

{extra}",
        memory_root = memory_root.display(),
        transcript_dir = transcript_dir.display(),
        extra = extra.trim(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn prompt_includes_paths_and_extra() {
        let p = build_consolidation_prompt(Path::new("/m"), Path::new("/t"), "EXTRA_NOTE");
        assert!(p.contains("/m"));
        assert!(p.contains("/t"));
        assert!(p.contains("EXTRA_NOTE"));
    }
}
