//! Static HTML report template: stylesheet, content security policy, and document head and tail.

pub const REPORT_CSS: &str = r##":root {
  color-scheme: dark;
  --bg: #000000;
  --bg-secondary: #050505;
  --bg-card: #0b0b0b;
  --bg-card-hover: #121212;
  --bg-elevated: #141414;
  --border: #1f1f1f;
  --border-hover: #2a2a2a;
  --border-strong: #333333;
  --text-primary: #f5f5f5;
  --text-secondary: #a3a3a3;
  --text-muted: #6b6b6b;
  --success: #22c55e;
  --success-dim: rgba(34,197,94,0.10);
  --warning: #fbbf24;
  --warning-dim: rgba(251,191,36,0.10);
  --danger: #ef4444;
  --danger-dim: rgba(239,68,68,0.10);
  --info: #7cb9e8;
  --info-dim: rgba(124,185,232,0.10);
  --radius-xs: 4px;
  --radius-sm: 6px;
  --radius-md: 8px;
  --radius-lg: 12px;
  --radius-full: 9999px;
  --shadow-sm: 0 2px 6px rgba(0,0,0,0.6);
  --shadow-md: 0 6px 20px rgba(0,0,0,0.7);
  --font-sans: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, Inter, sans-serif;
  --font-mono: 'JetBrains Mono', 'SF Mono', 'Cascadia Code', Consolas, monospace;
  --ease: cubic-bezier(0.4, 0, 0.2, 1);
}
[data-theme="light"] {
  color-scheme: light;
  --bg: #ffffff;
  --bg-secondary: #fafafa;
  --bg-card: #ffffff;
  --bg-card-hover: #f7f7f7;
  --bg-elevated: #f1f1f1;
  --border: #e5e5e5;
  --border-hover: #d4d4d4;
  --border-strong: #b8b8b8;
  --text-primary: #0a0a0a;
  --text-secondary: #525252;
  --text-muted: #8a8a8a;
  --success: #16a34a;
  --warning: #d97706;
  --danger: #dc2626;
  --info: #2563eb;
}
@media (prefers-color-scheme: light) {
  :root:not([data-theme]) {
    color-scheme: light;
    --bg: #ffffff; --bg-secondary: #fafafa; --bg-card: #ffffff;
    --bg-card-hover: #f7f7f7; --bg-elevated: #f1f1f1;
    --border: #e5e5e5; --border-hover: #d4d4d4; --border-strong: #b8b8b8;
    --text-primary: #0a0a0a; --text-secondary: #525252; --text-muted: #8a8a8a;
    --success: #16a34a; --warning: #d97706; --danger: #dc2626; --info: #2563eb;
  }
}
*, *::before, *::after { margin: 0; padding: 0; box-sizing: border-box; }
html { scroll-behavior: smooth; }
body {
  background: var(--bg); color: var(--text-primary);
  font-family: var(--font-sans); font-size: 14px; line-height: 1.5;
  padding: 40px 24px 64px;
  -webkit-font-smoothing: antialiased; -moz-osx-font-smoothing: grayscale;
  font-variant-numeric: tabular-nums;
  font-feature-settings: 'cv02','cv03','cv04','cv11';
}
.report-shell { max-width: 1240px; margin: 0 auto; }
a { color: inherit; }

.kicker,.summary-label,.info-label,.metric .label,.heat-label,.routing-share {
  font-family: var(--font-mono); font-size: 10px; font-weight: 600;
  letter-spacing: 0.12em; text-transform: uppercase; color: var(--text-muted);
}

.hero { padding: 8px 0 20px; }
.hero-top { display: flex; justify-content: space-between; align-items: flex-start; gap: 24px; margin-bottom: 4px; }
.hero h1 {
  font-family: var(--font-sans); font-weight: 700;
  font-size: clamp(32px, 4.2vw, 44px); line-height: 1.05;
  letter-spacing: -0.025em; color: var(--text-primary); margin: 6px 0 4px;
}
.hero-meta { color: var(--text-secondary); font-size: 14px; margin-top: 2px; }
.generated-at { font-family: var(--font-mono); font-size: 10px; letter-spacing: 0.1em; text-transform: uppercase; color: var(--text-muted); }
.hero-divider { height: 1px; background: var(--border); margin: 20px 0 24px; }

