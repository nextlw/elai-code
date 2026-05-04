use std::collections::BTreeMap;
use std::fmt::Write;

use runtime::TelemetryEntry;

#[derive(Debug, Clone, Default)]
pub struct AggregatedStats {
    pub requests: u32,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_cost_usd: f64,
    pub total_latency_ms: u64,
}

impl AggregatedStats {
    #[must_use] 
    #[allow(clippy::cast_precision_loss)]
    pub fn avg_latency_ms(&self) -> f64 {
        if self.requests == 0 {
            0.0
        } else {
            self.total_latency_ms as f64 / f64::from(self.requests)
        }
    }
}

/// Group entries by an arbitrary key function.
pub fn aggregate_by<F>(entries: &[TelemetryEntry], key_fn: F) -> BTreeMap<String, AggregatedStats>
where
    F: Fn(&TelemetryEntry) -> String,
{
    let mut map: BTreeMap<String, AggregatedStats> = BTreeMap::new();
    for entry in entries {
        let key = key_fn(entry);
        let stats = map.entry(key).or_default();
        stats.requests += 1;
        stats.input_tokens += entry.input_tokens;
        stats.output_tokens += entry.output_tokens;
        stats.total_cost_usd += entry.cost_usd;
        stats.total_latency_ms += entry.latency_ms;
    }
    map
}

/// Calculate totals across all entries.
#[must_use] 
pub fn overall_stats(entries: &[TelemetryEntry]) -> AggregatedStats {
    let mut totals = AggregatedStats::default();
    for entry in entries {
        totals.requests += 1;
        totals.input_tokens += entry.input_tokens;
        totals.output_tokens += entry.output_tokens;
        totals.total_cost_usd += entry.cost_usd;
        totals.total_latency_ms += entry.latency_ms;
    }
    totals
}

/// Render an ASCII-aligned table.
/// Columns: Key | Requests | Input Tok | Output Tok | Cost USD | Avg Latency
#[must_use] 
pub fn render_stats_table(title: &str, stats: &BTreeMap<String, AggregatedStats>) -> String {
    const HEADERS: [&str; 6] = [
        "Key",
        "Requests",
        "Input Tok",
        "Output Tok",
        "Cost USD",
        "Avg Latency",
    ];

    // Build rows as arrays of Strings.
    let rows: Vec<[String; 6]> = stats
        .iter()
        .map(|(key, s)| {
            [
                key.clone(),
                s.requests.to_string(),
                s.input_tokens.to_string(),
                s.output_tokens.to_string(),
                format!("{:.6}", s.total_cost_usd),
                format!("{:.1}ms", s.avg_latency_ms()),
            ]
        })
        .collect();

    // Calculate column widths.
    let mut widths = [0usize; 6];
    for (i, h) in HEADERS.iter().enumerate() {
        widths[i] = h.len();
    }
    for row in &rows {
        for (i, cell) in row.iter().enumerate() {
            if cell.len() > widths[i] {
                widths[i] = cell.len();
            }
        }
    }

    let mut out = String::new();
    out.push_str(title);
    out.push('\n');

    // Header row — text left-aligned.
    let header_line = HEADERS
        .iter()
        .enumerate()
        .map(|(i, h)| format!("{:<width$}", h, width = widths[i]))
        .collect::<Vec<_>>()
        .join(" | ");
    out.push_str(&header_line);
    out.push('\n');

    // Separator.
    let sep_width: usize = widths.iter().sum::<usize>() + (widths.len() - 1) * 3;
    out.push_str(&"-".repeat(sep_width));
    out.push('\n');

    // Data rows — numbers right-aligned, key left-aligned.
    for row in &rows {
        let cells: Vec<String> = row
            .iter()
            .enumerate()
            .map(|(i, cell)| {
                if i == 0 {
                    format!("{:<width$}", cell, width = widths[i])
                } else {
                    format!("{:>width$}", cell, width = widths[i])
                }
            })
            .collect();
        out.push_str(&cells.join(" | "));
        out.push('\n');
    }

    if rows.is_empty() {
        out.push_str("(no data)\n");
    }

    out
}

