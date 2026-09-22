//! Model Routing analyzer — quantifies how the user's spend splits across
//! Opus / Sonnet / Haiku and estimates savings from rerouting cheap tasks.

use serde::Serialize;

use crate::db::{CostBasis, HistoricalSession};

#[derive(Debug, Clone, Serialize)]
pub struct ModelRoutingReport {
    pub total_sessions: usize,
    pub priced_sessions: usize,
    pub cost_basis: CostBasis,
    pub cost_sources: Vec<String>,
    pub total_cost: f64,
    pub opus: FamilyStats,
    pub sonnet: FamilyStats,
    pub haiku: FamilyStats,
    pub other: FamilyStats,
    pub savings_estimate_available: bool,
    pub estimated_savings_if_rerouted: f64,
    pub diagnosis: String,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct FamilyStats {
    pub sessions: usize,
    pub priced_sessions: usize,
    pub cost: f64,
    pub cost_share_pct: f64,
    pub avg_cost_per_session: f64,
}

fn classify(model_id: &str) -> &'static str {
    let lower = model_id.to_ascii_lowercase();
    if lower.contains("opus") {
        "opus"
    } else if lower.contains("sonnet") {
        "sonnet"
    } else if lower.contains("haiku") {
        "haiku"
    } else {
        "other"
    }
}

pub fn analyze(sessions: &[HistoricalSession]) -> ModelRoutingReport {
    let mut opus = FamilyStats::default();
    let mut sonnet = FamilyStats::default();
    let mut haiku = FamilyStats::default();
    let mut other = FamilyStats::default();
    let mut total_cost = 0.0;
    let coverage = crate::db::summarize_cost_provenance(sessions.iter().map(|session| {
        (
            session.cost_basis,
            session.cost_source.as_str(),
            session.known_cost,
        )
    }));

    for s in sessions {
        let family = classify(&s.model_id);
        let slot = match family {
            "opus" => &mut opus,
            "sonnet" => &mut sonnet,
            "haiku" => &mut haiku,
            _ => &mut other,
        };
        slot.sessions += 1;
        let Some(cost) = s.known_cost.filter(|cost| {
            s.cost_basis != CostBasis::Unavailable && cost.is_finite() && *cost >= 0.0
        }) else {
            continue;
        };
        slot.priced_sessions += 1;
        slot.cost += cost;
        total_cost += cost;
    }

    for f in [&mut opus, &mut sonnet, &mut haiku, &mut other] {
        f.cost_share_pct = if total_cost > 0.0 {
            (f.cost / total_cost) * 100.0
        } else {
            0.0
        };
        f.avg_cost_per_session = if f.priced_sessions > 0 {
            f.cost / f.priced_sessions as f64
        } else {
            0.0
        };
    }

    // Conservative savings estimate: assume 30% of Opus spend is on tasks that
    // could have been handled by Sonnet (4:1 Opus:Sonnet rate delta on inputs,
    // 5:3 on outputs → average ~5× cheaper). Keep the heuristic conservative.
    let savings_estimate_available = coverage.cost_basis == CostBasis::Exact;
    let estimated_savings_if_rerouted = if savings_estimate_available && opus.cost_share_pct > 60.0
    {
        opus.cost * 0.30 * 0.80
    } else {
        0.0
    };

    let diagnosis = if coverage.cost_basis == CostBasis::Partial {
        format!(
            "Known cost covers {} of {} sessions, so routing shares are partial and the savings estimate is withheld. {}",
            coverage.priced_sessions,
            sessions.len(),
            diagnose(&opus, &sonnet, &haiku, total_cost)
        )
    } else {
        diagnose(&opus, &sonnet, &haiku, total_cost)
    };

    ModelRoutingReport {
        total_sessions: sessions.len(),
        priced_sessions: coverage.priced_sessions,
        cost_basis: coverage.cost_basis,
        cost_sources: coverage.cost_sources,
        total_cost,
        opus,
        sonnet,
        haiku,
        other,
        savings_estimate_available,
        estimated_savings_if_rerouted,
        diagnosis,
    }
}

fn diagnose(opus: &FamilyStats, sonnet: &FamilyStats, haiku: &FamilyStats, total: f64) -> String {
    if total == 0.0 {
        return "No model-cost data yet.".to_string();
    }
    if opus.cost_share_pct >= 90.0 {
        format!(
            "Opus is carrying {:.0}% of your spend. Route simple lookups and one-shot rewrites \
            to Sonnet or Haiku — the savings are usually immediate.",
            opus.cost_share_pct
        )
    } else if opus.cost_share_pct >= 60.0 {
        format!(
            "Opus {:.0}% · Sonnet {:.0}% · Haiku {:.0}%. Healthy for deep work but there's headroom \
            to delegate quick refactors or greps to Sonnet.",
            opus.cost_share_pct, sonnet.cost_share_pct, haiku.cost_share_pct
        )
    } else if haiku.cost_share_pct >= 10.0 {
        "Nice mix across Opus / Sonnet / Haiku — you're already routing cheap tasks downward."
            .to_string()
    } else {
        format!(
            "Opus {:.0}% · Sonnet {:.0}% · Haiku {:.0}%. Consider adding Haiku for the smallest \
            tasks (commit messages, trivial edits).",
            opus.cost_share_pct, sonnet.cost_share_pct, haiku.cost_share_pct
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_covers_families() {
        assert_eq!(classify("claude-opus-4-7"), "opus");
        assert_eq!(classify("claude-opus-5"), "opus");
        assert_eq!(classify("claude-opus-5-20260724"), "opus");
        assert_eq!(classify("claude-opus-5[1m]"), "opus");
        assert_eq!(classify("claude-opus-5-5"), "opus");
        assert_eq!(classify("claude-sonnet-4-6"), "sonnet");
        assert_eq!(classify("claude-haiku-4-5"), "haiku");
        assert_eq!(classify("mystery-model"), "other");
    }

    #[test]
    fn empty_sessions_produce_zero_report() {
        let report = analyze(&[]);
        assert_eq!(report.total_sessions, 0);
        assert_eq!(report.total_cost, 0.0);
        assert_eq!(report.opus.sessions, 0);
    }

    #[test]
    fn unavailable_raw_cost_is_excluded_and_coverage_stays_partial() {
        let exact = HistoricalSession {
            model_id: "claude-opus-5".into(),
            total_cost: 2.5,
            known_cost: Some(2.5),
            cost_basis: crate::db::CostBasis::Exact,
            cost_source: "session-calculated".into(),
            ..HistoricalSession::default()
        };
        let unavailable = HistoricalSession {
            model_id: "claude-opus-5".into(),
            total_cost: 99.0,
            known_cost: None,
            cost_basis: crate::db::CostBasis::Unavailable,
            cost_source: "unknown".into(),
            ..HistoricalSession::default()
        };

        let report = analyze(&[exact, unavailable]);

        assert_eq!(report.total_sessions, 2);
        assert_eq!(report.priced_sessions, 1);
        assert_eq!(report.cost_basis, crate::db::CostBasis::Partial);
        assert_eq!(report.total_cost, 2.5);
        assert_eq!(report.opus.sessions, 2);
        assert_eq!(report.opus.priced_sessions, 1);
        assert_eq!(report.opus.avg_cost_per_session, 2.5);
        assert_eq!(report.estimated_savings_if_rerouted, 0.0);
        assert!(!report.savings_estimate_available);
        assert!(
            report
                .diagnosis
                .contains("Known cost covers 1 of 2 sessions")
        );
    }
}
