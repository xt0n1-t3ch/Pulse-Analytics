---
name: Pulse
description: Established monochrome desktop analytics system.
colors:
  bg-primary: "#050505"
  bg-secondary: "#070707"
  bg-card: "#070707"
  bg-card-hover: "#0b0b0b"
  bg-elevated: "#0c0c0c"
  bg-input: "#060606"
  surface-panel: "#070707"
  surface-panel-soft: "#060606"
  surface-raised: "#0b0b0b"
  divider: "#1b1b1b"
  text-primary: "#f5f5f5"
  text-secondary: "#b8b8b8"
  text-muted: "#8a8a8a"
  text-placeholder: "#666666"
  accent: "#ffffff"
  accent-hover: "#e6e6e6"
  accent-fg: "#000000"
  border: "#202022"
  border-strong: "#2b2b2e"
  border-hover: "#444449"
  success: "#22c55e"
  warning: "#fbbf24"
  danger: "#ef4444"
  info: "#7cb9e8"
  codex: "#3b82f6"
  claude: "#d97757"
  meter-track: "rgba(255, 255, 255, 0.055)"
  light-bg-primary: "#ffffff"
  light-bg-secondary: "#fafafa"
  light-bg-card: "#ffffff"
  light-bg-card-hover: "#f7f7f7"
  light-bg-elevated: "#f1f1f1"
  light-bg-input: "#ffffff"
  light-surface-panel: "#ffffff"
  light-surface-panel-soft: "#fafafa"
  light-surface-raised: "#f3f3f3"
  light-divider: "#e5e5e5"
  light-text-primary: "#0a0a0a"
  light-text-secondary: "#444444"
  light-text-muted: "#666666"
  light-text-placeholder: "#8a8a8a"
  light-accent: "#0a0a0a"
  light-accent-hover: "#1a1a1a"
  light-accent-fg: "#ffffff"
  light-border: "#e5e5e5"
  light-border-strong: "#d4d4d4"
  light-border-hover: "#a3a3a3"
  light-success: "#166534"
  light-warning: "#854d0e"
  light-danger: "#b91c1c"
  light-info: "#2563eb"
  light-codex: "#1d4ed8"
  light-claude: "#9a3412"
  light-meter-track: "rgba(10, 10, 10, 0.075)"
typography:
  body:
    fontFamily: "'Inter Variable', 'Segoe UI Variable', 'Segoe UI', -apple-system, BlinkMacSystemFont, sans-serif"
    fontSize: "13px"
    lineHeight: 1.5
  view-title:
    fontFamily: "'Inter Variable', 'Segoe UI Variable', 'Segoe UI', -apple-system, BlinkMacSystemFont, sans-serif"
    fontSize: "20px"
    fontWeight: 600
    letterSpacing: "-0.025em"
  section-heading:
    fontSize: "14px"
    fontWeight: 600
    letterSpacing: "-0.015em"
  label:
    fontSize: "10px"
    fontWeight: 600
    letterSpacing: "0.06em"
  metric:
    fontSize: "32px"
    fontWeight: 700
    lineHeight: 1.15
    letterSpacing: "-0.025em"
  mono:
    fontFamily: "'Cascadia Code', 'Cascadia Mono', Consolas, monospace"
rounded:
  xs: "4px"
  sm: "6px"
  md: "8px"
  lg: "12px"
  xl: "16px"
  full: "9999px"
spacing:
  control-gap: "6px"
  grid-gap: "16px"
  card-padding: "20px"
  page-inline: "clamp(12px, 1.15vw, 20px)"
  page-block: "clamp(12px, 1vw, 18px)"
  page-gap: "clamp(10px, 0.9vw, 14px)"
components:
  button:
    backgroundColor: "{colors.bg-elevated}"
    textColor: "{colors.text-primary}"
    rounded: "{rounded.sm}"
    padding: "6px 12px"
  button-primary:
    backgroundColor: "{colors.accent}"
    textColor: "{colors.accent-fg}"
    rounded: "{rounded.sm}"
    padding: "6px 12px"
  button-ghost:
    backgroundColor: "transparent"
    textColor: "{colors.text-secondary}"
    rounded: "{rounded.sm}"
    padding: "6px 12px"
  button-danger:
    textColor: "{colors.danger}"
    rounded: "{rounded.sm}"
    padding: "6px 12px"
  input:
    backgroundColor: "{colors.bg-input}"
    textColor: "{colors.text-primary}"
    rounded: "{rounded.sm}"
    padding: "8px 12px"
  chip:
    backgroundColor: "{colors.bg-elevated}"
    textColor: "{colors.text-secondary}"
    rounded: "{rounded.full}"
    padding: "3px 8px"
  card:
    backgroundColor: "{colors.bg-card}"
    rounded: "{rounded.lg}"
    padding: "{spacing.card-padding}"
  navigation:
    textColor: "{colors.text-secondary}"
    rounded: "{rounded.sm}"
    padding: "0 10px"