.summary-grid {
  display: grid; grid-template-columns: repeat(5, minmax(0,1fr));
  gap: 10px;
}
.summary-card {
  background: var(--bg-card); border: 1px solid var(--border);
  border-radius: var(--radius-lg); padding: 18px 18px 16px;
  transition: border-color .15s var(--ease), background .15s var(--ease);
}
.summary-card:hover { border-color: var(--border-hover); background: var(--bg-card-hover); }
.summary-value {
  font-family: var(--font-sans); font-weight: 700;
  font-size: clamp(22px, 2.2vw, 28px); letter-spacing: -0.02em;
  color: var(--text-primary); margin: 10px 0 4px;
  font-variant-numeric: tabular-nums; line-height: 1.1;
}
.summary-meta { color: var(--text-muted); font-size: 11px; line-height: 1.4; font-family: var(--font-mono); }

.anchor-nav {
  position: sticky; top: 0; z-index: 20;
  display: flex; flex-wrap: wrap; gap: 0;
  margin: 28px 0 24px; padding: 0;
  background: color-mix(in srgb, var(--bg) 88%, transparent);
  backdrop-filter: blur(10px); -webkit-backdrop-filter: blur(10px);
  border-top: 1px solid var(--border); border-bottom: 1px solid var(--border);
}
.anchor-nav a {
  padding: 12px 16px; color: var(--text-muted);
  font-family: var(--font-mono); font-size: 10px; font-weight: 600;
  text-transform: uppercase; letter-spacing: 0.12em;
  text-decoration: none;
  border-right: 1px solid var(--border);
  transition: color .15s var(--ease), background .15s var(--ease);
}
.anchor-nav a:hover { color: var(--text-primary); background: var(--bg-card); }

.section { margin-bottom: 48px; }
.section-header {
  display: flex; justify-content: space-between; gap: 20px;
  align-items: flex-end; margin-bottom: 16px;
  padding-bottom: 12px; border-bottom: 1px solid var(--border);
}
.section-header h2 {
  font-family: var(--font-sans); font-weight: 700;
  font-size: clamp(20px, 2.2vw, 26px); letter-spacing: -0.02em;
  color: var(--text-primary); margin: 0;
}
.section-header p { margin: 6px 0 0; color: var(--text-secondary); font-size: 13px; max-width: 64ch; }
.section-grid { display: grid; grid-template-columns: repeat(2, minmax(0,1fr)); gap: 10px; }
.info-grid { display: grid; grid-template-columns: repeat(4, minmax(0,1fr)); gap: 10px; }
.metric-strip { display: grid; grid-template-columns: repeat(3, minmax(0,1fr)); gap: 10px; margin-top: 16px; }

.card,.info-card {
  background: var(--bg-card); border: 1px solid var(--border);
  border-radius: var(--radius-lg); padding: 22px;
  transition: border-color .15s var(--ease), background .15s var(--ease);
}
.card:hover,.info-card:hover { border-color: var(--border-hover); }
.card > h2, .card > h3, .info-card > h2, .info-card > h3 {
  font-family: var(--font-mono); font-size: 10px; font-weight: 600;
  letter-spacing: 0.12em; text-transform: uppercase;
  color: var(--text-muted); margin: 0 0 14px;
}

.metric {
  background: var(--bg-secondary); border: 1px solid var(--border);
  border-radius: var(--radius-md); padding: 14px 16px;
}
.metric .label { display: block; margin-bottom: 6px; }
.metric .value {
  font-family: var(--font-sans); font-weight: 700;
  font-size: 18px; color: var(--text-primary);
  font-variant-numeric: tabular-nums; letter-spacing: -0.01em;
}

.speed-split { display: grid; grid-template-columns: repeat(2, minmax(0,1fr)); gap: 10px; }
.speed-cell {
  background: var(--bg-secondary); border: 1px solid var(--border);
  border-radius: var(--radius-md); padding: 16px 18px; position: relative;
}
.speed-cell.is-fast { border-color: var(--warning); background: var(--warning-dim); }
.speed-head { display: flex; align-items: center; gap: 8px; }
.speed-name {
  font-family: var(--font-mono); font-size: 10px; font-weight: 600;
  letter-spacing: 0.12em; text-transform: uppercase; color: var(--text-muted);
}
.speed-bolt { color: var(--warning); font-size: 13px; line-height: 1; }
.speed-value {
  font-family: var(--font-sans); font-weight: 700; font-size: 24px;
  color: var(--text-primary); letter-spacing: -0.02em; margin: 8px 0 2px;
  font-variant-numeric: tabular-nums;
}
.speed-meta { font-family: var(--font-mono); font-size: 11px; color: var(--text-muted); }
.speed-bar { height: 3px; margin-top: 12px; background: var(--border); border-radius: var(--radius-xs); overflow: hidden; }
.speed-fill { height: 100%; background: var(--text-primary); }
.speed-cell.is-fast .speed-fill { background: var(--warning); }

