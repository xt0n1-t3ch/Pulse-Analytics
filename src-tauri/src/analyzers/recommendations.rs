//! Recommendations engine — turns the raw analyzer output into a short list
//! of actionable items. Each item carries a `fix_prompt` plus provider-aware
//! UI copy so the frontend can present the right action verb for Claude or
//! Codex without hardcoding strings.

use cc_discord_presence::provider::Provider;
use serde::Serialize;

use super::Severity;
use super::cache_health::CacheHealthReport;
use super::inflection::InflectionPoint;
use super::model_routing::ModelRoutingReport;
use super::prompt_complexity::PromptComplexityReport;
use super::session_health::SessionHealthReport;
use super::tool_frequency::ToolFrequencyReport;
use crate::db::HistoricalSession;

#[derive(Debug, Clone, Serialize)]
pub struct Recommendation {
    pub id: String,
    pub severity: Severity,
    pub title: String,
    pub description: String,
    pub estimated_savings: Option<String>,
    pub action: String,
    pub fix_prompt: String,
    pub fix_label: String,
    pub instruction_file: String,
    pub color: &'static str,
}

pub struct AnalysisContext<'a> {
    pub provider: Provider,
    pub sessions: &'a [HistoricalSession],
    pub cache: &'a CacheHealthReport,
    pub routing: Option<&'a ModelRoutingReport>,
    pub inflections: &'a [InflectionPoint],
    pub tool_frequency: Option<&'a ToolFrequencyReport>,
    pub prompt_complexity: Option<&'a PromptComplexityReport>,
    pub session_health: Option<&'a SessionHealthReport>,
}