---
# Design System: Pulse

## Overview

**Creative North Star: "Signal Ledger"**

Signal Ledger is the incumbent name in the global stylesheet. Pulse uses matte neutral surfaces, compact type, and thin separators to make account and session facts readable. Provider marks and semantic colors identify sources and states without replacing the monochrome shell.

This reference captures the existing system on September 19, 2026. The completed Accounts refinement extends that system; it does not approve a new visual world or change global tokens. Component-local measurements remain local unless the shared source already defines them.

**Key Characteristics:**

- Matte dark and light surfaces with thin borders.
- Compact Inter text and tabular numeric values.
- Official provider identity assets and limited semantic color.
- Responsive layouts with explicit keyboard focus.

Source authority: [global theme and primitives](frontend/src/styles/global.css), [bundled font](frontend/src/styles/fonts.css), [TopBar](frontend/src/components/TopBar.svelte), and [StatCard](frontend/src/components/StatCard.svelte). [Accounts](frontend/src/views/Accounts.svelte) and [AllowanceRail](frontend/src/components/AllowanceRail.svelte) supply the observed extension patterns. See the [Accounts surface record](.impeccable/surfaces/frontend-src-views-accounts-svelte.md) for its direction, accepted evidence, and asset provenance. This is a reference for future UI work, not a release receipt.

## Colors

The shell uses near-black neutrals in dark mode and white neutrals in light mode. Frontmatter names without a prefix record `:root`; `light-` names record `[data-theme="light"]` overrides. Those prefixes are documentation keys, not new CSS variables. Runtime CSS remains the source of truth.

### Primary

The `accent` pair supplies high-contrast actions and shell emphasis. The light theme reverses the pair. `accent-hover` supplies the shared primary button hover state.

### Neutral

The `bg-*` and `surface-*` families separate the canvas, input fields, panels, and raised tones. The `text-*` family distinguishes primary facts, supporting copy, and muted metadata. `border`, `border-strong`, and `divider` separate regions without shadows.

### Semantic and provider colors

Success, warning, danger, and info retain their existing meanings. Codex blue and Claude coral identify providers; they are not alternate shell palettes. `--provider-accent` resolves through provider selectors and falls back to info. The existing OpenCode selector uses primary text. Accounts meters use primary text with warning for high usage; Home allowance meters use info. Neither surface establishes a new global meter color.

**The Semantic Color Rule.** Keep shell emphasis monochrome. Use provider and status tokens for their named meanings, not decorative recoloring.

No synthesized tonal ramps are introduced. The sidecar uses existing CSS variables, including their theme-specific dim variants.

## Typography

Inter Variable is the bundled sans face, with the platform fallbacks recorded in frontmatter. Cascadia Code, Cascadia Mono, and Consolas provide the existing monospace stack. There is no separate decorative display face.

The shared type scale runs through `--fs-xs`, `--fs-sm`, `--fs-base`, `--fs-md`, `--fs-lg`, `--fs-xl`, `--fs-2xl`, `--fs-3xl`, and `--fs-display`. Their current sizes are 10, 11, 13, 14, 16, 20, 24, 32, and 44 px. Below 800 px, the final three become 22, 28, and 36 px. Frontmatter records reusable roles, not every component override.

Shared view titles use the view-title role; section headings use the section-heading role. StatCard uses a local fluid metric size (22 to 30 px), not the shared metric size in every context. Accounts also uses local headings and values; its surface brief records those measurements. Small metadata in AllowanceRail is an observed local treatment, not a new body-text minimum.

**The Numeric Alignment Rule.** Use tabular numerals for comparable usage values, balances, and metrics.

## Layout

The shared app view fills the available width and has no fixed global maximum. Page padding and gaps use the fluid values in frontmatter. Spacing entries describe existing uses, not a newly imposed spacing scale.