.cache-grade { display: flex; gap: 22px; align-items: center; margin-bottom: 16px; }
.cache-letter {
  font-family: var(--font-sans); font-weight: 800;
  font-size: clamp(72px, 9vw, 108px); line-height: 0.9;
  letter-spacing: -0.06em;
}
.cache-copy h3 {
  font-family: var(--font-mono); font-size: 10px; font-weight: 600;
  letter-spacing: 0.12em; text-transform: uppercase;
  color: var(--text-muted); margin-bottom: 4px;
}
.cache-copy .ratio {
  font-family: var(--font-sans); font-weight: 700;
  font-size: 26px; color: var(--text-primary); letter-spacing: -0.02em;
}
.cache-copy p { color: var(--text-secondary); font-size: 13px; margin-top: 4px; max-width: 48ch; }

.report-svg { width: 100%; height: 240px; display: block; }
.token-legend { list-style: none; padding: 0; margin: 14px 0 0 0; display: grid; grid-template-columns: repeat(2, minmax(0,1fr)); gap: 8px 18px; font-family: var(--font-mono); font-size: 11px; color: var(--text-secondary); letter-spacing: 0.02em; }
.token-legend li { display: flex; align-items: center; gap: 8px; }
.token-legend li b { margin-left: auto; color: var(--text-primary); font-weight: 600; }
.token-legend .dot { display: inline-block; width: 8px; height: 8px; border-radius: 50%; flex-shrink: 0; }

.routing-row + .routing-row { margin-top: 16px; padding-top: 16px; border-top: 1px solid var(--border); }
.routing-label-row { display: flex; justify-content: space-between; align-items: baseline; gap: 12px; }
.routing-name {
  font-family: var(--font-sans); font-weight: 600;
  font-size: 15px; color: var(--text-primary); letter-spacing: -0.01em;
}
.routing-meta { margin-top: 3px; color: var(--text-muted); font-size: 11px; font-family: var(--font-mono); }
.routing-track { height: 3px; margin-top: 10px; background: var(--border); border-radius: var(--radius-xs); overflow: hidden; }
.routing-fill { height: 100%; background: var(--text-primary); transition: width 1.2s cubic-bezier(.2,.9,.3,1); }

.inflection-grid { display: grid; grid-template-columns: repeat(3, minmax(0,1fr)); gap: 10px; }
.inflection-card {
  background: var(--bg-card); border: 1px solid var(--border);
  border-radius: var(--radius-lg); padding: 18px; position: relative;
  transition: border-color .15s var(--ease);
}
.inflection-card:hover { border-color: var(--border-hover); }
.inflection-card::before {
  content: ''; position: absolute; top: 0; left: 18px;
  width: 32px; height: 2px; border-radius: 2px;
}
.inflection-up::before { background: var(--danger); }
.inflection-down::before { background: var(--success); }
.inflection-head { display: flex; justify-content: space-between; gap: 10px; align-items: baseline; }
.inflection-date {
  font-family: var(--font-mono); font-size: 10px; color: var(--text-muted);
  letter-spacing: 0.12em; text-transform: uppercase;
}
.inflection-direction {
  font-family: var(--font-mono); font-size: 9px; font-weight: 600;
  letter-spacing: 0.14em; text-transform: uppercase;
  padding: 3px 8px; border-radius: var(--radius-full);
}
.inflection-up .inflection-direction { background: var(--danger-dim); color: var(--danger); }
.inflection-down .inflection-direction { background: var(--success-dim); color: var(--success); }
.inflection-metric {
  font-family: var(--font-sans); font-weight: 700; font-size: 26px;
  color: var(--text-primary); letter-spacing: -0.02em; margin-top: 8px;
  font-variant-numeric: tabular-nums;
}
.inflection-support { font-family: var(--font-mono); font-size: 11px; color: var(--text-muted); margin-top: 2px; }
.inflection-card p { color: var(--text-secondary); font-size: 12px; margin-top: 8px; line-height: 1.5; }

