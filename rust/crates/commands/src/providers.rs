use runtime::TelemetryEntry;

#[derive(Debug, Clone)]
pub struct ProviderHealthSummary {
    pub id: String,
    pub total_calls: u32,
    pub success_rate: f64,
    pub avg_latency_ms: f64,
    pub ema_latency_ms: f64,
    pub score: f64,
    pub status: ProviderStatus,
}

#[derive(Debug, Clone, Copy)]
pub enum ProviderStatus {
    Healthy,
    Degraded { reason: &'static str },
}

/// Confidence label based on sample size.
pub fn confidence_label(total_calls: u32) -> &'static str {
    if total_calls < 20 {
        "Low"
    } else if total_calls <= 100 {
        "Medium"
    } else {
        "High"
    }
}

/// Exponential Moving Average of latencies with given alpha.
/// `ema[0] = latencies[0]; ema[i] = alpha * latencies[i] + (1-alpha) * ema[i-1]`
pub fn compute_ema_latency(latencies: &[f64], alpha: f64) -> f64 {
    if latencies.is_empty() {
        return 0.0;
    }
    let mut ema = latencies[0];
    for &lat in &latencies[1..] {
        ema = alpha * lat + (1.0 - alpha) * ema;
    }
    ema
}

/// Aggregate telemetry entries by provider field.
pub fn aggregate_providers(entries: &[TelemetryEntry]) -> Vec<ProviderHealthSummary> {
    use std::collections::BTreeMap;

    struct ProviderAccum {
        total_calls: u32,
        success_count: u32,
        total_latency_ms: u64,
        latencies_ordered: Vec<f64>,
    }

    let mut map: BTreeMap<String, ProviderAccum> = BTreeMap::new();

    for entry in entries {
        let provider_id = entry.provider.clone().unwrap_or_else(|| "unknown".to_string());
        let accum = map.entry(provider_id).or_insert_with(|| ProviderAccum {
            total_calls: 0,
            success_count: 0,
            total_latency_ms: 0,
            latencies_ordered: Vec::new(),
        });
        accum.total_calls += 1;
        if entry.success {
            accum.success_count += 1;
        }
        accum.total_latency_ms += entry.latency_ms;
        accum.latencies_ordered.push(entry.latency_ms as f64);
    }

    map.into_iter()
        .map(|(id, accum)| {
            let success_rate = if accum.total_calls == 0 {
                1.0
            } else {
                f64::from(accum.success_count) / f64::from(accum.total_calls)
            };
            let avg_latency_ms = if accum.total_calls == 0 {
                0.0
            } else {
                accum.total_latency_ms as f64 / f64::from(accum.total_calls)
            };
            let ema_latency_ms = compute_ema_latency(&accum.latencies_ordered, 0.3);
            let score = success_rate * 100.0 - avg_latency_ms * 0.05;
            let status = if success_rate < 0.85 {
                ProviderStatus::Degraded { reason: "Low success rate" }
            } else {
                ProviderStatus::Healthy
            };
            ProviderHealthSummary {
                id,
                total_calls: accum.total_calls,
                success_rate,
                avg_latency_ms,
                ema_latency_ms,
                score,
                status,
            }
        })
        .collect()
}

/// Return the `limit` most recent entries that have a provider (routing decisions).
pub fn recent_routing_decisions<'a>(
    entries: &'a [TelemetryEntry],
    limit: usize,
) -> Vec<&'a TelemetryEntry> {
    let mut with_provider: Vec<&TelemetryEntry> = entries
        .iter()
        .filter(|e| e.provider.is_some())
        .collect();
    // Sort by timestamp descending (lexicographic is fine for ISO 8601).
    with_provider.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
    with_provider.into_iter().take(limit).collect()
}

/// Return the `limit` most recent failed entries.
pub fn recent_failures<'a>(
    entries: &'a [TelemetryEntry],
    limit: usize,
) -> Vec<&'a TelemetryEntry> {
    let mut failures: Vec<&TelemetryEntry> =
        entries.iter().filter(|e| !e.success).collect();
    failures.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
    failures.into_iter().take(limit).collect()
}