The rendered shell uses TopBar, not the older Sidebar component. TopBar places navigation beside the brand on wide screens. It hides account context at 1080 px, moves navigation below the title bar at 800 px, and uses three navigation columns at 620 px. Shared metric strips change from four columns to two at 900 px and one at 620 px.

Accounts has its own content-width cap and master-detail breakpoints. Those are surface choices, not changes to the shared full-width contract. AllowanceRail uses content-sized columns with a minimum of 250 px when space permits. The parent Dashboard panel owns its surrounding surface.

## Elevation & Depth

Ordinary panels use neutral tones and borders. The shared `--elev-0`, `--elev-1`, and `--elev-2` hooks all resolve to `none`; panel sheen is `none` and the panel edge is transparent. Focus rings, small control effects, and Discord-preview shadows remain separate existing mechanisms. This is not a blanket prohibition on all shadows.

**The Flat Panel Rule.** Use neutral fill and borders for ordinary app panels. Do not infer floating panels from the legacy shadow token names.

Motion supports state changes: shared buttons use 150 ms color transitions and an 80 ms press transform; interactive cards use 150 ms transitions. The sidecar records the existing easing functions. The global reduced-motion query reduces animation and transition durations to 0.01 ms and limits animation to one iteration.

Existing comments describing a lit or raised panel conflict with the resolved flat tokens. Those comments are not design authority and are not repaired in this documentation pass.

## Shapes

The existing radius scale runs from small control corners to rounded panels and full pills. Use the frontmatter scale rather than adding new global values. Shared controls use the small radius, ordinary cards use the large radius, and chips use the full radius. Thin borders remain the primary region separator.

Provider images retain their aspect ratio through `object-fit: contain`. Component-local image and meter corner values do not extend the global radius scale.

## Components

### Buttons

Shared buttons are compact, bordered controls with a small radius. Primary buttons use the accent pair; ghost buttons remove the resting fill and border. Danger buttons use danger text and a dim semantic background. Hover changes fill or border, press moves the control down one pixel, and disabled controls reduce opacity. Global keyboard focus uses the provider-linked ring. Accounts has a larger local action treatment and an additional visible outline; preserve that distinction.

### Chips

Shared chips are compact uppercase state labels with full rounding. Semantic variants pair foreground and dim background tokens. Their existing text treatment does not authorize decorative headings or kickers.

### Cards / Containers

Shared cards use the card background, thin border, large radius, and card padding from frontmatter. Interactive cards change border and fill and lift one pixel. Static panels do not acquire this behavior. AllowanceRail rows remain flat inside the parent panel rather than becoming nested cards.

### Inputs / Fields

Shared fields use the input background, small radius, and bordered focus ring. Placeholder text uses the placeholder token. Accounts uses labeled connection fields and its own larger field dimensions. Do not infer global error or disabled-field styling from unobserved states.

### Navigation

TopBar navigation uses text labels. Hover adds an elevated neutral fill; the active item adds a muted border and stronger text. Keyboard focus has a visible outline. Narrow layouts preserve labels instead of adopting the legacy Sidebar's icon-only behavior.

### Usage and account patterns

Usage meters keep their value and direction in text. Identity, provider-native windows, balances, and reset entitlements remain distinct. Freshness and unavailable copy prevent missing data from looking like zero usage. Accounts reset tiles are informational, not purchase or consume-reset controls. The [surface brief](.impeccable/surfaces/frontend-src-views-accounts-svelte.md) owns this composition and its proof.

## Do's and Don'ts

### Do:

- Do use the existing CSS variables so dark and light themes change together.
- Do preserve official provider marks, aspect ratios, and source provenance.
- Do pair usage values with their units, direction, and unavailable state.
- Do retain keyboard focus, accessible labels, and reduced-motion behavior.

### Don't:

- Don't turn the Accounts master-detail composition into a requirement for every route.
- Don't substitute a generated mark or glyph for an official provider asset.
- Don't treat missing provider values as zero or make read-only reset entitlements look actionable.
- Don't copy stale visual comments or decorative kickers into new system rules.

Not canonized or repaired: the existing `view-kicker` style and decorative eyebrow uses are not reusable guidance; the Impeccable craft floor rejects that pattern. Legacy raised-panel comments do not override flat tokens. Small AllowanceRail metadata is documented as local evidence, not endorsed as a universal readable minimum. Product code remains outside this pass.
