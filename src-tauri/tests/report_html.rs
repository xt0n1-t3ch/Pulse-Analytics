//! Integration tests for the HTML report: rendering and the no-remote-assets guarantee.

use std::path::PathBuf;

use cc_discord_presence::provider::Provider;
use pulse::report::{generate_html_report_for_provider, generate_markdown_report_for_provider};

fn html() -> String {
    generate_html_report_for_provider(Provider::Claude, Some(30), None)
}

fn sample_html_path() -> PathBuf {
    // Cargo's scratch folder for integration tests, inside the target directory.
    std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("pulse-report-sample.html")
}

#[test]
fn writes_rendered_sample_for_visual_review() {
    isolate_storage();
    let report = html();
    let path = sample_html_path();
    std::fs::write(&path, &report).expect("write sample report");
    assert!(
        path.exists(),
        "sample report should be written for /browser review"
    );
    eprintln!("pulse sample report written to {}", path.display());
}

#[test]
fn html_report_has_no_remote_font_or_asset_references() {
    isolate_storage();
    let report = html();
    assert!(
        !report.contains("fonts.googleapis.com"),
        "report must not pull Google Fonts (offline regression guard)"
    );
    assert!(
        !report.contains("fonts.gstatic.com"),
        "report must not pull gstatic font binaries"
    );
    assert!(
        !report.contains("@import url("),
        "report must not @import any remote stylesheet"
    );
    assert!(
        !report.contains("http://"),
        "report must not reference any http:// asset"
    );
    for occurrence in report.match_indices("https://") {
        let (idx, _) = occurrence;
        let tail = &report[idx..(idx + 120).min(report.len())];
        let is_allowed_namespace =
            tail.starts_with("https://www.w3.org") || tail.starts_with("https://github.com");
        assert!(
            is_allowed_namespace,
            "unexpected https reference in report (must be offline-safe): {tail}"
        );
    }
}