.info-value {
  font-family: var(--font-sans); font-weight: 700;
  font-size: clamp(20px, 2vw, 26px); color: var(--text-primary);
  letter-spacing: -0.02em; margin-top: 6px;
  font-variant-numeric: tabular-nums;
}
.info-card p { color: var(--text-secondary); font-size: 12px; margin-top: 4px; }

table { width: 100%; border-collapse: collapse; }
th, td {
  padding: 10px 12px; border-bottom: 1px solid var(--border);
  text-align: left; vertical-align: top; font-size: 13px;
  font-family: var(--font-sans);
}
th {
  font-family: var(--font-mono); font-size: 10px; font-weight: 600;
  letter-spacing: 0.12em; text-transform: uppercase;
  color: var(--text-muted);
  border-bottom: 1px solid var(--border-hover);
}
td { color: var(--text-secondary); font-variant-numeric: tabular-nums; }
tr:hover td { color: var(--text-primary); }
tr:last-child td { border-bottom: none; }
.num, .cost { text-align: right; font-family: var(--font-mono); }
.cost { color: var(--text-primary); font-weight: 600; }
.fast-tag {
  display: inline-flex; align-items: center; gap: 3px;
  margin-left: 6px; padding: 1px 6px; border-radius: var(--radius-full);
  background: var(--warning-dim); color: var(--warning);
  font-family: var(--font-mono); font-size: 9px; font-weight: 600;
  letter-spacing: 0.08em; text-transform: uppercase;
}
.preview-cell {
  max-width: 420px; color: var(--text-secondary);
  font-size: 12px; line-height: 1.5;
  font-family: var(--font-sans);
}

.heatmap { display: grid; grid-template-columns: repeat(6, minmax(0,1fr)); gap: 4px; }
.heat-cell {
  background: var(--bg-card); border: 1px solid var(--border);
  border-radius: var(--radius-md); padding: 12px;
  background-image: linear-gradient(180deg, rgba(34,197,94, calc(var(--heat,0) * .35)) 0%, transparent 100%);
}
.heat-label { display: block; }
.heat-value {
  font-family: var(--font-sans); font-weight: 700;
  font-size: 16px; color: var(--text-primary); margin-top: 4px;
  font-variant-numeric: tabular-nums; letter-spacing: -0.01em;
}
.heat-meta { color: var(--text-muted); font-size: 10px; font-family: var(--font-mono); }

.empty-state {
  padding: 28px; background: var(--bg-card);
  border: 1px dashed var(--border-hover);
  border-radius: var(--radius-lg);
  color: var(--text-secondary); font-size: 13px; text-align: center;
  font-family: var(--font-sans);
}

.diagram-code {
  margin: 0;
  padding: 16px 18px;
  background: var(--bg-secondary);
  border: 1px solid var(--border);
  border-radius: var(--radius-md);
  color: var(--text-secondary);
  font-family: var(--font-mono);
  font-size: 11px;
  line-height: 1.65;
  white-space: pre-wrap;
  word-break: break-word;
}