/// Render the full stats report.
#[must_use] 
pub fn render_stats_report(
    entries: &[TelemetryEntry],
    by_model: bool,
    by_project: bool,
    days: Option<u32>,
) -> String {
    if entries.is_empty() {
        return "No telemetry data found. Use elai to start tracking.\n".to_string();
    }

    let mut out = String::new();

    if let Some(d) = days {
        let _ = writeln!(out, "Stats (last {d} days)");
    } else {
        out.push_str("Stats (all time)\n");
    }
    out.push_str(&"=".repeat(50));
    out.push('\n');

    let overall = overall_stats(entries);
    let _ = write!(
        out,
        "Overall: {} requests | {} input tokens | {} output tokens | ${:.6} cost\n\n",
        overall.requests, overall.input_tokens, overall.output_tokens, overall.total_cost_usd
    );

    if by_model {
        let by_model_map = aggregate_by(entries, |e| e.model.clone());
        out.push_str(&render_stats_table("By Model", &by_model_map));
        out.push('\n');
    }

    if by_project {
        let by_project_map = aggregate_by(entries, |e| e.project.clone());
        out.push_str(&render_stats_table("By Project", &by_project_map));
        out.push('\n');
    }

    if !by_model && !by_project {
        // Default: show by model.
        let by_model_map = aggregate_by(entries, |e| e.model.clone());
        out.push_str(&render_stats_table("By Model", &by_model_map));
        out.push('\n');
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_entry(model: &str, project: &str, input: u64, output: u64, cost: f64, latency: u64) -> TelemetryEntry {
        TelemetryEntry {
            timestamp: "2026-04-27T10:00:00Z".to_string(),
            session_id: "sess-1".to_string(),
            project: project.to_string(),
            model: model.to_string(),
            input_tokens: input,
            output_tokens: output,
            cache_write_tokens: 0,
            cache_read_tokens: 0,
            cost_usd: cost,
            latency_ms: latency,
            success: true,
            provider: None,
            error_type: None,
        }
    }

    #[test]
    fn test_aggregate_by_model_groups_correctly() {
        let entries = vec![
            make_entry("claude-sonnet", "proj-a", 100, 50, 0.001, 300),
            make_entry("claude-sonnet", "proj-a", 200, 100, 0.002, 400),
            make_entry("claude-haiku", "proj-b", 80, 30, 0.0005, 200),
            make_entry("claude-haiku", "proj-b", 120, 60, 0.001, 250),
            make_entry("claude-haiku", "proj-b", 90, 40, 0.0008, 220),
        ];
        let map = aggregate_by(&entries, |e| e.model.clone());
        assert_eq!(map.len(), 2);
        let sonnet = map.get("claude-sonnet").unwrap();
        assert_eq!(sonnet.requests, 2);
        assert_eq!(sonnet.input_tokens, 300);
        assert_eq!(sonnet.output_tokens, 150);
        let haiku = map.get("claude-haiku").unwrap();
        assert_eq!(haiku.requests, 3);
        assert_eq!(haiku.input_tokens, 290);
    }

    #[test]
    fn test_overall_stats_sums_all() {
        let entries = vec![
            make_entry("m1", "p1", 100, 50, 0.01, 100),
            make_entry("m2", "p2", 200, 80, 0.02, 200),
            make_entry("m1", "p1", 150, 60, 0.015, 150),
        ];
        let s = overall_stats(&entries);
        assert_eq!(s.requests, 3);
        assert_eq!(s.input_tokens, 450);
        assert_eq!(s.output_tokens, 190);
        let eps = 1e-9;
        assert!((s.total_cost_usd - 0.045).abs() < eps);
    }

    #[test]
    fn test_render_table_aligns_columns() {
        let entries = vec![
            make_entry("very-long-model-name", "proj", 100, 50, 0.001, 300),
            make_entry("m", "proj", 200, 100, 0.002, 400),
        ];
        let map = aggregate_by(&entries, |e| e.model.clone());
        let table = render_stats_table("Test", &map);
        // All data lines (after header and separator) should have same length
        let lines: Vec<&str> = table.lines().collect();
        // Find the header and separator indices
        let data_lines: Vec<&str> = lines
            .iter()
            .skip(2) // title + header
            .filter(|l| !l.starts_with('-') && !l.is_empty())
            .copied()
            .collect();
        if data_lines.len() >= 2 {
            let first_len = data_lines[0].len();
            for line in &data_lines {
                assert_eq!(line.len(), first_len, "Line lengths must match: '{line}'");
            }
        }
    }

    #[test]
    fn test_empty_entries_no_panic() {
        let entries: Vec<TelemetryEntry> = vec![];
        let map = aggregate_by(&entries, |e| e.model.clone());
        let table = render_stats_table("Empty", &map);
        assert!(table.contains("no data"));
    }
}
