use crate::db;

struct ReportData {
    sessions: Vec<db::HistoricalSession>,
    daily: Vec<db::DailyStat>,
    summary: db::AnalyticsSummary,
    projects: Vec<db::ProjectStat>,
    forecast: db::CostForecast,
    hourly: Vec<db::HourlyActivity>,
    models: Vec<db::ModelStat>,
}

fn known_session_cost(session: &db::HistoricalSession) -> Option<f64> {
    session.known_cost.filter(|cost| {
        session.cost_basis != db::CostBasis::Unavailable && cost.is_finite() && *cost >= 0.0
    })
}

fn merge_cost_basis(current: db::CostBasis, next: db::CostBasis) -> db::CostBasis {
    use db::CostBasis::{Estimated, Exact, Partial, Unavailable};
    match (current, next) {
        (Unavailable, basis) => basis,
        (basis, Unavailable) => basis,
        (Exact, Exact) => Exact,
        (Estimated, Estimated) => Estimated,
        _ => Partial,
    }
}

fn observe_aggregate_cost(
    session: &db::HistoricalSession,
    total_cost: &mut f64,
    priced_sessions: &mut i64,
    cost_basis: &mut db::CostBasis,
    cost_sources: &mut Vec<String>,
) {
    let Some(cost) = known_session_cost(session) else {
        return;
    };
    *total_cost += cost;
    *priced_sessions += 1;
    *cost_basis = merge_cost_basis(*cost_basis, session.cost_basis);
    let source = session.cost_source.trim();
    if !source.is_empty() && source != "unknown" && !cost_sources.iter().any(|item| item == source)
    {
        cost_sources.push(source.to_string());
    }
}

fn finish_aggregate_cost(
    sessions: i64,
    priced_sessions: i64,
    cost_basis: &mut db::CostBasis,
    cost_sources: &mut Vec<String>,
) {
    if priced_sessions == 0 {
        *cost_basis = db::CostBasis::Unavailable;
    } else if priced_sessions != sessions {
        *cost_basis = db::CostBasis::Partial;
    }
    cost_sources.sort();
    cost_sources.dedup();
}