/// Render the full providers dashboard as a String.
pub fn render_providers_dashboard(entries: &[TelemetryEntry], verbose: bool) -> String {
    let summaries = aggregate_providers(entries);

    if summaries.is_empty() {
        return "No provider telemetry found yet.\n".to_string();
    }

    // Find leader (highest score).
    let leader = summaries
        .iter()
        .max_by(|a, b| a.score.partial_cmp(&b.score).unwrap_or(std::cmp::Ordering::Equal));

    let mut out = String::new();
    out.push_str("Provider Health & Orchestration State\n");
    out.push_str(&"-".repeat(50));
    out.push('\n');

    if let Some(l) = leader {
        let conf = confidence_label(l.total_calls);
        out.push_str(&format!(
            "  Leader: {} (Score: {:.1}) [{conf} Confidence]\n",
            l.id, l.score
        ));
    }
    out.push_str(&"-".repeat(50));
    out.push('\n');

    // Header
    let h_provider = "Provider";
    let h_status = "Status";
    let h_ema = "EMA Latency";
    let h_success = "Success Rate";

    // Column widths
    let w_provider = summaries.iter().map(|s| s.id.len()).max().unwrap_or(8).max(h_provider.len());
    let w_status = summaries
        .iter()
        .map(|s| match s.status {
            ProviderStatus::Healthy => "Healthy".len(),
            ProviderStatus::Degraded { reason } => reason.len(),
        })
        .max()
        .unwrap_or(7)
        .max(h_status.len());
    let w_ema = h_ema.len();
    let w_success = h_success.len();

    out.push_str(&format!(
        "  {:<w_provider$} | {:<w_status$} | {:<w_ema$} | {:<w_success$}\n",
        h_provider, h_status, h_ema, h_success
    ));
    out.push_str(&format!(
        "  {}\n",
        "-".repeat(w_provider + w_status + w_ema + w_success + 9)
    ));

    for s in &summaries {
        let status_str = match s.status {
            ProviderStatus::Healthy => "Healthy".to_string(),
            ProviderStatus::Degraded { reason } => reason.to_string(),
        };
        out.push_str(&format!(
            "  {:<w_provider$} | {:<w_status$} | {:>w_ema$} | {:>w_success$}\n",
            s.id,
            status_str,
            format!("{:.0}ms", s.ema_latency_ms),
            format!("{:.1}%", s.success_rate * 100.0),
        ));
    }
    out.push_str(&"-".repeat(50));
    out.push('\n');

    if verbose {
        // Recent routing decisions
        let decisions = recent_routing_decisions(entries, 5);
        if !decisions.is_empty() {
            out.push_str("  Recent Routing Decisions\n");
            for e in decisions {
                let time = if e.timestamp.len() >= 19 {
                    &e.timestamp[11..19]
                } else {
                    &e.timestamp
                };
                let provider = e.provider.as_deref().unwrap_or("?");
                out.push_str(&format!(
                    "  [{time}] {provider} chosen ({} in / {} out tokens)\n",
                    e.input_tokens, e.output_tokens
                ));
            }
            out.push_str(&"-".repeat(50));
            out.push('\n');
        }

        // Recent failures
        let failures = recent_failures(entries, 5);
        if !failures.is_empty() {
            out.push_str("  Recent Failures\n");
            for e in failures {
                let time = if e.timestamp.len() >= 19 {
                    &e.timestamp[11..19]
                } else {
                    &e.timestamp
                };
                let provider = e.provider.as_deref().unwrap_or("?");
                let err = e.error_type.as_deref().unwrap_or("unknown error");
                out.push_str(&format!("  [{time}] {provider} | {err}\n"));
            }
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_entry(
        provider: Option<&str>,
        success: bool,
        latency_ms: u64,
        timestamp: &str,
    ) -> TelemetryEntry {
        TelemetryEntry {
            timestamp: timestamp.to_string(),
            session_id: "sess".to_string(),
            project: "proj".to_string(),
            model: "claude-sonnet".to_string(),
            input_tokens: 100,
            output_tokens: 50,
            cache_write_tokens: 0,
            cache_read_tokens: 0,
            cost_usd: 0.001,
            latency_ms,
            success,
            provider: provider.map(str::to_string),
            error_type: if success { None } else { Some("RateLimit".to_string()) },
        }
    }

    #[test]
    fn test_ema_latency_computation() {
        // ema[0] = 100; ema[1] = 0.3*200 + 0.7*100 = 130; ema[2] = 0.3*300 + 0.7*130 = 181
        let lats = vec![100.0, 200.0, 300.0];
        let ema = compute_ema_latency(&lats, 0.3);
        let expected = 0.3 * 300.0 + 0.7 * (0.3 * 200.0 + 0.7 * 100.0);
        assert!((ema - expected).abs() < 1e-9, "ema={ema} expected={expected}");
    }

    #[test]
    fn test_score_calculation() {
        // success_rate=0.95, avg_latency=400 -> score = 95.0 - 400*0.05 = 95 - 20 = 75
        let entries = vec![
            make_entry(Some("provider-x"), true, 400, "2026-04-27T10:00:00Z"),
            make_entry(Some("provider-x"), true, 400, "2026-04-27T10:01:00Z"),
            make_entry(Some("provider-x"), true, 400, "2026-04-27T10:02:00Z"),
            make_entry(Some("provider-x"), true, 400, "2026-04-27T10:03:00Z"),
            make_entry(Some("provider-x"), true, 400, "2026-04-27T10:04:00Z"),
            make_entry(Some("provider-x"), true, 400, "2026-04-27T10:05:00Z"),
            make_entry(Some("provider-x"), true, 400, "2026-04-27T10:06:00Z"),
            make_entry(Some("provider-x"), true, 400, "2026-04-27T10:07:00Z"),
            make_entry(Some("provider-x"), true, 400, "2026-04-27T10:08:00Z"),
            make_entry(Some("provider-x"), true, 400, "2026-04-27T10:09:00Z"),
            make_entry(Some("provider-x"), true, 400, "2026-04-27T10:10:00Z"),
            make_entry(Some("provider-x"), true, 400, "2026-04-27T10:11:00Z"),
            make_entry(Some("provider-x"), true, 400, "2026-04-27T10:12:00Z"),
            make_entry(Some("provider-x"), true, 400, "2026-04-27T10:13:00Z"),
            make_entry(Some("provider-x"), true, 400, "2026-04-27T10:14:00Z"),
            make_entry(Some("provider-x"), true, 400, "2026-04-27T10:15:00Z"),
            make_entry(Some("provider-x"), true, 400, "2026-04-27T10:16:00Z"),
            make_entry(Some("provider-x"), true, 400, "2026-04-27T10:17:00Z"),
            make_entry(Some("provider-x"), true, 400, "2026-04-27T10:18:00Z"),
            make_entry(Some("provider-x"), false, 400, "2026-04-27T10:19:00Z"),
        ];
        let summaries = aggregate_providers(&entries);
        let s = summaries.iter().find(|x| x.id == "provider-x").unwrap();
        // success_rate = 19/20 = 0.95, avg_latency = 400ms, score = 95 - 20 = 75
        let expected_score = 0.95 * 100.0 - 400.0 * 0.05;
        assert!((s.score - expected_score).abs() < 1e-6, "score={} expected={expected_score}", s.score);
    }

    #[test]
    fn test_aggregate_groups_by_provider() {
        let entries = vec![
            make_entry(Some("anthropic"), true, 300, "2026-04-27T10:00:00Z"),
            make_entry(Some("anthropic"), true, 350, "2026-04-27T10:01:00Z"),
            make_entry(Some("anthropic"), true, 320, "2026-04-27T10:02:00Z"),
            make_entry(Some("openai"), true, 450, "2026-04-27T10:03:00Z"),
            make_entry(Some("openai"), false, 500, "2026-04-27T10:04:00Z"),
        ];
        let summaries = aggregate_providers(&entries);
        assert_eq!(summaries.len(), 2);
        let anthropic = summaries.iter().find(|s| s.id == "anthropic").unwrap();
        assert_eq!(anthropic.total_calls, 3);
        let openai = summaries.iter().find(|s| s.id == "openai").unwrap();
        assert_eq!(openai.total_calls, 2);
    }

    #[test]
    fn test_degraded_status_low_success() {
        // 1 success out of 10 = 10% success rate → Degraded
        let mut entries = Vec::new();
        for i in 0..9 {
            entries.push(make_entry(
                Some("bad-provider"),
                false,
                500,
                &format!("2026-04-27T10:{i:02}:00Z"),
            ));
        }
        entries.push(make_entry(Some("bad-provider"), true, 500, "2026-04-27T10:09:00Z"));

        let summaries = aggregate_providers(&entries);
        let s = summaries.iter().find(|x| x.id == "bad-provider").unwrap();
        assert!(s.success_rate < 0.85, "success_rate={}", s.success_rate);
        assert!(
            matches!(s.status, ProviderStatus::Degraded { .. }),
            "expected Degraded, got {:?}",
            s.status
        );
    }
}