#[test]
fn html_report_is_well_formed_with_style_and_charts() {
    isolate_storage();
    let report = html();
    assert!(!report.trim().is_empty(), "report must be non-empty");
    assert!(
        report.starts_with("<!DOCTYPE html>"),
        "report must open with doctype"
    );
    assert_eq!(
        report.matches("<html").count(),
        1,
        "exactly one <html> open tag"
    );
    assert_eq!(
        report.matches("</html>").count(),
        1,
        "exactly one </html> close tag"
    );
    assert!(
        report.contains("<style>") && report.contains("</style>"),
        "report must inline its CSS"
    );
    assert!(report.contains("</body>"), "report must close its body");
    assert!(
        report.contains("<svg"),
        "report must render inline SVG charts"
    );
    assert!(
        report.contains(r#"aria-label="Token composition""#),
        "token composition chart must be present"
    );
}

#[test]
fn html_report_enforces_a_scriptless_offline_content_security_policy() {
    isolate_storage();
    let report = html();
    let expected_policy = "default-src 'none'; style-src 'unsafe-inline'; script-src 'none'; img-src 'none'; font-src 'none'; connect-src 'none'; media-src 'none'; object-src 'none'; frame-src 'none'; child-src 'none'; worker-src 'none'; manifest-src 'none'; base-uri 'none'; form-action 'none'; navigate-to 'none'";

    assert!(
        report.contains(&format!(
            r#"<meta http-equiv="Content-Security-Policy" content="{expected_policy}">"#
        )),
        "offline report must publish the exact restrictive CSP"
    );
    assert!(
        !report.contains("<script"),
        "offline report must not contain executable script blocks"
    );
    assert!(
        !report.contains("onclick="),
        "offline report must not contain inline event handlers"
    );
    assert!(
        !report.contains("javascript:"),
        "offline report must not contain script navigation URLs"
    );
    assert!(
        !report.contains(r#"<a href="http"#),
        "offline report must not expose network navigation anchors"
    );
}

#[test]
fn html_and_markdown_reports_escape_hostile_project_filters() {
    isolate_storage();
    let hostile = r#"</div><script>globalThis.pwned=1</script><a href="https://evil.example/x" data-x='1'>&boom</a>"#;
    let report = generate_html_report_for_provider(Provider::Claude, Some(30), Some(hostile));
    let markdown = generate_markdown_report_for_provider(Provider::Claude, Some(30), Some(hostile));

    assert!(
        !report.contains(hostile),
        "raw hostile HTML must never survive"
    );
    assert!(
        report.contains("&lt;/div&gt;&lt;script&gt;globalThis.pwned=1&lt;/script&gt;&lt;a href=&quot;hxxps://evil.example/x&quot; data-x=&#39;1&#39;&gt;&amp;boom&lt;/a&gt;"),
        "hostile filter must render only as inert encoded text"
    );
    assert!(
        !markdown.contains("<script>"),
        "Markdown export must not retain executable raw HTML"
    );
    assert!(
        markdown.contains("&lt;script&gt;") && markdown.contains("&lt;/script&gt;"),
        "Markdown export must encode hostile HTML"
    );
}

#[test]
fn report_uses_one_provider_snapshot_for_every_provider_specific_label() {
    isolate_storage();
    for (provider, expected_html, expected_markdown, foreign_html, foreign_markdown) in [
        (
            Provider::Claude,
            "<div class=\"info-value\">Claude Code</div><p>CLAUDE.md · Fix with Claude Code</p>",
            "- Provider: Claude Code\n- Instruction file: CLAUDE.md",
            "<div class=\"info-value\">Codex</div><p>AGENTS.md · Fix with Codex</p>",
            "- Provider: Codex\n- Instruction file: AGENTS.md",
        ),
        (
            Provider::Codex,
            "<div class=\"info-value\">Codex</div><p>AGENTS.md · Fix with Codex</p>",
            "- Provider: Codex\n- Instruction file: AGENTS.md",
            "<div class=\"info-value\">Claude Code</div><p>CLAUDE.md · Fix with Claude Code</p>",
            "- Provider: Claude Code\n- Instruction file: CLAUDE.md",
        ),
    ] {
        let report = generate_html_report_for_provider(provider, Some(30), None);
        let markdown = generate_markdown_report_for_provider(provider, Some(30), None);

        assert!(report.contains(expected_html));
        assert!(markdown.contains(expected_markdown));
        assert!(!report.contains(foreign_html));
        assert!(!markdown.contains(foreign_markdown));
    }
}

#[test]
fn codex_reports_keep_cache_health_and_omit_claude_only_routing() {
    isolate_storage();
    let html = generate_html_report_for_provider(Provider::Codex, Some(30), None);
    let markdown = generate_markdown_report_for_provider(Provider::Codex, Some(30), None);

    assert!(html.contains(r#"id="cache""#));
    assert!(markdown.contains("## Cache"));
    assert!(!html.contains(r#"id="routing""#));
    assert!(!html.contains("Model Routing Analysis"));
    assert!(!html.contains("Opus 4.8+"));
    assert!(!markdown.contains("## Routing"));
    assert!(!markdown.contains("Speed Split"));
    assert!(!markdown.contains("Opus 4.8+"));
}

#[test]
fn html_report_contains_brand_kpi_and_sections() {
    isolate_storage();
    let report = html();
    assert!(report.contains("Pulse"), "brand kicker present");
    assert!(report.contains("Analytics Report"), "report title present");
    assert!(
        report.contains(r#"class="summary-grid""#),
        "KPI strip present"
    );
    assert!(
        report.contains(r#"class="summary-card""#),
        "KPI cards present"
    );
    for anchor in [
        r#"id="cache""#,
        r#"id="routing""#,
        r#"id="inflections""#,
        r#"id="sessions""#,
        r#"id="tools""#,
        r#"id="topology""#,
        r#"id="prompts""#,
        r#"id="recommendations""#,
    ] {
        assert!(
            report.contains(anchor),
            "missing analyzer section: {anchor}"
        );
    }
    assert!(
        report.contains("Speed Split"),
        "speed split surfacing present"
    );
    assert!(
        report.contains(r#"class="speed-split""#),
        "speed split layout present"
    );
}

#[test]
fn html_report_uses_offline_system_font_stack() {
    isolate_storage();
    let report = html();
    assert!(
        report.contains("-apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, Inter, sans-serif"),
        "body must use the self-contained system UI font stack"
    );
    assert!(
        report.contains("'JetBrains Mono', 'SF Mono', 'Cascadia Code', Consolas, monospace"),
        "monospace must use a self-contained stack"
    );
}

#[test]
fn markdown_report_is_valid_non_empty_gfm_with_sections() {
    isolate_storage();
    let md = generate_markdown_report_for_provider(Provider::Claude, Some(30), None);
    assert!(!md.trim().is_empty(), "markdown report must be non-empty");
    assert!(
        md.contains("# Pulse Analytics Report"),
        "top-level heading present"
    );
    for heading in [
        "## Executive Summary",
        "## Cache",
        "## Routing",
        "### Speed Split",
        "## Inflections",
        "## Sessions",
        "## Tools",
        "## Telemetry Topology",
        "## Prompts",
    ] {
        assert!(md.contains(heading), "missing markdown section: {heading}");
    }
    assert!(
        md.contains("| Tier | Sessions | Cost | Share |"),
        "speed split table header present"
    );
    assert!(
        !md.contains("fonts.googleapis.com"),
        "markdown must not embed remote font references"
    );
}

fn isolate_storage() {
    static ROOT: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
    ROOT.get_or_init(|| {
        // Kept inside the target directory: a static is never dropped.
        let root =
            tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).expect("isolated report storage");
        // All test entry points wait for this initialization before reading state.
        unsafe {
            std::env::set_var("PULSE_HOME", root.path());
            std::env::set_var("CLAUDE_HOME", root.path().join("claude-source"));
            std::env::set_var("CODEX_HOME", root.path().join("codex-source"));
        }
        root
    });
}