.rec-list { list-style: none; padding: 0; margin: 0; display: flex; flex-direction: column; gap: 10px; }
.rec-item {
  background: var(--bg-card); border: 1px solid var(--border);
  border-left: 3px solid var(--text-muted);
  border-radius: var(--radius-lg); padding: 18px 22px;
  transition: border-color .15s var(--ease), background .15s var(--ease);
}
.rec-item:hover { border-color: var(--border-hover); }
.rec-item[data-sev="critical"] { border-left-color: var(--danger); }
.rec-item[data-sev="warning"]  { border-left-color: var(--warning); }
.rec-item[data-sev="info"]     { border-left-color: var(--info); }
.rec-item[data-sev="positive"] { border-left-color: var(--success); }
.rec-head { display: flex; gap: 10px; align-items: center; flex-wrap: wrap; margin-bottom: 6px; }
.rec-pill {
  padding: 3px 8px; border-radius: var(--radius-full);
  font-family: var(--font-mono); font-size: 9px; font-weight: 600;
  letter-spacing: 0.14em; text-transform: uppercase;
}
.rec-pill.critical { background: var(--danger-dim); color: var(--danger); }
.rec-pill.warning  { background: var(--warning-dim); color: var(--warning); }
.rec-pill.info     { background: var(--info-dim); color: var(--info); }
.rec-pill.positive { background: var(--success-dim); color: var(--success); }
.rec-title {
  font-family: var(--font-sans); font-weight: 600;
  font-size: 15px; color: var(--text-primary); letter-spacing: -0.01em;
}
.rec-desc { color: var(--text-secondary); font-size: 13px; line-height: 1.55; margin-top: 4px; }
.rec-meta { margin-top: 10px; display: flex; gap: 14px; flex-wrap: wrap; font-size: 11px; font-family: var(--font-mono); }
.meta-k { color: var(--text-muted); letter-spacing: 0.10em; text-transform: uppercase; }
.meta-v { color: var(--text-secondary); }
.meta-v.accent { color: var(--text-primary); font-weight: 600; }
.rec-fix summary {
  margin-top: 12px; padding: 7px 14px;
  background: var(--bg-elevated); border: 1px solid var(--border-hover);
  border-radius: var(--radius-xs);
  color: var(--text-primary);
  font-family: var(--font-mono); font-size: 10px; font-weight: 600;
  letter-spacing: 0.12em; text-transform: uppercase;
  cursor: pointer; transition: all .15s var(--ease);
  display: inline-flex;
}
.rec-fix summary:hover { background: var(--text-primary); color: var(--bg); border-color: var(--text-primary); }
.rec-fix-prompt {
  margin: 10px 0 0; padding: 12px 14px;
  background: var(--bg-secondary); border: 1px solid var(--border);
  border-radius: var(--radius-md); color: var(--text-secondary);
  font-family: var(--font-mono); font-size: 11px; line-height: 1.55;
  white-space: pre-wrap; overflow-wrap: anywhere;
}

.footer {
  margin-top: 48px; padding: 22px 0; border-top: 1px solid var(--border);
  font-family: var(--font-mono); font-size: 11px; color: var(--text-muted);
  letter-spacing: 0.04em;
  display: flex; flex-wrap: wrap; gap: 18px;
  align-items: baseline; justify-content: space-between;
}
.footer-brand {
  text-transform: uppercase; letter-spacing: 0.14em; font-weight: 600;
  color: var(--text-secondary);
}
.footer-meta b { color: var(--text-primary); font-weight: 600; }
.footer-links { opacity: .85; }

@media (max-width: 1100px) {
  .summary-grid, .info-grid { grid-template-columns: repeat(2, minmax(0,1fr)); }
  .section-grid, .inflection-grid { grid-template-columns: 1fr; }
  .heatmap { grid-template-columns: repeat(3, minmax(0,1fr)); }
}
@media (max-width: 760px) {
  body { padding: 24px 16px 40px; }
  .hero-top, .section-header, .routing-label-row { flex-direction: column; align-items: flex-start; gap: 8px; }
  .summary-grid, .metric-strip, .speed-split { grid-template-columns: 1fr; }
  .cache-grade { flex-direction: column; gap: 14px; align-items: flex-start; }
  table { display: block; overflow-x: auto; white-space: nowrap; }
}

@media print {
  :root {
    color-scheme: light;
    --bg: #ffffff; --bg-secondary: #fafafa; --bg-card: #ffffff; --bg-card-hover: #f5f5f5;
    --bg-elevated: #f1f1f1; --border: #d4d4d4; --border-hover: #b0b0b0;
    --text-primary: #0a0a0a; --text-secondary: #333; --text-muted: #666;
  }
  body { padding: 0; }
  .anchor-nav, .screen-only { display: none !important; }
  .section { break-inside: avoid; }
  .hero { border-top: 2px solid #000; padding-top: 16px; }
}"##;

pub const REPORT_CSP: &str = "default-src 'none'; style-src 'unsafe-inline'; script-src 'none'; img-src 'none'; font-src 'none'; connect-src 'none'; media-src 'none'; object-src 'none'; frame-src 'none'; child-src 'none'; worker-src 'none'; manifest-src 'none'; base-uri 'none'; form-action 'none'; navigate-to 'none'";

pub const REPORT_HEAD: &str = r##"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta http-equiv="Content-Security-Policy" content="PULSE_REPORT_CSP">
<title>Pulse Analytics Report</title>
<style>
PULSE_REPORT_CSS
</style>
</head>
<body>
<div class="report-shell">"##;

pub const REPORT_TAIL: &str = "</body></html>";

pub fn report_head() -> String {
    REPORT_HEAD
        .replace("PULSE_REPORT_CSP", REPORT_CSP)
        .replace("PULSE_REPORT_CSS", REPORT_CSS)
}