pub fn generate(ctx: &AnalysisContext) -> Vec<Recommendation> {
    let mut recs: Vec<Recommendation> = Vec::new();
    let provider_name = ctx.provider.display_name();
    let instruction_file = ctx.provider.instruction_file_name();
    let home_dir = ctx.provider.home_dir_name();
    let fix_label = ctx.provider.fix_action_label().to_string();
    let session_store = ctx.provider.sessions_glob_label();

    if ctx.cache.trend_weighted_ratio < 50.0 && ctx.cache.sessions_analyzed > 3 {
        let sev = if ctx.cache.trend_weighted_ratio < 30.0 {
            Severity::Critical
        } else {
            Severity::Warning
        };
        recs.push(Recommendation {
            id: "cache-hit-low".into(),
            severity: sev,
            title: "Prompt cache is underperforming".into(),
            description: format!(
                "Recent cache hit ratio is {:.0}% — every turn is re-paying for the stable part \
                of your context. Usually this means {instruction_file} changed recently or volatile content \
                moved to the top of the prompt.",
                ctx.cache.trend_weighted_ratio
            ),
            estimated_savings: Some("up to 40–60% fewer input tokens".into()),
            action: format!(
                "Audit {instruction_file} for volatile content. Keep stable text (rules, project overview) at the top; put anything that changes per session at the bottom."
            ),
            fix_prompt: format!(
                "Analyze my {provider_name} cache health. Read the {instruction_file} files in this repo and in {home_dir}. Identify sections that are likely changing between sessions and invalidating the prompt cache (current hit ratio: {:.0}%). Suggest a reordering that pins stable rules at the top and pushes volatile content to the bottom.",
                ctx.cache.trend_weighted_ratio
            ),
            fix_label: fix_label.clone(),
            instruction_file: instruction_file.to_string(),
            color: sev.color_hint(),
        });
    } else if ctx.cache.trend_weighted_ratio >= 70.0 && ctx.cache.sessions_analyzed > 5 {
        recs.push(Recommendation {
            id: "cache-healthy".into(),
            severity: Severity::Positive,
            title: "Cache health is strong".into(),
            description: format!(
                "{:.0}% of your input is served from cache. Keep your stable prompts stable — small reshuffles can drop this grade quickly.",
                ctx.cache.trend_weighted_ratio
            ),
            estimated_savings: None,
            action: "No action — maintain current prompt stability.".into(),
            fix_prompt: format!(
                "No fix needed — my {provider_name} cache hit ratio is healthy. Just verify nothing has changed that would regress it."
            ),
            fix_label: fix_label.clone(),
            instruction_file: instruction_file.to_string(),
            color: Severity::Positive.color_hint(),
        });
    }

    if let Some(routing) = ctx.routing
        && routing.savings_estimate_available
        && routing.opus.cost_share_pct >= 90.0
        && routing.total_cost > 10.0
        && routing.estimated_savings_if_rerouted > 0.0
    {
        recs.push(Recommendation {
            id: "opus-dominance".into(),
            severity: Severity::Warning,
            title: "Opus-tier models are carrying almost all your spend".into(),
            description: format!(
                "{:.0}% of cost is premium-tier reasoning ({} sessions). Simple lookups, commit messages, and quick refactors often run fine on cheaper models.",
                routing.opus.cost_share_pct, routing.opus.sessions
            ),
            estimated_savings: Some(format!(
                "~${:.2} if ~30% of premium work moves down one tier",
                routing.estimated_savings_if_rerouted
            )),
            action: "For the next few sessions, try the mid-tier model first on reads / small edits and only escalate when reasoning depth is needed."
                .into(),
            fix_prompt: format!(
                "Looking at my last 30 days of {provider_name} usage, {:.0}% of cost is coming from my highest-tier reasoning model. Suggest a concrete routing workflow: which classes of tasks should I push to cheaper models without hurting quality?",
                routing.opus.cost_share_pct
            ),
            fix_label: fix_label.clone(),
            instruction_file: instruction_file.to_string(),
            color: Severity::Warning.color_hint(),
        });
    }

    if let Some(spike) = ctx.inflections.iter().find(|p| p.direction == "spike") {
        recs.push(Recommendation {
            id: format!("inflection-spike-{}", spike.date),
            severity: Severity::Warning,
            title: format!("Cost spike on {}", spike.date),
            description: format!(
                "Cost per session ran {:.1}× the prior 3-day baseline. ${:.2} across {} session(s). Likely cause: {instruction_file} change, longer contexts, or a {provider_name} update.",
                spike.multiplier, spike.cost_on_day, spike.sessions_on_day
            ),
            estimated_savings: None,
            action: format!(
                "Compare your setup on {} to the days before. `git log --since=\"{} 1 day ago\" --until=\"{} 1 day\"` often reveals the trigger.",
                spike.date, spike.date, spike.date
            ),
            fix_prompt: format!(
                "On {} my {provider_name} cost/session jumped {:.1}× versus the baseline. Check {session_store} for sessions on that date and figure out what changed — {instruction_file} edits, new skills/MCP servers, model routing, or a version bump.",
                spike.date, spike.multiplier
            ),
            fix_label: fix_label.clone(),
            instruction_file: instruction_file.to_string(),
            color: Severity::Warning.color_hint(),
        });
    }

    if let Some(tool_frequency) = ctx.tool_frequency {
        if tool_frequency.avg_tools_per_session >= 40.0 {
            recs.push(Recommendation {
                id: "tool-density-high".into(),
                severity: Severity::Warning,
                title: "Tool-call density is high".into(),
                description: format!(
                    "Average session fires {:.0} tools. Long read/search chains usually mean context drift and a missing compact checkpoint.",
                    tool_frequency.avg_tools_per_session
                ),
                estimated_savings: Some("~10–25% fewer cache-write tokens".into()),
                action: "Compact every 30–40 tool calls. Split recon from implementation so searches do not ride along with edits."
                    .into(),
                fix_prompt: format!(
                    "Review my {provider_name} workflow for tool overuse. I average 40+ tool calls per session. Suggest a tighter cadence for search, compacting, and fresh-session boundaries."
                ),
                fix_label: fix_label.clone(),
                instruction_file: instruction_file.to_string(),
                color: Severity::Warning.color_hint(),
            });
        }

        if tool_frequency.mcp_share_pct >= 20.0 {
            recs.push(Recommendation {
                id: "mcp-heavy".into(),
                severity: Severity::Info,
                title: "MCP traffic is unusually heavy".into(),
                description: format!(
                    "{:.0}% of tool calls are MCP-backed. Extra schemas and server churn can invalidate prompt prefixes.",
                    tool_frequency.mcp_share_pct
                ),
                estimated_savings: Some("~5–15% per avoidable cache break".into()),
                action: "Disconnect idle MCP servers between tasks and keep only the ones needed for the current repo."
                    .into(),
                fix_prompt: format!(
                    "Audit my active {provider_name} MCP servers. Which ones are adding tool-schema bloat without helping the current task, and what should I disable by default?"
                ),
                fix_label: fix_label.clone(),
                instruction_file: instruction_file.to_string(),
                color: Severity::Info.color_hint(),
            });
        }
    }

    if let Some(prompt_complexity) = ctx.prompt_complexity
        && prompt_complexity.available
        && prompt_complexity.avg_specificity_score < 45.0
    {
        recs.push(Recommendation {
            id: "prompt-specificity-low".into(),
            severity: Severity::Info,
            title: "Prompts are broad; specificity can improve".into(),
            description: format!(
                "Average specificity score is {:.0}/100. Missing file paths, exact errors, or line references forces wider repo scans.",
                prompt_complexity.avg_specificity_score
            ),
            estimated_savings: Some("~20–40% fewer exploratory tokens".into()),
            action: "When possible, include file paths, failing command output, and desired end-state in the first prompt."
                .into(),
            fix_prompt: format!(
                "Show me how to rewrite my recent {provider_name} prompts so they are more specific. Use file paths, failing commands, exact errors, and concrete acceptance criteria."
            ),
            fix_label: fix_label.clone(),
            instruction_file: instruction_file.to_string(),
            color: Severity::Info.color_hint(),
        });
    }

    if let Some(session_health) = ctx.session_health
        && session_health.available
        && session_health.peak_overlap_pct > 40
    {
        let overlap_copy = match ctx.provider {
            Provider::Claude => "Anthropic peak hours",
            Provider::Codex | Provider::OpenCode | Provider::CommandCode => {
                "your busiest overlap window"
            }
        };
        recs.push(Recommendation {
            id: "peak-hour-overlap".into(),
            severity: Severity::Info,
            title: "A lot of work lands in throttled hours".into(),
            description: format!(
                "{}% of traced session traffic overlaps {overlap_copy}.",
                session_health.peak_overlap_pct
            ),
            estimated_savings: Some("~more stable long-session headroom off-peak".into()),
            action: "Shift big refactors, test generation, or long debugging runs outside your busiest window when possible."
                .into(),
            fix_prompt: format!(
                "Help me reschedule the most expensive {provider_name} work to off-peak hours. Which task types should I batch for later to preserve my session limits?"
            ),
            fix_label: fix_label.clone(),
            instruction_file: instruction_file.to_string(),
            color: Severity::Info.color_hint(),
        });
    }

    let long_sessions: usize = ctx
        .sessions
        .iter()
        .filter(|s| s.duration_secs > 7200)
        .count();
    if long_sessions >= 5 {
        recs.push(Recommendation {
            id: "long-sessions".into(),
            severity: Severity::Info,
            title: format!("{long_sessions} sessions exceed 2 hours"),
            description: "Very long sessions drift context and eat cache. Shorter, focused sessions tend to use tokens more efficiently and compact less."
                .into(),
            estimated_savings: Some("10–25% fewer cache-write tokens".into()),
            action: "Break long pairings into feature-sized sessions. Compact or start fresh when you shift topics."
                .into(),
            fix_prompt: format!(
                "I keep hitting multi-hour {provider_name} sessions. Suggest a workflow for chunking my work into shorter focused sessions without losing thread between them (e.g. summaries, per-feature branches, compact cadence)."
            ),
            fix_label: fix_label.clone(),
            instruction_file: instruction_file.to_string(),
            color: Severity::Info.color_hint(),
        });
    }

    let expensive: Vec<&HistoricalSession> = ctx
        .sessions
        .iter()
        .filter(|s| {
            s.cost_basis != crate::db::CostBasis::Unavailable
                && s.known_cost.is_some_and(|cost| cost > 20.0)
        })
        .collect();
    if !expensive.is_empty() {
        recs.push(Recommendation {
            id: "high-cost-sessions".into(),
            severity: Severity::Info,
            title: format!("{} high-cost session(s) (>$20)", expensive.len()),
            description: "A handful of sessions carry outsized cost. Worth knowing which ones and why — they're usually either rabbit-hole debugging or repeated re-reads."
                .into(),
            estimated_savings: None,
            action: "Open the Sessions tab, sort by cost, and review the top 3 — the pattern is usually obvious once you see them lined up."
                .into(),
            fix_prompt: format!(
                "Analyze my 3 most expensive {provider_name} sessions from Pulse analytics history. What made each one expensive, and what would have kept it cheaper?"
            ),
            fix_label: fix_label.clone(),
            instruction_file: instruction_file.to_string(),
            color: Severity::Info.color_hint(),
        });
    }

    if recs.is_empty() {
        recs.push(Recommendation {
            id: "all-good".into(),
            severity: Severity::Positive,
            title: "No action items right now".into(),
            description: "Your cache, model routing, and session shape all look healthy.".into(),
            estimated_savings: None,
            action: "Keep going.".into(),
            fix_prompt: String::new(),
            fix_label: fix_label.clone(),
            instruction_file: instruction_file.to_string(),
            color: Severity::Positive.color_hint(),
        });
    }

    recs
}