fn load_report_data(days: i64, project: Option<&str>, provider: &str) -> ReportData {
    let sessions = db::get_session_history_scoped(Some(provider), Some(days), project, Some(5000));
    if project.is_none() {
        return ReportData {
            daily: db::get_daily_stats_scoped(Some(provider), Some(days)),
            summary: db::get_analytics_summary_scoped(Some(provider)),
            projects: db::get_project_stats_scoped(Some(provider), Some(days)),
            forecast: db::get_cost_forecast_scoped(Some(provider)),
            hourly: db::get_hourly_activity_scoped(Some(provider), Some(days)),
            models: db::get_model_distribution_scoped(Some(provider), Some(days)),
            sessions,
        };
    }

    use chrono::{Datelike, Timelike, Utc};
    use std::collections::{BTreeMap, HashMap, HashSet};

    let now = Utc::now();
    let days_elapsed = now.day() as i64;
    let days_in_month = {
        let (y, m) = (now.year(), now.month());
        if m == 12 {
            chrono::NaiveDate::from_ymd_opt(y + 1, 1, 1)
        } else {
            chrono::NaiveDate::from_ymd_opt(y, m + 1, 1)
        }
        .and_then(|d| d.pred_opt())
        .map(|d| d.day() as i64)
        .unwrap_or(30)
    };
    let month_start = now.format("%Y-%m-01T00:00:00+00:00").to_string();

    let mut daily_map: BTreeMap<(String, String, String), db::DailyStat> = BTreeMap::new();
    let mut hourly_map: BTreeMap<i64, db::HourlyActivity> = BTreeMap::new();
    let mut project_map: HashMap<String, db::ProjectStat> = HashMap::new();
    let mut model_map: HashMap<String, db::ModelStat> = HashMap::new();
    let mut tracked_days: HashSet<String> = HashSet::new();
    let mut top_model_counts: HashMap<String, i64> = HashMap::new();

    for session in &sessions {
        let ts = session
            .started_at
            .as_deref()
            .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
            .map(|value| value.with_timezone(&Utc));
        let date = ts
            .map(|value| value.format("%Y-%m-%d").to_string())
            .unwrap_or_else(|| now.format("%Y-%m-%d").to_string());
        tracked_days.insert(date.clone());

        let daily_key = (date.clone(), session.project.clone(), session.model.clone());
        let entry = daily_map.entry(daily_key).or_insert(db::DailyStat {
            date: date.clone(),
            project: session.project.clone(),
            model: session.model.clone(),
            session_count: 0,
            priced_sessions: 0,
            cost_basis: db::CostBasis::Unavailable,
            cost_sources: Vec::new(),
            total_cost: 0.0,
            total_tokens: 0,
            input_tokens: 0,
            output_tokens: 0,
            cache_write_tokens: 0,
            cache_read_tokens: 0,
        });
        entry.session_count += 1;
        observe_aggregate_cost(
            session,
            &mut entry.total_cost,
            &mut entry.priced_sessions,
            &mut entry.cost_basis,
            &mut entry.cost_sources,
        );
        entry.total_tokens += session.total_tokens;
        entry.input_tokens += session.input_tokens;
        entry.output_tokens += session.output_tokens;
        entry.cache_write_tokens += session.cache_write_tokens;
        entry.cache_read_tokens += session.cache_read_tokens;

        let project_entry = project_map
            .entry(session.project.clone())
            .or_insert(db::ProjectStat {
                project: session.project.clone(),
                session_count: 0,
                priced_sessions: 0,
                cost_basis: db::CostBasis::Unavailable,
                cost_sources: Vec::new(),
                total_cost: 0.0,
                total_tokens: 0,
                avg_session_cost: 0.0,
                avg_duration_secs: 0.0,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
                top_model: String::new(),
            });
        project_entry.session_count += 1;
        observe_aggregate_cost(
            session,
            &mut project_entry.total_cost,
            &mut project_entry.priced_sessions,
            &mut project_entry.cost_basis,
            &mut project_entry.cost_sources,
        );
        project_entry.total_tokens += session.total_tokens;
        project_entry.avg_duration_secs += session.duration_secs as f64;
        project_entry.cache_read_tokens += session.cache_read_tokens;
        project_entry.cache_write_tokens += session.cache_write_tokens;

        let model_entry = model_map
            .entry(session.model.clone())
            .or_insert(db::ModelStat {
                model: session.model.clone(),
                session_count: 0,
                priced_sessions: 0,
                cost_basis: db::CostBasis::Unavailable,
                cost_sources: Vec::new(),
                total_cost: 0.0,
            });
        model_entry.session_count += 1;
        observe_aggregate_cost(
            session,
            &mut model_entry.total_cost,
            &mut model_entry.priced_sessions,
            &mut model_entry.cost_basis,
            &mut model_entry.cost_sources,
        );
        *top_model_counts.entry(session.model.clone()).or_insert(0) += 1;

        if let Some(ts) = ts {
            let hour = i64::from(ts.hour() as i32);
            let hourly = hourly_map.entry(hour).or_insert(db::HourlyActivity {
                hour,
                session_count: 0,
                priced_sessions: 0,
                cost_basis: db::CostBasis::Unavailable,
                cost_sources: Vec::new(),
                total_cost: 0.0,
            });
            hourly.session_count += 1;
            observe_aggregate_cost(
                session,
                &mut hourly.total_cost,
                &mut hourly.priced_sessions,
                &mut hourly.cost_basis,
                &mut hourly.cost_sources,
            );
        }
    }

    for stat in daily_map.values_mut() {
        finish_aggregate_cost(
            stat.session_count,
            stat.priced_sessions,
            &mut stat.cost_basis,
            &mut stat.cost_sources,
        );
    }
    for stat in hourly_map.values_mut() {
        finish_aggregate_cost(
            stat.session_count,
            stat.priced_sessions,
            &mut stat.cost_basis,
            &mut stat.cost_sources,
        );
    }
    let mut projects: Vec<db::ProjectStat> = project_map
        .into_values()
        .map(|mut stat| {
            finish_aggregate_cost(
                stat.session_count,
                stat.priced_sessions,
                &mut stat.cost_basis,
                &mut stat.cost_sources,
            );
            if stat.priced_sessions > 0 {
                stat.avg_session_cost = stat.total_cost / stat.priced_sessions as f64;
            }
            if stat.session_count > 0 {
                stat.avg_duration_secs /= stat.session_count as f64;
            }
            stat.top_model = top_model_counts
                .iter()
                .max_by_key(|(_, count)| *count)
                .map(|(model, _)| model.clone())
                .unwrap_or_default();
            stat
        })
        .collect();
    projects.sort_by(|a, b| {
        b.total_cost
            .partial_cmp(&a.total_cost)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut models: Vec<db::ModelStat> = model_map
        .into_values()
        .map(|mut stat| {
            finish_aggregate_cost(
                stat.session_count,
                stat.priced_sessions,
                &mut stat.cost_basis,
                &mut stat.cost_sources,
            );
            stat
        })
        .collect();
    models.sort_by_key(|model| std::cmp::Reverse(model.session_count));

    let total_sessions = sessions.len() as i64;
    let cost_coverage = db::summarize_cost_provenance(sessions.iter().map(|session| {
        (
            session.cost_basis,
            session.cost_source.as_str(),
            session.known_cost,
        )
    }));
    let total_cost: f64 = sessions.iter().filter_map(known_session_cost).sum();
    let total_tokens: i64 = sessions.iter().map(|s| s.total_tokens).sum();
    let total_cache_read: i64 = sessions.iter().map(|s| s.cache_read_tokens).sum();
    let total_cache_write: i64 = sessions.iter().map(|s| s.cache_write_tokens).sum();
    let avg_duration_secs = if total_sessions > 0 {
        sessions.iter().map(|s| s.duration_secs).sum::<i64>() as f64 / total_sessions as f64
    } else {
        0.0
    };
    let avg_tokens_per_session = if total_sessions > 0 {
        total_tokens as f64 / total_sessions as f64
    } else {
        0.0
    };
    let avg_cost_per_session = if cost_coverage.priced_sessions > 0 {
        total_cost / cost_coverage.priced_sessions as f64
    } else {
        0.0
    };
    let top_project = projects
        .first()
        .map(|p| p.project.clone())
        .unwrap_or_else(|| "—".into());
    let top_model = models
        .first()
        .map(|model| model.model.clone())
        .unwrap_or_else(|| "—".into());
    let month_sessions = sessions
        .iter()
        .filter(|s| {
            s.started_at
                .as_deref()
                .is_some_and(|ts| ts >= month_start.as_str())
        })
        .collect::<Vec<_>>();
    let month_cost_coverage = db::summarize_cost_provenance(month_sessions.iter().map(|session| {
        (
            session.cost_basis,
            session.cost_source.as_str(),
            session.known_cost,
        )
    }));
    let billed_month_sessions = month_sessions
        .iter()
        .filter(|session| {
            db::monetary_provenance(&session.cost_source) == db::MonetaryProvenance::ProviderBilled
                && session
                    .known_cost
                    .is_some_and(|cost| cost.is_finite() && cost >= 0.0)
        })
        .collect::<Vec<_>>();
    let api_equivalent_month_sessions = month_sessions
        .iter()
        .filter(|session| {
            db::monetary_provenance(&session.cost_source) == db::MonetaryProvenance::ApiEquivalent
                && session
                    .known_cost
                    .is_some_and(|cost| cost.is_finite() && cost >= 0.0)
        })
        .collect::<Vec<_>>();
    let billed_spend_usd = (!billed_month_sessions.is_empty()).then(|| {
        billed_month_sessions
            .iter()
            .filter_map(|session| session.known_cost)
            .sum::<f64>()
    });
    let api_equivalent_usd = (!api_equivalent_month_sessions.is_empty()).then(|| {
        api_equivalent_month_sessions
            .iter()
            .filter_map(|session| session.known_cost)
            .sum::<f64>()
    });
    let daily_billed_spend_usd = billed_spend_usd
        .filter(|_| days_elapsed > 0)
        .map(|value| value / days_elapsed as f64);
    let daily_api_equivalent_usd = api_equivalent_usd
        .filter(|_| days_elapsed > 0)
        .map(|value| value / days_elapsed as f64);
    let billed_sessions = billed_month_sessions.len();
    let api_equivalent_sessions = api_equivalent_month_sessions.len();
    drop(billed_month_sessions);
    drop(api_equivalent_month_sessions);
    drop(month_sessions);

    ReportData {
        sessions,
        daily: daily_map.into_values().collect(),
        summary: db::AnalyticsSummary {
            total_sessions,
            priced_sessions: cost_coverage.priced_sessions as i64,
            cost_basis: cost_coverage.cost_basis,
            cost_sources: cost_coverage.cost_sources,
            total_cost,
            total_tokens,
            total_cache_read,
            total_cache_write,
            avg_duration_secs,
            avg_tokens_per_session,
            avg_cost_per_session,
            top_project,
            top_model,
            days_tracked: tracked_days.len() as i64,
        },
        projects,
        forecast: db::CostForecast {
            billed_spend_usd,
            daily_billed_spend_usd,
            projected_billed_spend_usd: daily_billed_spend_usd
                .map(|value| value * days_in_month as f64),
            api_equivalent_usd,
            daily_api_equivalent_usd,
            projected_api_equivalent_usd: daily_api_equivalent_usd
                .map(|value| value * days_in_month as f64),
            days_elapsed,
            days_in_month,
            cost_basis: month_cost_coverage.cost_basis,
            cost_sources: month_cost_coverage.cost_sources,
            sessions: month_cost_coverage.sessions,
            priced_sessions: month_cost_coverage.priced_sessions,
            billed_sessions,
            api_equivalent_sessions,
            refreshed_at: now.to_rfc3339(),
        },
        hourly: hourly_map.into_values().collect(),
        models,
    }
}

/// Render the analytics report as Markdown — suitable for pasting into a
/// GitHub issue, a Slack message, or a CC session. Sections mirror the HTML
/// report: cache grade, stats, top sessions, project + model breakdowns.
pub fn generate_markdown_report(days: Option<i64>, project: Option<&str>) -> String {
    let provider = cc_discord_presence::provider::load_active_provider();
    generate_markdown_report_for_provider(provider, days, project)
}

pub fn generate_markdown_report_scoped(
    provider: Option<&str>,
    days: Option<i64>,
    project: Option<&str>,
) -> String {
    let active = cc_discord_presence::provider::load_active_provider();
    let scope = provider
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| active.as_str());
    if let Some(provider) = cc_discord_presence::provider::Provider::parse(scope) {
        return generate_markdown_report_for_provider(provider, days, project);
    }

    use std::fmt::Write as _;
    let d = days.unwrap_or(30);
    let data = load_report_data(d, project, scope);
    let coverage = db::summarize_cost_provenance(data.sessions.iter().map(|session| {
        (
            session.cost_basis,
            session.cost_source.as_str(),
            session.known_cost,
        )
    }));
    let label = match scope {
        "all" => "All providers",
        "openai" => "OpenAI API",
        "anthropic" => "Anthropic API",
        _ => "Selected provider",
    };
    let mut markdown = String::new();
    writeln!(markdown, "# Pulse Analytics Report").unwrap();
    writeln!(markdown).unwrap();
    writeln!(markdown, "- Provider scope: {label}").unwrap();
    writeln!(markdown, "- Period: last {d} days").unwrap();
    writeln!(
        markdown,
        "- Project: {}",
        project
            .map(markdown_escape)
            .unwrap_or_else(|| "All projects".to_string())
    )
    .unwrap();
    writeln!(markdown).unwrap();
    writeln!(markdown, "## Summary").unwrap();
    writeln!(markdown, "- Sessions: {}", data.sessions.len()).unwrap();
    writeln!(markdown, "- Tokens: {}", data.summary.total_tokens).unwrap();
    writeln!(
        markdown,
        "- Known monetary value: {} ({})",
        format_cost(data.summary.total_cost),
        format_cost_coverage(&coverage)
    )
    .unwrap();
    if coverage.cost_basis == db::CostBasis::Unavailable {
        writeln!(
            markdown,
            "- Cost note: no exact provider cost is available for this scope."
        )
        .unwrap();
    }
    writeln!(markdown).unwrap();
    writeln!(
        markdown,
        "Provider-specific recommendations are available after selecting one subscription provider."
    )
    .unwrap();
    writeln!(markdown).unwrap();
    writeln!(markdown, "---").unwrap();
    writeln!(markdown, "Generated by Pulse (cc-discord-presence)").unwrap();
    markdown
}

pub fn generate_markdown_report_for_provider(
    provider: cc_discord_presence::provider::Provider,
    days: Option<i64>,
    project: Option<&str>,
) -> String {
    use super::analyzers::{
        cache_health, inflection, model_routing, prompt_complexity, session_trace, tool_frequency,
        trace_overview,
    };
    use std::fmt::Write as _;

    let d = days.unwrap_or(30);
    let data = load_report_data(d, project, provider.as_str());
    let sessions = data.sessions;
    let projects = data.projects;
    let models = data.models;
    let forecast = data.forecast;
    let summary = data.summary;

    let total_sessions = sessions.len();
    let cost_coverage = db::summarize_cost_provenance(sessions.iter().map(|session| {
        (
            session.cost_basis,
            session.cost_source.as_str(),
            session.known_cost,
        )
    }));
    let total_cost: f64 = sessions.iter().filter_map(known_session_cost).sum();
    let total_tokens: i64 = sessions.iter().map(|s| s.total_tokens).sum();

    let capabilities = provider.capabilities();
    let cache = cache_health::analyze_for_provider(provider, &sessions);
    let routing = capabilities
        .model_routing
        .then(|| model_routing::analyze(&sessions));
    let inflections = inflection::detect_for_provider(provider, &sessions);
    let traces = session_trace::load_session_traces(&sessions);
    let tool_frequency = tool_frequency::analyze(&sessions, &traces);
    let prompt_complexity = prompt_complexity::analyze(&sessions, &traces);
    let trace_overview =
        trace_overview::build(provider, &sessions, &traces, cache.trend_weighted_ratio);

    let mut top_sessions: Vec<_> = sessions.iter().collect();
    top_sessions.sort_by(|a, b| {
        known_session_cost(b)
            .unwrap_or_default()
            .partial_cmp(&known_session_cost(a).unwrap_or_default())
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let period = project
        .map(|p| format!("{} — last {d} days", markdown_escape(p)))
        .unwrap_or_else(|| format!("All projects — last {d} days"));
    let generated = chrono::Utc::now().format("%Y-%m-%d %H:%M UTC");
    let truncate = |value: &str, max_chars: usize| {
        let mut out = String::new();
        for (idx, ch) in value.chars().enumerate() {
            if idx >= max_chars {
                out.push('…');
                break;
            }
            out.push(ch);
        }
        out
    };

    let mut md = String::new();
    writeln!(md, "# Pulse Analytics Report\n").unwrap();
    writeln!(md, "{period}\n").unwrap();
    writeln!(md, "Generated {generated}\n").unwrap();

    writeln!(md, "## Executive Summary\n").unwrap();
    writeln!(md, "| Metric | Value |\n|---|---|").unwrap();
    writeln!(md, "| Known monetary value | {} |", format_cost(total_cost)).unwrap();
    writeln!(
        md,
        "| Cost coverage | {} |\n",
        markdown_escape(&format_cost_coverage(&cost_coverage))
    )
    .unwrap();
    writeln!(md, "| Sessions | {} |", total_sessions).unwrap();
    writeln!(md, "| Tokens | {} |", format_tokens_short(total_tokens)).unwrap();
    writeln!(
        md,
        "| Cache grade | {} ({:.1}% weighted hit ratio) |",
        cache.grade, cache.trend_weighted_ratio
    )
    .unwrap();
    writeln!(
        md,
        "| Provider-billed month to date | {} |",
        forecast
            .billed_spend_usd
            .map(format_cost)
            .unwrap_or_else(|| "Unavailable".to_string())
    )
    .unwrap();
    writeln!(
        md,
        "| API-equivalent month to date | {} |",
        forecast
            .api_equivalent_usd
            .map(format_cost)
            .unwrap_or_else(|| "Unavailable".to_string())
    )
    .unwrap();
    writeln!(
        md,
        "| All-time tracked | {} sessions over {} days |\n",
        summary.total_sessions, summary.days_tracked
    )
    .unwrap();

    writeln!(md, "## Cache\n").unwrap();
    writeln!(md, "- Grade: {} ({})", cache.grade, cache.grade_label).unwrap();
    writeln!(
        md,
        "- Weighted hit ratio: {:.1}%",
        cache.trend_weighted_ratio
    )
    .unwrap();
    writeln!(md, "- Overall hit ratio: {:.1}%", cache.hit_ratio).unwrap();
    writeln!(
        md,
        "- Cache read tokens: {}",
        format_tokens_short(cache.total_cache_read)
    )
    .unwrap();
    writeln!(
        md,
        "- Cache write tokens: {}",
        format_tokens_short(cache.total_cache_write)
    )
    .unwrap();
    writeln!(
        md,
        "- Net input tokens: {}",
        format_tokens_short(cache.total_input)
    )
    .unwrap();
    writeln!(md, "- Diagnosis: {}\n", markdown_escape(&cache.diagnosis)).unwrap();

    if let Some(routing) = routing.as_ref() {
        writeln!(md, "## Routing\n").unwrap();
        writeln!(md, "| Family | Sessions | Value share | Avg value/session | Known value |\n|---|---:|---:|---:|---:|").unwrap();
        for (label, stats) in [
            ("Opus", &routing.opus),
            ("Sonnet", &routing.sonnet),
            ("Haiku", &routing.haiku),
            ("Other", &routing.other),
        ] {
            writeln!(
                md,
                "| {} | {} | {:.1}% | {} | {} |",
                label,
                stats.sessions,
                stats.cost_share_pct,
                format_cost(stats.avg_cost_per_session),
                format_cost(stats.cost)
            )
            .unwrap();
        }
        writeln!(
            md,
            "\nRouting diagnosis: {}",
            markdown_escape(&routing.diagnosis)
        )
        .unwrap();
        let routing_savings = if routing.savings_estimate_available {
            format_cost(routing.estimated_savings_if_rerouted)
        } else {
            "Unavailable until cost coverage is exact".to_string()
        };
        writeln!(md, "Estimated reroute savings: {routing_savings}\n").unwrap();

        if !models.is_empty() {
            writeln!(md, "### Model Distribution\n").unwrap();
            writeln!(md, "| Model | Sessions | Cost |\n|---|---:|---:|").unwrap();
            for model in &models {
                let marker = if model_label_is_fast_capable(&model.model) {
                    " ⚡"
                } else {
                    ""
                };
                writeln!(
                    md,
                    "| {}{} | {} | {} |",
                    markdown_escape(&model.model),
                    marker,
                    model.session_count,
                    format_cost(model.total_cost)
                )
                .unwrap();
            }
            writeln!(md).unwrap();
        }

        let speed_split = compute_speed_split(&sessions);
        writeln!(md, "### Speed Split\n").unwrap();
        writeln!(
            md,
            "| Tier | Sessions | Cost | Share |\n|---|---:|---:|---:|"
        )
        .unwrap();
        writeln!(
            md,
            "| Fast-capable ⚡ | {} | {} | {:.1}% |",
            speed_split.fast_sessions,
            format_cost(speed_split.fast_cost),
            speed_split.fast_share_pct()
        )
        .unwrap();
        writeln!(
            md,
            "| Standard | {} | {} | {:.1}% |",
            speed_split.standard_sessions,
            format_cost(speed_split.standard_cost),
            speed_split.standard_share_pct()
        )
        .unwrap();
        writeln!(
            md,
            "\nFast-capable monetary value uses Opus 4.8, Opus 5 and Opus 5.5 pricing (2x priority-speed rate when fast mode is active).\n"
        )
        .unwrap();
    }

    writeln!(md, "## Inflections\n").unwrap();
    if inflections.is_empty() {
        writeln!(
            md,
            "No major cost-per-session inflections detected in this window.\n"
        )
        .unwrap();
    } else {
        writeln!(md, "| Date | Direction | Delta | Sessions | Cost | Baseline | Note |\n|---|---|---:|---:|---:|---:|---|").unwrap();
        for point in inflections.iter().take(10) {
            let direction = if point.direction == "spike" {
                "Spike"
            } else {
                "Drop"
            };
            writeln!(
                md,
                "| {} | {} | {:.2}x | {} | {} | {} | {} |",
                markdown_escape(&point.date),
                direction,
                point.multiplier,
                point.sessions_on_day,
                format_cost(point.cost_on_day),
                format_cost(point.baseline_cost),
                markdown_escape(&point.note)
            )
            .unwrap();
        }
        writeln!(md).unwrap();
    }

    writeln!(md, "## Sessions\n").unwrap();
    if top_sessions.is_empty() {
        writeln!(md, "No sessions found in this window.\n").unwrap();
    } else {
        writeln!(
            md,
            "| # | Project | Model | Tokens | Duration | Cost |\n|---:|---|---|---:|---:|---:|"
        )
        .unwrap();
        for (idx, session) in top_sessions.iter().take(10).enumerate() {
            writeln!(
                md,
                "| {} | {} | {} | {} | {} | {} |",
                idx + 1,
                markdown_escape(&session.project),
                markdown_escape(&session.model),
                format_tokens_short(session.total_tokens),
                format_duration(session.duration_secs),
                known_session_cost(session)
                    .map(format_cost)
                    .unwrap_or_else(|| "Unavailable".to_string())
            )
            .unwrap();
        }
        writeln!(md).unwrap();
    }
    if !projects.is_empty() {
        writeln!(md, "### Projects\n").unwrap();
        writeln!(md, "| Project | Sessions | Tokens | Avg Value | Known Value | Top Model |\n|---|---:|---:|---:|---:|---|").unwrap();
        for project in projects.iter().take(20) {
            writeln!(
                md,
                "| {} | {} | {} | {} | {} | {} |",
                markdown_escape(&project.project),
                project.session_count,
                format_tokens_short(project.total_tokens),
                format_cost(project.avg_session_cost),
                format_cost(project.total_cost),
                markdown_escape(&project.top_model)
            )
            .unwrap();
        }
        writeln!(md).unwrap();
    }

    writeln!(md, "## Tools\n").unwrap();
    writeln!(
        md,
        "- Traced sessions: {} of {}",
        tool_frequency.traced_sessions, tool_frequency.sessions_analyzed
    )
    .unwrap();
    writeln!(
        md,
        "- Total tool calls: {}",
        tool_frequency.total_tool_calls
    )
    .unwrap();
    writeln!(
        md,
        "- Average tools/session: {:.1}",
        tool_frequency.avg_tools_per_session
    )
    .unwrap();
    writeln!(md, "- MCP share: {:.1}%", tool_frequency.mcp_share_pct).unwrap();
    writeln!(
        md,
        "- Diagnosis: {}\n",
        markdown_escape(&tool_frequency.diagnosis)
    )
    .unwrap();
    if tool_frequency.available && !tool_frequency.top_tools.is_empty() {
        writeln!(md, "| Tool | Calls | Share |\n|---|---:|---:|").unwrap();
        for tool in &tool_frequency.top_tools {
            writeln!(
                md,
                "| {} | {} | {:.1}% |",
                markdown_escape(&tool.name),
                tool.count,
                tool.share_pct
            )
            .unwrap();
        }
        writeln!(md).unwrap();
    }

    writeln!(md, "## Telemetry Topology\n").unwrap();
    writeln!(md, "- Provider: {}", provider.display_name()).unwrap();
    writeln!(
        md,
        "- Instruction file: {}",
        provider.instruction_file_name()
    )
    .unwrap();
    writeln!(md, "- Session store: {}", provider.sessions_glob_label()).unwrap();
    writeln!(md, "- Global state: {}", provider.global_state_label()).unwrap();
    writeln!(
        md,
        "- Trace coverage: {} of {} sessions",
        trace_overview.traced_sessions, trace_overview.total_sessions
    )
    .unwrap();
    writeln!(
        md,
        "- Messages: {} user · {} assistant",
        trace_overview.user_messages, trace_overview.assistant_messages
    )
    .unwrap();
    writeln!(
        md,
        "- Tool calls: {} total · {} MCP · {} compact checkpoints\n",
        trace_overview.total_tool_calls,
        trace_overview.mcp_tool_calls,
        trace_overview.total_compactions
    )
    .unwrap();
    if !trace_overview.top_tools.is_empty() {
        writeln!(md, "### Top traced tools\n").unwrap();
        writeln!(md, "| Tool | Calls | Share |\n|---|---:|---:|").unwrap();
        for tool in &trace_overview.top_tools {
            writeln!(
                md,
                "| {} | {} | {:.1}% |",
                markdown_escape(&tool.name),
                tool.calls,
                tool.share_pct
            )
            .unwrap();
        }
        writeln!(md).unwrap();
    }
    writeln!(
        md,
        "```mermaid\n{}\n```\n",
        markdown_code_block(&trace_overview.telemetry_mermaid)
    )
    .unwrap();
    writeln!(
        md,
        "```mermaid\n{}\n```\n",
        markdown_code_block(&trace_overview.cache_mermaid)
    )
    .unwrap();

    writeln!(md, "## Prompts\n").unwrap();
    writeln!(
        md,
        "- Prompts analyzed: {}",
        prompt_complexity.prompts_analyzed
    )
    .unwrap();
    writeln!(
        md,
        "- Average complexity: {:.1}",
        prompt_complexity.avg_complexity_score
    )
    .unwrap();
    writeln!(
        md,
        "- Average specificity: {:.1}",
        prompt_complexity.avg_specificity_score
    )
    .unwrap();
    writeln!(
        md,
        "- High-complexity sessions: {}",
        prompt_complexity.high_complexity_sessions
    )
    .unwrap();
    writeln!(
        md,
        "- Low-specificity sessions: {}",
        prompt_complexity.low_specificity_sessions
    )
    .unwrap();
    writeln!(
        md,
        "- Diagnosis: {}\n",
        markdown_escape(&prompt_complexity.diagnosis)
    )
    .unwrap();
    if prompt_complexity.available && !prompt_complexity.top_sessions.is_empty() {
        writeln!(
            md,
            "| Project | Complexity | Specificity | Label | Preview |\n|---|---:|---:|---|---|"
        )
        .unwrap();
        for session in prompt_complexity.top_sessions.iter().take(8) {
            writeln!(
                md,
                "| {} | {} | {} | {} | {} |",
                markdown_escape(&session.project),
                session.complexity_score,
                session.specificity_score,
                markdown_escape(session.label),
                markdown_escape(&truncate(&session.preview, 96))
            )
            .unwrap();
        }
        writeln!(md).unwrap();
    }

    writeln!(md, "---\n").unwrap();
    writeln!(md, "Generated by Pulse (cc-discord-presence)").unwrap();
    md
}

pub fn generate_html_report(days: Option<i64>, project: Option<&str>) -> String {
    let provider = cc_discord_presence::provider::load_active_provider();
    generate_html_report_for_provider(provider, days, project)
}

pub fn generate_html_report_scoped(
    provider: Option<&str>,
    days: Option<i64>,
    project: Option<&str>,
) -> String {
    let active = cc_discord_presence::provider::load_active_provider();
    let scope = provider
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| active.as_str());
    if let Some(provider) = cc_discord_presence::provider::Provider::parse(scope) {
        return generate_html_report_for_provider(provider, days, project);
    }

    let d = days.unwrap_or(30);
    let data = load_report_data(d, project, scope);
    let coverage = db::summarize_cost_provenance(data.sessions.iter().map(|session| {
        (
            session.cost_basis,
            session.cost_source.as_str(),
            session.known_cost,
        )
    }));
    let label = match scope {
        "all" => "All providers",
        "openai" => "OpenAI API",
        "anthropic" => "Anthropic API",
        _ => "Selected provider",
    };
    let project_label = project.unwrap_or("All projects");
    format!(
        r#"<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Pulse Analytics Report</title><style>body{{margin:0;background:#0b0d10;color:#f5f7fa;font:15px/1.55 Inter,system-ui,sans-serif}}main{{max-width:920px;margin:0 auto;padding:56px 28px}}.kicker{{color:#8ea0b8;text-transform:uppercase;letter-spacing:.12em;font-size:12px}}h1{{font-size:36px;margin:.25rem 0 2rem}}.grid{{display:grid;grid-template-columns:repeat(auto-fit,minmax(180px,1fr));gap:14px}}.card{{border:1px solid #29313c;border-radius:12px;background:#12161c;padding:18px}}.label{{color:#8ea0b8;font-size:12px}}.value{{font-size:24px;font-weight:700;margin-top:6px}}.note{{margin-top:24px;color:#aab6c5}}code{{color:#d6e2f2}}</style></head><body><main><div class="kicker">Pulse · {label}</div><h1>Analytics Report</h1><div class="grid"><div class="card"><div class="label">Period</div><div class="value">{days} days</div></div><div class="card"><div class="label">Project</div><div class="value">{project}</div></div><div class="card"><div class="label">Sessions</div><div class="value">{sessions}</div></div><div class="card"><div class="label">Processed token volume</div><div class="value">{tokens}</div></div><div class="card"><div class="label">Known monetary value</div><div class="value">{cost}</div><div class="label">{coverage}</div></div></div><p class="note">Provider-specific recommendations require selecting one subscription provider. Unknown monetary value remains unavailable rather than being rendered as zero.</p></main></body></html>"#,
        label = html_escape(label),
        days = d,
        project = html_escape(project_label),
        sessions = data.sessions.len(),
        tokens = html_escape(&format_tokens_short(data.summary.total_tokens)),
        cost = html_escape(&format_cost(data.summary.total_cost)),
        coverage = html_escape(&format_cost_coverage(&coverage)),
    )
}

pub fn generate_html_report_for_provider(
    provider: cc_discord_presence::provider::Provider,
    days: Option<i64>,
    project: Option<&str>,
) -> String {
    use super::analyzers::{
        cache_health, inflection, model_routing, prompt_complexity, session_trace, tool_frequency,
        trace_overview,
    };
    use std::collections::BTreeMap;
    use std::fmt::Write as _;

    let d = days.unwrap_or(30);
    let data = load_report_data(d, project, provider.as_str());
    let sessions = data.sessions;
    let daily = data.daily;
    let summary = data.summary;
    let projects = data.projects;
    let forecast = data.forecast;
    let hourly = data.hourly;
    let models = data.models;

    let total_sessions = sessions.len();
    let cost_coverage = db::summarize_cost_provenance(sessions.iter().map(|session| {
        (
            session.cost_basis,
            session.cost_source.as_str(),
            session.known_cost,
        )
    }));
    let total_cost: f64 = sessions.iter().filter_map(known_session_cost).sum();
    let total_tokens: i64 = sessions.iter().map(|s| s.total_tokens).sum();
    let total_input: i64 = sessions
        .iter()
        .map(|s| (s.input_tokens - s.cache_write_tokens - s.cache_read_tokens).max(0))
        .sum();
    let total_output: i64 = sessions.iter().map(|s| s.output_tokens).sum();
    let total_cache_w: i64 = sessions.iter().map(|s| s.cache_write_tokens).sum();
    let total_cache_r: i64 = sessions.iter().map(|s| s.cache_read_tokens).sum();

    let capabilities = provider.capabilities();
    let cache = cache_health::analyze_for_provider(provider, &sessions);
    let routing = capabilities
        .model_routing
        .then(|| model_routing::analyze(&sessions));
    let inflections = inflection::detect_for_provider(provider, &sessions);
    let traces = session_trace::load_session_traces(&sessions);
    let tool_frequency = tool_frequency::analyze(&sessions, &traces);
    let prompt_complexity = prompt_complexity::analyze(&sessions, &traces);
    let trace_overview =
        trace_overview::build(provider, &sessions, &traces, cache.trend_weighted_ratio);

    let grade_color = match cache.grade {
        'A' | 'B' => "#22c55e",
        'C' => "#fbbf24",
        _ => "#ef4444",
    };
    let project_table_html = build_project_table(&projects);
    let top_sessions_html = build_top_sessions(&sessions);
    let hourly_heatmap_html = build_hourly_heatmap(&hourly);
    let recommendations = build_recommendations(provider, &sessions, &traces);
    let mut by_date: BTreeMap<String, f64> = BTreeMap::new();
    for day in &daily {
        *by_date.entry(day.date.clone()).or_default() += day.total_cost;
    }
    let mut routing_section_html = String::new();
    if let Some(routing) = routing.as_ref() {
        let speed_split = compute_speed_split(&sessions);
        let speed_split_html = build_speed_split_html(&speed_split);
        let model_table_html = build_model_table(&models, total_sessions);
        let mut routing_rows_html = String::new();
        for (label, stats) in [
            ("Opus", &routing.opus),
            ("Sonnet", &routing.sonnet),
            ("Haiku", &routing.haiku),
            ("Other", &routing.other),
        ] {
            let width = stats.cost_share_pct.clamp(0.0, 100.0);
            write!(routing_rows_html, r##"<div class="routing-row"><div class="routing-label-row"><span class="routing-name">{label}</span><span class="routing-share">{share:.1}%</span></div><div class="routing-meta">{sessions} sessions · {avg_cost} avg · {cost}</div><div class="routing-track"><div class="routing-fill" style="width:{width:.1}%"></div></div></div>"##, label = html_escape(label), share = stats.cost_share_pct, sessions = stats.sessions, avg_cost = format_cost(stats.avg_cost_per_session), cost = html_escape(&format_cost(stats.cost)), width = width).unwrap();
        }
        let routing_savings = if routing.savings_estimate_available {
            format_cost(routing.estimated_savings_if_rerouted)
        } else {
            "Unavailable".to_string()
        };
        write!(routing_section_html, r##"<section id="routing" class="section"><div class="section-header"><div><h2>Routing</h2><p>Family-level monetary-value split. Bars stay monochrome. Diagnosis stays textual for export parity.</p></div></div><div class="section-grid"><div class="card"><h2>Family Value</h2>{routing_rows}<div class="metric-strip"><div class="metric"><div class="label">Sessions</div><div class="value">{routing_sessions}</div></div><div class="metric"><div class="label">Known Value</div><div class="value">{routing_cost}</div></div><div class="metric"><div class="label">Potential Savings</div><div class="value">{routing_savings}</div></div></div><p style="margin-top:18px;">{routing_diagnosis}</p></div>{model_table_html}</div><div class="section-grid" style="margin-top:18px;">{speed_split_html}</div></section>"##, routing_rows = routing_rows_html, routing_sessions = routing.total_sessions, routing_cost = html_escape(&format_cost(routing.total_cost)), routing_savings = html_escape(&routing_savings), routing_diagnosis = html_escape(&routing.diagnosis), model_table_html = model_table_html, speed_split_html = speed_split_html).unwrap();
    }

    let mut inflections_html = String::new();
    if inflections.is_empty() {
        inflections_html.push_str(r#"<div class="empty-state">No major cost-per-session inflections detected in this window.</div>"#);
    } else {
        inflections_html.push_str(r#"<div class="inflection-grid">"#);
        for point in inflections.iter().take(6) {
            let direction_class = if point.direction == "spike" {
                "inflection-up"
            } else {
                "inflection-down"
            };
            let direction_label = if point.direction == "spike" {
                "Up"
            } else {
                "Down"
            };
            write!(inflections_html, r##"<article class="inflection-card {direction_class}"><div class="inflection-head"><span class="inflection-date">{date}</span><span class="inflection-direction">{direction_label}</span></div><div class="inflection-metric">{multiplier:.2}×</div><div class="inflection-support">{sessions} sessions · {cost}</div><p>{note}</p></article>"##, direction_class = direction_class, date = html_escape(&point.date), direction_label = direction_label, multiplier = point.multiplier, sessions = point.sessions_on_day, cost = html_escape(&format_cost(point.cost_on_day)), note = html_escape(&point.note)).unwrap();
        }
        inflections_html.push_str("</div>");
    }
    let mut tools_table_html = String::new();
    if tool_frequency.available && !tool_frequency.top_tools.is_empty() {
        tools_table_html.push_str(r#"<div class="card"><h2>Top Tools</h2><table><tr><th>Tool</th><th>Calls</th><th>Share</th></tr>"#);
        for tool in &tool_frequency.top_tools {
            write!(
                tools_table_html,
                r#"<tr><td>{}</td><td class="num">{}</td><td class="num">{:.1}%</td></tr>"#,
                html_escape(&tool.name),
                tool.count,
                tool.share_pct
            )
            .unwrap();
        }
        tools_table_html.push_str("</table></div>");
    } else {
        tools_table_html.push_str(r#"<div class="card"><h2>Top Tools</h2><div class="empty-state">No JSONL tool traces available yet.</div></div>"#);
    }

    let truncate = |value: &str, max_chars: usize| {
        let mut out = String::new();
        for (idx, ch) in value.chars().enumerate() {
            if idx >= max_chars {
                out.push('…');
                break;
            }
            out.push(ch);
        }
        out
    };
    let mut prompt_table_html = String::new();
    if prompt_complexity.available && !prompt_complexity.top_sessions.is_empty() {
        prompt_table_html.push_str(r#"<div class="card"><h2>Most Complex Prompts</h2><table><tr><th>Project</th><th>Complexity</th><th>Specificity</th><th>Label</th><th>Preview</th></tr>"#);
        for session in prompt_complexity.top_sessions.iter().take(8) {
            prompt_table_html.push_str(&render_prompt_row(
                &session.project,
                session.complexity_score,
                session.specificity_score,
                session.label,
                &truncate(&session.preview, 140),
            ));
        }
        prompt_table_html.push_str("</table></div>");
    } else {
        prompt_table_html.push_str(r#"<div class="card"><h2>Most Complex Prompts</h2><div class="empty-state">No prompt previews available yet.</div></div>"#);
    }

    let generated_at = chrono::Utc::now().format("%Y-%m-%d %H:%M UTC").to_string();
    let period_label = if let Some(p) = project {
        format!("{p} — Last {d} days")
    } else {
        format!("All Projects — Last {d} days")
    };
    let daily_chart_html = build_daily_cost_svg(&by_date);
    let token_chart_html =
        build_token_composition_svg(total_input, total_output, total_cache_w, total_cache_r);

    let mut html = String::new();
    html.push_str(&crate::report_template::report_head());
    write!(html, r##"<header class="hero"><div class="hero-top"><div><div class="kicker">Pulse · {provider_name} Analytics</div><h1>Analytics Report</h1><div class="hero-meta">{period_label}</div></div><div class="generated-at">Generated {generated_at}</div></div><div class="hero-divider"></div><div class="summary-grid"><div class="summary-card"><div class="summary-label">Known Monetary Value</div><div class="summary-value">{total_cost}</div><div class="summary-meta">{cost_coverage}</div></div><div class="summary-card"><div class="summary-label">Provider-billed MTD</div><div class="summary-value">{billed_mtd}</div><div class="summary-meta">Only provider-reported billing</div></div><div class="summary-card"><div class="summary-label">API-equivalent MTD</div><div class="summary-value">{api_equivalent_mtd}</div><div class="summary-meta">Published rates, never billed spend</div></div><div class="summary-card"><div class="summary-label">Sessions</div><div class="summary-value">{total_sessions}</div><div class="summary-meta">Tracked in current window</div></div><div class="summary-card"><div class="summary-label">Tokens</div><div class="summary-value">{total_tokens}</div><div class="summary-meta">Processed input + output + cache volume</div></div><div class="summary-card"><div class="summary-label">Cache Grade</div><div class="summary-value" style="color:{grade_color}">{cache_grade}</div><div class="summary-meta">{cache_ratio:.1}% weighted hit ratio</div></div></div></header>"##,
        provider_name = html_escape(provider.display_name()),
        period_label = html_escape(&period_label),
        generated_at = html_escape(&generated_at),
        total_cost = html_escape(&format_cost(total_cost)),
        cost_coverage = html_escape(&format_cost_coverage(&cost_coverage)),
        billed_mtd = html_escape(&forecast.billed_spend_usd.map(format_cost).unwrap_or_else(|| "Unavailable".to_string())),
        api_equivalent_mtd = html_escape(&forecast.api_equivalent_usd.map(format_cost).unwrap_or_else(|| "Unavailable".to_string())),
        total_sessions = total_sessions,
        total_tokens = html_escape(&format_tokens_short(total_tokens)),
        grade_color = grade_color,
        cache_grade = cache.grade,
        cache_ratio = cache.trend_weighted_ratio,
    ).unwrap();
    let topology_tools_html = if trace_overview.top_tools.is_empty() {
        r#"<div class="empty-state">No traced tool mix yet.</div>"#.to_string()
    } else {
        let mut html = String::from(
            r#"<div class="card"><h2>Top Traced Tools</h2><table><tr><th>Tool</th><th>Calls</th><th>Share</th></tr>"#,
        );
        for tool in &trace_overview.top_tools {
            write!(
                html,
                r#"<tr><td>{}</td><td class="num">{}</td><td class="num">{:.1}%</td></tr>"#,
                html_escape(&tool.name),
                tool.calls,
                tool.share_pct
            )
            .unwrap();
        }
        html.push_str("</table></div>");
        html
    };

    if capabilities.model_routing {
        html.push_str(r##"<nav class="anchor-nav screen-only"><a href="#cache">Cache</a><a href="#routing">Routing</a><a href="#inflections">Inflections</a><a href="#sessions">Sessions</a><a href="#tools">Tools</a><a href="#topology">Topology</a><a href="#prompts">Prompts</a><a href="#recommendations">Fixes</a></nav>"##);
    } else {
        html.push_str(r##"<nav class="anchor-nav screen-only"><a href="#cache">Cache</a><a href="#inflections">Inflections</a><a href="#sessions">Sessions</a><a href="#tools">Tools</a><a href="#topology">Topology</a><a href="#prompts">Prompts</a><a href="#recommendations">Fixes</a></nav>"##);
    }
    write!(html, r##"<section id="cache" class="section"><div class="section-header"><div><h2>Cache</h2><p>Weighted cache health drives grade color. Token mix stays visible for fast copy-paste review.</p></div></div><div class="section-grid"><div class="card"><div class="cache-grade"><div class="cache-letter" style="color:{grade_color}">{cache_grade}</div><div class="cache-copy"><h3>Cache Health</h3><div class="ratio">{cache_ratio:.1}%</div><p>{cache_diagnosis}</p></div></div><div class="metric-strip"><div class="metric"><div class="label">Overall Hit Ratio</div><div class="value">{overall_ratio:.1}%</div></div><div class="metric"><div class="label">Cache Read</div><div class="value">{cache_read}</div></div><div class="metric"><div class="label">Cache Write</div><div class="value">{cache_write}</div></div></div></div><div class="card chart-card"><h2>Token Composition</h2>{token_chart_html}<ul class="token-legend"><li><span class="dot" style="background:#f5f5f5"></span>Pure Input<b>{pure_input_short}</b></li><li><span class="dot" style="background:#7cb9e8"></span>Output<b>{output_short}</b></li><li><span class="dot" style="background:#fbbf24"></span>Cache Write<b>{cache_w_short}</b></li><li><span class="dot" style="background:#22c55e"></span>Cache Read<b>{cache_r_short}</b></li></ul></div></div></section>"##, grade_color = grade_color, cache_grade = cache.grade, cache_ratio = cache.trend_weighted_ratio, cache_diagnosis = html_escape(&cache.diagnosis), overall_ratio = cache.hit_ratio, cache_read = html_escape(&format_tokens_short(cache.total_cache_read)), cache_write = html_escape(&format_tokens_short(cache.total_cache_write)), token_chart_html = token_chart_html, pure_input_short = html_escape(&format_tokens_short(total_input)), output_short = html_escape(&format_tokens_short(total_output)), cache_w_short = html_escape(&format_tokens_short(total_cache_w)), cache_r_short = html_escape(&format_tokens_short(total_cache_r))).unwrap();
    html.push_str(&routing_section_html);
    write!(html, r##"<section id="inflections" class="section"><div class="section-header"><div><h2>Inflections</h2><p>Spike cards use red rail. Efficiency drops use green rail. Sorted by absolute signal strength.</p></div></div>{inflections_html}</section>"##, inflections_html = inflections_html).unwrap();
    write!(html, r##"<section id="sessions" class="section"><div class="section-header"><div><h2>Sessions</h2><p>Daily cost trend, hourly activity, top sessions, project mix. Same data sources. Cleaner export.</p></div></div><div class="section-grid"><div class="card chart-card"><h2>Daily Cost Trend</h2>{daily_chart_html}</div><div class="card"><h2>Hourly Activity</h2>{hourly_heatmap_html}</div></div><div class="section-grid" style="margin-top:18px;">{top_sessions_html}{project_table_html}</div></section>"##, daily_chart_html = daily_chart_html, hourly_heatmap_html = hourly_heatmap_html, top_sessions_html = top_sessions_html, project_table_html = project_table_html).unwrap();
    write!(html, r##"<section id="tools" class="section"><div class="section-header"><div><h2>Tools</h2><p>Tool intensity, MCP share, compact gaps, top tool mix.</p></div></div><div class="info-grid"><div class="info-card"><div class="info-label">Traced Sessions</div><div class="info-value">{traced_sessions}</div><p>{sessions_analyzed} sessions analyzed</p></div><div class="info-card"><div class="info-label">Total Tool Calls</div><div class="info-value">{tool_calls}</div><p>{tools_per_session:.1} avg per session</p></div><div class="info-card"><div class="info-label">Calls / Hour</div><div class="info-value">{calls_per_hour:.1}</div><p>{mcp_share:.1}% MCP share</p></div><div class="info-card"><div class="info-label">Compact Gaps</div><div class="info-value">{compact_gaps}</div><p>{tool_diagnosis}</p></div></div><div style="margin-top:18px;">{tools_table_html}</div></section>"##, traced_sessions = tool_frequency.traced_sessions, sessions_analyzed = tool_frequency.sessions_analyzed, tool_calls = tool_frequency.total_tool_calls, tools_per_session = tool_frequency.avg_tools_per_session, calls_per_hour = tool_frequency.avg_tool_calls_per_hour, mcp_share = tool_frequency.mcp_share_pct, compact_gaps = tool_frequency.compact_gap_sessions, tool_diagnosis = html_escape(&tool_frequency.diagnosis), tools_table_html = tools_table_html).unwrap();
    write!(html, r##"<section id="topology" class="section"><div class="section-header"><div><h2>Telemetry Topology</h2><p>Provider-aware wiring from instruction files to cache reuse, session telemetry, analytics storage, exports, and rich presence.</p></div></div><div class="info-grid"><div class="info-card"><div class="info-label">Provider</div><div class="info-value">{provider_display}</div><p>{instruction_file} · {fix_label}</p></div><div class="info-card"><div class="info-label">Session Store</div><div class="info-value">{traced_sessions}/{total_sessions}</div><p>{session_store}</p></div><div class="info-card"><div class="info-label">Message Flow</div><div class="info-value">{user_messages}/{assistant_messages}</div><p>User / assistant traced messages</p></div><div class="info-card"><div class="info-label">Cache + Tools</div><div class="info-value">{tool_calls}</div><p>{cache_ratio:.1}% cache hit · {mcp_calls} MCP · {compactions} compactions</p></div></div><div class="section-grid" style="margin-top:18px;"><div class="card"><h2>Telemetry Flow</h2><pre class="diagram-code">{telemetry_mermaid}</pre></div><div class="card"><h2>Cache &amp; Tool Flow</h2><pre class="diagram-code">{cache_mermaid}</pre></div></div><div style="margin-top:18px;">{topology_tools_html}</div></section>"##,
        provider_display = html_escape(&trace_overview.provider_display),
        instruction_file = html_escape(&trace_overview.instruction_file),
        fix_label = html_escape(&trace_overview.fix_button_label),
        traced_sessions = trace_overview.traced_sessions,
        total_sessions = trace_overview.total_sessions,
        session_store = html_escape(&trace_overview.session_store),
        user_messages = trace_overview.user_messages,
        assistant_messages = trace_overview.assistant_messages,
        tool_calls = trace_overview.total_tool_calls,
        cache_ratio = trace_overview.cache_hit_ratio,
        mcp_calls = trace_overview.mcp_tool_calls,
        compactions = trace_overview.total_compactions,
        telemetry_mermaid = html_escape(&trace_overview.telemetry_mermaid),
        cache_mermaid = html_escape(&trace_overview.cache_mermaid),
        topology_tools_html = topology_tools_html,
    ).unwrap();
    write!(html, r##"<section id="prompts" class="section"><div class="section-header"><div><h2>Prompts</h2><p>Prompt complexity stays copyable. Preview column trims long prompts without hiding signal.</p></div></div><div class="info-grid"><div class="info-card"><div class="info-label">Prompts Analyzed</div><div class="info-value">{prompts_analyzed}</div><p>{prompt_sessions} sessions scanned</p></div><div class="info-card"><div class="info-label">Avg Complexity</div><div class="info-value">{avg_complexity:.1}</div><p>{high_complexity} high-complexity sessions</p></div><div class="info-card"><div class="info-label">Avg Specificity</div><div class="info-value">{avg_specificity:.1}</div><p>{low_specificity} low-specificity sessions</p></div><div class="info-card"><div class="info-label">Diagnosis</div><div class="info-value">{prompt_label}</div><p>{prompt_diagnosis}</p></div></div><div style="margin-top:18px;">{prompt_table_html}</div></section>"##, prompts_analyzed = prompt_complexity.prompts_analyzed, prompt_sessions = prompt_complexity.sessions_analyzed, avg_complexity = prompt_complexity.avg_complexity_score, high_complexity = prompt_complexity.high_complexity_sessions, avg_specificity = prompt_complexity.avg_specificity_score, low_specificity = prompt_complexity.low_specificity_sessions, prompt_label = if prompt_complexity.available { "Live" } else { "Pending" }, prompt_diagnosis = html_escape(&prompt_complexity.diagnosis), prompt_table_html = prompt_table_html).unwrap();
    write!(
        html,
        r##"<section id="recommendations" class="section"><div class="section-header"><div><h2>Recommendations</h2><p>Rule-based fixes from the Pulse recommendations engine. Each card ships with a "{fix_label}" prompt — paste into {provider_name} to remediate.</p></div></div><ul class="rec-list">{recommendations}</ul></section><footer class="footer"><div class="footer-brand">Pulse · {provider_name} Analytics</div><div class="footer-meta">All-time <b>{all_time_sessions}</b> sessions · <b>{all_time_cost}</b> · {all_time_days} days tracked</div><div class="footer-links">Offline report · v{version}</div></footer></div>"##,
        provider_name = html_escape(provider.display_name()),
        fix_label = html_escape(provider.fix_action_label()),
        recommendations = recommendations,
        version = env!("CARGO_PKG_VERSION"),
        all_time_sessions = summary.total_sessions,
        all_time_cost = html_escape(&format_cost(summary.total_cost)),
        all_time_days = summary.days_tracked
    )
    .unwrap();
    html.push_str(crate::report_template::REPORT_TAIL);
    html
}

struct SpeedSplit {
    fast_sessions: usize,
    standard_sessions: usize,
    fast_cost: f64,
    standard_cost: f64,
}

impl SpeedSplit {
    fn total_cost(&self) -> f64 {
        self.fast_cost + self.standard_cost
    }
    fn fast_share_pct(&self) -> f64 {
        let total = self.total_cost();
        if total > 0.0 {
            (self.fast_cost / total) * 100.0
        } else {
            0.0
        }
    }
    fn standard_share_pct(&self) -> f64 {
        100.0 - self.fast_share_pct()
    }
}

fn compute_speed_split(sessions: &[db::HistoricalSession]) -> SpeedSplit {
    let mut split = SpeedSplit {
        fast_sessions: 0,
        standard_sessions: 0,
        fast_cost: 0.0,
        standard_cost: 0.0,
    };
    for s in sessions {
        if cc_discord_presence::cost::is_fast_capable(&s.model_id) {
            split.fast_sessions += 1;
            split.fast_cost += known_session_cost(s).unwrap_or(0.0);
        } else {
            split.standard_sessions += 1;
            split.standard_cost += known_session_cost(s).unwrap_or(0.0);
        }
    }
    split
}

fn build_speed_split_html(split: &SpeedSplit) -> String {
    format!(
        r##"<div class="card"><h2>Speed Split</h2><div class="speed-split"><div class="speed-cell is-fast"><div class="speed-head"><span class="speed-bolt">⚡</span><span class="speed-name">Fast-capable</span></div><div class="speed-value">{fast_cost}</div><div class="speed-meta">{fast_sessions} sessions · {fast_share:.1}%</div><div class="speed-bar"><div class="speed-fill" style="width:{fast_share:.1}%"></div></div></div><div class="speed-cell"><div class="speed-head"><span class="speed-name">Standard</span></div><div class="speed-value">{standard_cost}</div><div class="speed-meta">{standard_sessions} sessions · {standard_share:.1}%</div><div class="speed-bar"><div class="speed-fill" style="width:{standard_share:.1}%"></div></div></div></div><p style="margin-top:14px;color:var(--text-secondary);font-size:12px;">Fast-capable monetary value uses Opus 4.8, Opus 5 and Opus 5.5 pricing at the 2× priority-speed rate when fast mode is active. Standard covers every other model.</p></div>"##,
        fast_cost = html_escape(&format_cost(split.fast_cost)),
        fast_sessions = split.fast_sessions,
        fast_share = split.fast_share_pct(),
        standard_cost = html_escape(&format_cost(split.standard_cost)),
        standard_sessions = split.standard_sessions,
        standard_share = split.standard_share_pct(),
    )
}

fn format_tokens_short(t: i64) -> String {
    if t >= 1_000_000 {
        format!("{:.1}M", t as f64 / 1_000_000.0)
    } else if t >= 1_000 {
        format!("{:.1}K", t as f64 / 1_000.0)
    } else {
        t.to_string()
    }
}

fn format_cost(c: f64) -> String {
    if c >= 1.0 {
        format!("${c:.2}")
    } else {
        format!("${c:.4}")
    }
}

fn format_cost_coverage(coverage: &db::CostCoverage) -> String {
    let basis = match coverage.cost_basis {
        db::CostBasis::Exact => "Exact",
        db::CostBasis::Partial => "Partial",
        db::CostBasis::Estimated => "Estimated (API-equivalent)",
        db::CostBasis::Unavailable => "Unavailable",
    };
    let sources = if coverage.cost_sources.is_empty() {
        String::new()
    } else {
        format!(" · {}", coverage.cost_sources.join(", "))
    };
    format!(
        "{basis} · {}/{} sessions priced{sources}",
        coverage.priced_sessions, coverage.sessions
    )
}

fn format_duration(secs: i64) -> String {
    if secs <= 0 {
        return "—".to_string();
    }
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    if h > 0 {
        format!("{h}h {m}m")
    } else {
        format!("{m}m")
    }
}

fn build_top_sessions(sessions: &[db::HistoricalSession]) -> String {
    let mut sorted: Vec<_> = sessions
        .iter()
        .filter(|session| known_session_cost(session).is_some())
        .collect();
    sorted.sort_by(|a, b| {
        known_session_cost(b)
            .unwrap_or_default()
            .partial_cmp(&known_session_cost(a).unwrap_or_default())
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let top = &sorted[..sorted.len().min(25)];
    if top.is_empty() {
        return String::new();
    }
    let mut html = String::from(
        r#"<div class="card"><h2>Most Costly Sessions</h2><table><tr><th>#</th><th>Project</th><th>Model</th><th>Tokens</th><th>Duration</th><th style="text-align:right">Cost</th></tr>"#,
    );
    for (i, s) in top.iter().enumerate() {
        html.push_str(&render_session_row(
            i + 1,
            &s.project,
            &s.model,
            s.total_tokens,
            s.duration_secs,
            known_session_cost(s).unwrap_or_default(),
        ));
    }
    html.push_str("</table></div>");
    html
}

fn build_project_table(projects: &[db::ProjectStat]) -> String {
    if projects.is_empty() {
        return String::new();
    }
    let mut html = String::from(
        r#"<div class="card"><h2>Project Breakdown</h2><table><tr><th>Project</th><th>Sessions</th><th>Tokens</th><th>Avg Value</th><th style="text-align:right">Known Value</th></tr>"#,
    );
    for p in projects {
        html.push_str(&format!(
            "<tr><td>{}</td><td class=\"num\">{}</td><td class=\"num\">{}</td><td class=\"num\">{}</td><td class=\"cost\">{}</td></tr>",
            html_escape(&p.project),
            p.session_count,
            html_escape(&format_tokens_short(p.total_tokens)),
            html_escape(&format_cost(p.avg_session_cost)),
            html_escape(&format_cost(p.total_cost))
        ));
    }
    html.push_str("</table></div>");
    html
}

fn model_label_is_fast_capable(model_label: &str) -> bool {
    let normalized = model_label
        .to_ascii_lowercase()
        .replace([' ', '.'], "-")
        .replace("--", "-");
    cc_discord_presence::cost::is_fast_capable(&normalized)
}

fn build_model_table(models: &[db::ModelStat], total: usize) -> String {
    if models.is_empty() {
        return String::new();
    }
    let mut html = String::from(
        r#"<div class="card"><h2>Model Routing Analysis</h2><table><tr><th>Model</th><th>Sessions</th><th>Share</th><th style="text-align:right">Cost</th></tr>"#,
    );
    for model in models {
        let pct = if total > 0 {
            (model.session_count as f64 / total as f64) * 100.0
        } else {
            0.0
        };
        let fast_tag = if model_label_is_fast_capable(&model.model) {
            r#"<span class="fast-tag">⚡ Fast</span>"#
        } else {
            ""
        };
        html.push_str(&format!(
            "<tr><td>{name}{fast_tag}</td><td class=\"num\">{count}</td><td class=\"num\">{pct:.0}%</td><td class=\"cost\">{cost}</td></tr>",
            name = html_escape(&model.model),
            fast_tag = fast_tag,
            count = model.session_count,
            cost = format_cost(model.total_cost)
        ));
    }
    html.push_str("</table></div>");
    html
}

fn build_daily_cost_svg(by_date: &std::collections::BTreeMap<String, f64>) -> String {
    if by_date.is_empty() {
        return r#"<div class="empty-state">No daily cost data available.</div>"#.to_string();
    }
    let width = 760.0;
    let height = 220.0;
    let padding_x = 18.0;
    let padding_y = 20.0;
    let values: Vec<f64> = by_date.values().copied().collect();
    let max = values.iter().copied().fold(0.0_f64, f64::max).max(1.0);
    let step_x = if values.len() > 1 {
        (width - padding_x * 2.0) / (values.len() - 1) as f64
    } else {
        0.0
    };
    let baseline = height - padding_y;
    let coords: Vec<(f64, f64)> = values
        .iter()
        .enumerate()
        .map(|(idx, value)| {
            let x = padding_x + step_x * idx as f64;
            let y = baseline - ((value / max) * (height - padding_y * 2.0));
            (x, y)
        })
        .collect();
    let end_x = padding_x + step_x * (values.len().saturating_sub(1)) as f64;

    let curve = curve_segments(&coords);
    let line_path = match coords.first() {
        Some((x, y)) => format!("M {x:.2} {y:.2}{curve}"),
        None => String::new(),
    };
    let area_path = match coords.first() {
        Some((x, y)) => {
            format!("M {x:.2} {baseline:.2} L {x:.2} {y:.2}{curve} L {end_x:.2} {baseline:.2} Z")
        }
        None => String::new(),
    };

    let peak_idx =
        values.iter().enumerate().fold(
            0usize,
            |best, (idx, value)| {
                if *value > values[best] { idx } else { best }
            },
        );
    let peak_label = coords
        .get(peak_idx)
        .map(|(x, y)| {
            let anchor = if peak_idx == 0 {
                "start"
            } else if peak_idx + 1 == coords.len() {
                "end"
            } else {
                "middle"
            };
            format!(
                r##"<circle cx="{x:.2}" cy="{y:.2}" r="3" fill="#f5f5f5"/><text x="{x:.2}" y="{ty:.2}" text-anchor="{anchor}" fill="#f5f5f5" font-size="11" font-weight="700" font-family="-apple-system, 'Segoe UI', sans-serif">{label}</text>"##,
                x = x,
                y = y,
                ty = (y - 8.0).max(12.0),
                anchor = anchor,
                label = html_escape(&format_cost(values[peak_idx])),
            )
        })
        .unwrap_or_default();

    let labels = by_date
        .iter()
        .enumerate()
        .filter(|(idx, _)| values.len() <= 6 || *idx == 0 || *idx == values.len() - 1 || idx % 2 == 0)
        .map(|(idx, (date, _))| {
            let x = padding_x + step_x * idx as f64;
            format!(
                r##"<text x="{x:.2}" y="{y}" text-anchor="middle" fill="#6b6b6b" font-size="10">{label}</text>"##,
                y = height - 4.0,
                label = html_escape(date),
            )
        })
        .collect::<Vec<_>>()
        .join("");

    format!(
        r##"<svg viewBox="0 0 {width} {height}" class="report-svg" role="img" aria-label="Daily cost trend">
<rect x="0" y="0" width="{width}" height="{height}" fill="transparent"/>
<line x1="{padding_x}" y1="{baseline}" x2="{end_x}" y2="{baseline}" stroke="#1f1f1f" stroke-width="1"/>
<path d="{area_path}" fill="rgba(245,245,245,0.08)"/>
<path d="{line_path}" fill="none" stroke="#f5f5f5" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"/>
{peak_label}
{labels}
</svg>"##,
        baseline = baseline,
        end_x = end_x,
        area_path = area_path,
        line_path = line_path,
        peak_label = peak_label,
        labels = labels,
    )
}

fn curve_segments(points: &[(f64, f64)]) -> String {
    if points.len() < 2 {
        return String::new();
    }
    let mut path = String::new();
    for i in 0..points.len() - 1 {
        let p0 = points[i.saturating_sub(1)];
        let p1 = points[i];
        let p2 = points[i + 1];
        let p3 = points[(i + 2).min(points.len() - 1)];
        let c1x = p1.0 + (p2.0 - p0.0) / 6.0;
        let c1y = p1.1 + (p2.1 - p0.1) / 6.0;
        let c2x = p2.0 - (p3.0 - p1.0) / 6.0;
        let c2y = p2.1 - (p3.1 - p1.1) / 6.0;
        path.push_str(&format!(
            " C {c1x:.2} {c1y:.2}, {c2x:.2} {c2y:.2}, {x:.2} {y:.2}",
            c1x = c1x,
            c1y = c1y,
            c2x = c2x,
            c2y = c2y,
            x = p2.0,
            y = p2.1,
        ));
    }
    path
}

fn build_token_composition_svg(input: i64, output: i64, cache_w: i64, cache_r: i64) -> String {
    let segments = [
        ("#f5f5f5", "Input", input.max(0) as f64),
        ("#7cb9e8", "Output", output.max(0) as f64),
        ("#fbbf24", "Cache W", cache_w.max(0) as f64),
        ("#22c55e", "Cache R", cache_r.max(0) as f64),
    ];
    let total = segments
        .iter()
        .map(|(_, _, value)| *value)
        .sum::<f64>()
        .max(1.0);
    let width = 760.0;
    let bar_x = 22.0;
    let bar_y = 74.0;
    let bar_w = width - 44.0;
    let bar_h = 18.0;
    let label_min_w = 48.0;
    let mut cursor = bar_x;
    let mut bars = String::new();
    for (color, label, value) in segments {
        let segment_w = (value / total) * bar_w;
        if segment_w > 0.0 {
            bars.push_str(&format!(
                r##"<rect x="{x:.2}" y="{bar_y}" width="{w:.2}" height="{bar_h}" rx="9" fill="{color}"/>"##,
                x = cursor,
                w = segment_w.max(2.0),
            ));
            if segment_w >= label_min_w {
                bars.push_str(&format!(
                    r##"<text x="{cx:.2}" y="{ty:.2}" text-anchor="middle" fill="#0a0a0a" font-size="10" font-weight="700" font-family="JetBrains Mono, monospace">{label} {pct:.0}%</text>"##,
                    cx = cursor + segment_w / 2.0,
                    ty = bar_y + bar_h / 2.0 + 3.5,
                    label = html_escape(label),
                    pct = (value / total) * 100.0,
                ));
            }
        }
        cursor += segment_w;
    }
    format!(
        r##"<svg viewBox="0 0 {width} 140" class="report-svg" role="img" aria-label="Token composition">
<rect x="{bar_x}" y="{bar_y}" width="{bar_w}" height="{bar_h}" rx="9" fill="#121212" stroke="#1f1f1f" stroke-width="1"/>
{bars}
<text x="{bar_x}" y="40" fill="#fafafa" font-size="26" font-weight="700">{total_label}</text>
<text x="{bar_x}" y="58" fill="#6b6b6b" font-size="11" font-family="JetBrains Mono, monospace">total token mix</text>
</svg>"##,
        bars = bars,
        total_label = html_escape(&format_tokens_short(
            (input + output + cache_w + cache_r).max(0)
        )),
    )
}

fn build_hourly_heatmap(hourly: &[db::HourlyActivity]) -> String {
    let mut counts = [0i64; 24];
    let mut costs = [0.0f64; 24];
    for h in hourly {
        if h.hour >= 0 && h.hour < 24 {
            counts[h.hour as usize] = h.session_count;
            costs[h.hour as usize] = h.total_cost;
        }
    }
    let max_count = counts.iter().copied().max().unwrap_or(0).max(1) as f64;
    let mut html = String::from(r#"<div class="heatmap">"#);
    for hour in 0..24 {
        let label = if hour == 0 {
            "12a".to_string()
        } else if hour < 12 {
            format!("{hour}a")
        } else if hour == 12 {
            "12p".to_string()
        } else {
            format!("{}p", hour - 12)
        };
        let intensity = ((counts[hour] as f64 / max_count) * 0.78 + 0.06).clamp(0.06, 0.92);
        html.push_str(&format!(
            r#"<div class="heat-cell" style="--heat:{intensity:.3}">
  <div class="heat-label">{label}</div>
  <div class="heat-value">{count}</div>
  <div class="heat-meta">{cost}</div>
</div>"#,
            label = html_escape(&label),
            count = counts[hour],
            cost = html_escape(&format_cost(costs[hour])),
        ));
    }
    html.push_str("</div>");
    html
}

fn html_escape(input: &str) -> String {
    neutralize_remote_urls(input)
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn markdown_escape(input: &str) -> String {
    let encoded = html_escape(input).replace(['\r', '\n'], " ");
    let mut escaped = String::with_capacity(encoded.len());
    for ch in encoded.chars() {
        if matches!(
            ch,
            '\\' | '`'
                | '*'
                | '_'
                | '{'
                | '}'
                | '['
                | ']'
                | '('
                | ')'
                | '#'
                | '+'
                | '-'
                | '.'
                | '!'
                | '|'
                | '>'
        ) {
            escaped.push('\\');
        }
        escaped.push(ch);
    }
    escaped
}

fn markdown_code_block(input: &str) -> String {
    neutralize_remote_urls(input).replace("```", "` ` `")
}

fn neutralize_remote_urls(input: &str) -> String {
    input
        .replace("https://", "hxxps://")
        .replace("http://", "hxxp://")
}

fn render_session_row(
    index: usize,
    project: &str,
    model: &str,
    total_tokens: i64,
    duration_secs: i64,
    total_cost: f64,
) -> String {
    format!(
        "<tr><td>{index}</td><td>{project}</td><td>{model}</td><td class=\"num\">{tokens}</td><td class=\"num\">{duration}</td><td class=\"cost\">{cost}</td></tr>",
        project = html_escape(project),
        model = html_escape(model),
        tokens = html_escape(&format_tokens_short(total_tokens)),
        duration = html_escape(&format_duration(duration_secs)),
        cost = html_escape(&format_cost(total_cost)),
    )
}

fn render_prompt_row(
    project: &str,
    complexity_score: u8,
    specificity_score: u8,
    label: &str,
    preview: &str,
) -> String {
    format!(
        r#"<tr><td>{project}</td><td class="num">{complexity_score}</td><td class="num">{specificity_score}</td><td>{label}</td><td class="preview-cell">{preview}</td></tr>"#,
        project = html_escape(project),
        label = html_escape(label),
        preview = html_escape(preview),
    )
}

fn build_recommendations(
    provider: cc_discord_presence::provider::Provider,
    sessions: &[db::HistoricalSession],
    traces: &std::collections::HashMap<String, crate::analyzers::session_trace::SessionTrace>,
) -> String {
    use super::analyzers::{
        cache_health, inflection, model_routing, prompt_complexity, recommendations,
        session_health, tool_frequency,
    };
    let provider_name = provider.display_name();

    let cache = cache_health::analyze_for_provider(provider, sessions);
    let routing = model_routing::analyze(sessions);
    let inflections = inflection::detect_for_provider(provider, sessions);
    let tool_frequency = tool_frequency::analyze(sessions, traces);
    let prompt_complexity = prompt_complexity::analyze(sessions, traces);
    let session_health =
        session_health::analyze(sessions, traces, &tool_frequency, &prompt_complexity);
    let ctx = recommendations::AnalysisContext {
        provider,
        sessions,
        cache: &cache,
        routing: provider.capabilities().model_routing.then_some(&routing),
        inflections: &inflections,
        tool_frequency: Some(&tool_frequency),
        prompt_complexity: Some(&prompt_complexity),
        session_health: Some(&session_health),
    };
    let recs = recommendations::generate(&ctx);

    if recs.is_empty() {
        return r#"<li class="rec-item positive"><div class="rec-title">All clear</div><div class="rec-desc">No specific recommendations at this time.</div></li>"#.to_string();
    }

    recs.iter()
        .map(|r| {
            let severity_key = match r.severity {
                super::analyzers::Severity::Critical => "critical",
                super::analyzers::Severity::Warning => "warning",
                super::analyzers::Severity::Info => "info",
                super::analyzers::Severity::Positive => "positive",
            };
            let fix_prompt = if r.fix_prompt.is_empty()
                || matches!(r.severity, super::analyzers::Severity::Positive)
            {
                None
            } else {
                Some(r.fix_prompt.as_str())
            };
            render_recommendation(
                provider_name,
                severity_key,
                &r.title,
                &r.description,
                &r.action,
                r.estimated_savings.as_deref(),
                fix_prompt,
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_recommendation(
    provider_name: &str,
    severity_key: &str,
    title: &str,
    description: &str,
    action: &str,
    estimated_savings: Option<&str>,
    fix_prompt: Option<&str>,
) -> String {
    let savings = estimated_savings
        .map(|value| {
            format!(
                r#"<div class="rec-meta"><span class="meta-k">Potential savings</span><span class="meta-v accent">{}</span></div>"#,
                html_escape(value)
            )
        })
        .unwrap_or_default();
    let fix_details = fix_prompt
        .map(|prompt| {
            format!(
                r#"<details class="rec-fix"><summary>Fix with {provider}</summary><pre class="rec-fix-prompt">{prompt}</pre></details>"#,
                provider = html_escape(provider_name),
                prompt = html_escape(prompt),
            )
        })
        .unwrap_or_default();
    format!(
        r##"<li class="rec-item rec-{sev}" data-sev="{sev}">
  <div class="rec-head">
    <span class="rec-pill {sev}">{sev_label}</span>
    <div class="rec-title">{title}</div>
  </div>
  <div class="rec-desc">{desc}</div>
  {savings}
  <div class="rec-meta"><span class="meta-k">Action</span><span class="meta-v">{action}</span></div>
  {fix_details}
</li>"##,
        sev = severity_key,
        sev_label = severity_key.to_uppercase(),
        title = html_escape(title),
        desc = html_escape(description),
        action = html_escape(action),
        savings = savings,
        fix_details = fix_details,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOSTILE: &str = r#"</td><script>globalThis.pwned=1</script><img src=x onerror=alert(1)><a href="https://evil.example/x">&boom</a>"#;

    fn assert_inert(fragment: &str) {
        assert!(!fragment.contains("<script"));
        assert!(!fragment.contains("<img"));
        assert!(!fragment.contains("<a href"));
        assert!(!fragment.contains("https://evil.example"));
        assert!(fragment.contains("&lt;script&gt;"));
        assert!(fragment.contains("hxxps://evil.example"));
    }

    #[test]
    fn homogeneous_estimated_costs_remain_estimated() {
        assert_eq!(
            merge_cost_basis(db::CostBasis::Estimated, db::CostBasis::Estimated),
            db::CostBasis::Estimated
        );
        assert_eq!(
            merge_cost_basis(db::CostBasis::Exact, db::CostBasis::Estimated),
            db::CostBasis::Partial
        );
    }

    #[test]
    fn database_session_project_and_model_fields_are_inert_html() {
        let row = render_session_row(1, HOSTILE, HOSTILE, 42, 60, 0.25);
        assert_inert(&row);
        assert_eq!(row.matches("&lt;script&gt;").count(), 2);
    }

    #[test]
    fn database_project_and_date_aggregates_are_inert_html() {
        let project = db::ProjectStat {
            project: HOSTILE.to_string(),
            session_count: 1,
            priced_sessions: 1,
            cost_basis: db::CostBasis::Exact,
            cost_sources: vec!["fixture".to_string()],
            total_cost: 0.25,
            total_tokens: 42,
            avg_session_cost: 0.25,
            avg_duration_secs: 60.0,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            top_model: HOSTILE.to_string(),
        };
        let project_table = build_project_table(&[project]);
        assert_inert(&project_table);

        let daily = std::collections::BTreeMap::from([(HOSTILE.to_string(), 0.25)]);
        let daily_chart = build_daily_cost_svg(&daily);
        assert_inert(&daily_chart);
    }

    #[test]
    fn top_sessions_never_promote_unavailable_raw_cost() {
        let exact = db::HistoricalSession {
            project: "priced".into(),
            model: "gpt-5.6".into(),
            total_cost: 2.5,
            known_cost: Some(2.5),
            cost_basis: db::CostBasis::Exact,
            cost_source: "session-calculated".into(),
            ..db::HistoricalSession::default()
        };
        let unavailable = db::HistoricalSession {
            project: "unpriced".into(),
            model: "gpt-5.6".into(),
            total_cost: 99.0,
            known_cost: None,
            cost_basis: db::CostBasis::Unavailable,
            cost_source: "unknown".into(),
            ..db::HistoricalSession::default()
        };

        let html = build_top_sessions(&[exact, unavailable]);

        assert!(html.contains("priced"));
        assert!(html.contains("$2.50"));
        assert!(!html.contains("unpriced"));
        assert!(!html.contains("$99.00"));
    }

    #[test]
    fn prompt_project_label_and_preview_fields_are_inert_html() {
        let row = render_prompt_row(HOSTILE, 90, 10, HOSTILE, HOSTILE);
        assert_inert(&row);
        assert_eq!(row.matches("&lt;script&gt;").count(), 3);
    }

    #[test]
    fn recommendation_copy_and_fix_prompt_are_inert_html() {
        let recommendation = render_recommendation(
            "Codex",
            "warning",
            HOSTILE,
            HOSTILE,
            HOSTILE,
            Some(HOSTILE),
            Some(HOSTILE),
        );
        assert_inert(&recommendation);
        assert_eq!(recommendation.matches("&lt;script&gt;").count(), 5);
        assert!(!recommendation.contains("data-prompt="));
        assert!(!recommendation.contains("<button"));
        assert!(recommendation.contains("<details class=\"rec-fix\">"));
    }

    #[test]
    fn markdown_external_text_cannot_create_html_or_links() {
        let escaped = markdown_escape(HOSTILE);
        assert!(!escaped.contains("<script"));
        assert!(!escaped.contains("https://evil.example"));
        assert!(escaped.contains("&lt;script&gt;"));
        assert!(escaped.contains("hxxps://evil\\.example/x"));
    }

    /// Reports receive human labels ("Claude Opus 5"), not API ids, so the
    /// normalizer has to round-trip them back into a form `is_fast_capable`
    /// understands. Opus 5.5, Opus 5 and Opus 4.8 are the fast-capable models today.
    #[test]
    fn model_label_fast_capable_covers_opus_5_5_5_and_4_8() {
        assert!(model_label_is_fast_capable("Claude Opus 5.5"));
        assert!(model_label_is_fast_capable("Claude Opus 5"));
        assert!(model_label_is_fast_capable("Claude Opus 4.8"));
    }

    #[test]
    fn model_label_fast_capable_excludes_earlier_and_other_families() {
        assert!(!model_label_is_fast_capable("Claude Opus 4.7"));
        assert!(!model_label_is_fast_capable("Claude Opus 4.6"));
        assert!(!model_label_is_fast_capable("Claude Sonnet 5"));
        assert!(!model_label_is_fast_capable("Claude Fable 5"));
    }
}
