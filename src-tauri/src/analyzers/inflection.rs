//! Inflection-point detector: surfaces days where the user's effective
//! cost-per-token or cost-per-session shifted by ≥2× versus the prior rolling
//! baseline. These points are the "something broke / something changed" flags.

use cc_discord_presence::provider::Provider;
use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::db::HistoricalSession;

#[derive(Debug, Clone, Serialize)]
pub struct InflectionPoint {
    pub date: String,
    pub multiplier: f64,
    pub direction: &'static str,
    pub sessions_on_day: usize,
    pub observed_sessions_on_day: usize,
    pub cost_basis: crate::db::CostBasis,
    pub cost_sources: Vec<String>,
    pub cost_on_day: f64,
    pub baseline_cost: f64,
    pub note: String,
}

/// Detect cost-per-session inflections day-over-day. Returns points sorted
/// most-impactful first (largest multiplier).
pub fn detect(sessions: &[HistoricalSession]) -> Vec<InflectionPoint> {
    detect_for_provider(Provider::Claude, sessions)
}

pub fn detect_for_provider(
    provider: Provider,
    sessions: &[HistoricalSession],
) -> Vec<InflectionPoint> {
    use std::collections::BTreeMap;

    let mut grouped: BTreeMap<String, Vec<&HistoricalSession>> = BTreeMap::new();
    for s in sessions {
        let Some(started) = s.started_at.as_deref() else {
            continue;
        };
        let Ok(dt) = DateTime::parse_from_rfc3339(started) else {
            continue;
        };
        let day = dt.with_timezone(&Utc).format("%Y-%m-%d").to_string();
        grouped.entry(day).or_default().push(s);
    }
    let by_day = grouped
        .into_iter()
        .map(|(day, sessions)| {
            let coverage = crate::db::summarize_cost_provenance(sessions.iter().map(|session| {
                (
                    session.cost_basis,
                    session.cost_source.as_str(),
                    session.known_cost,
                )
            }));
            let cost = sessions
                .iter()
                .filter_map(|session| {
                    session.known_cost.filter(|cost| {
                        session.cost_basis != crate::db::CostBasis::Unavailable
                            && cost.is_finite()
                            && *cost >= 0.0
                    })
                })
                .sum::<f64>();
            (
                day,
                (
                    cost,
                    coverage.priced_sessions,
                    coverage.sessions,
                    coverage.cost_basis,
                    coverage.cost_sources,
                ),
            )
        })
        .collect::<BTreeMap<_, _>>();

    if by_day.len() < 3 {
        return Vec::new();
    }

    let mut points = Vec::new();
    let days: Vec<_> = by_day.into_iter().collect();

    // Rolling 3-day baseline (excluding the current day).
    for i in 3..days.len() {
        let (day, (cost_today, sessions_today, observed_today, basis, sources)) = &days[i];
        if *sessions_today == 0
            || *sessions_today != *observed_today
            || *basis != crate::db::CostBasis::Exact
        {
            continue;
        }
        let per_session_today = cost_today / *sessions_today as f64;
        let window = &days[i - 3..i];
        let mut baseline_cost = 0.0;
        let mut baseline_sessions = 0usize;
        let baseline_is_exact = window.iter().all(|(_, (_, priced, observed, basis, _))| {
            *priced == *observed && *basis == crate::db::CostBasis::Exact
        });
        if !baseline_is_exact {
            continue;
        }
        for (_, (c, n, _, _, _)) in window {
            baseline_cost += c;
            baseline_sessions += n;
        }
        if baseline_sessions == 0 {
            continue;
        }
        let per_session_baseline = baseline_cost / baseline_sessions as f64;
        if per_session_baseline < 0.01 {
            continue;
        }
        let multiplier = per_session_today / per_session_baseline;
        let instruction_file = provider.instruction_file_name();
        let (direction, threshold_ok, note) = if multiplier >= 2.0 {
            (
                "spike",
                true,
                format!(
                    "Cost/session jumped {:.1}× versus the prior 3-day average — \
                    worth checking what changed ({instruction_file}, model, or task complexity).",
                    multiplier
                ),
            )
        } else if multiplier <= 0.5 {
            (
                "drop",
                true,
                format!(
                    "Cost/session dropped to {:.1}× baseline — efficiency win. If this \
                    was intentional (e.g. {instruction_file} trim), keep it.",
                    multiplier
                ),
            )
        } else {
            ("", false, String::new())
        };
        if threshold_ok {
            points.push(InflectionPoint {
                date: day.clone(),
                multiplier,
                direction,
                sessions_on_day: *sessions_today,
                observed_sessions_on_day: *observed_today,
                cost_basis: *basis,
                cost_sources: sources.clone(),
                cost_on_day: *cost_today,
                baseline_cost: per_session_baseline * *sessions_today as f64,
                note,
            });
        }
    }

    points.sort_by(|a, b| {
        let sig_a = (a.multiplier - 1.0).abs();
        let sig_b = (b.multiplier - 1.0).abs();
        sig_b
            .partial_cmp(&sig_a)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    points
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_returns_no_points() {
        assert!(detect(&[]).is_empty());
    }

    #[test]
    fn unavailable_raw_cost_cannot_create_a_false_spike() {
        let mut sessions = Vec::new();
        for day in 1..=4 {
            sessions.push(HistoricalSession {
                started_at: Some(format!("2026-08-0{day}T12:00:00+00:00")),
                total_cost: 1.0,
                known_cost: Some(1.0),
                cost_basis: crate::db::CostBasis::Exact,
                cost_source: "session-calculated".into(),
                ..HistoricalSession::default()
            });
        }
        sessions.push(HistoricalSession {
            started_at: Some("2026-08-04T13:00:00+00:00".into()),
            total_cost: 100.0,
            known_cost: None,
            cost_basis: crate::db::CostBasis::Unavailable,
            cost_source: "unknown".into(),
            ..HistoricalSession::default()
        });

        assert!(detect(&sessions).is_empty());
    }
}
