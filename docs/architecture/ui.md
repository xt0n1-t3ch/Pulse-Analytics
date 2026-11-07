# Pulse UI design system

Pulse uses an ultra-dark operational shell with a true-black canvas. The canonical tokens and reusable layout patterns live in `frontend/src/styles/global.css`; views consume those owners rather than redefining a second palette.

## Color contract

- Dark and light themes keep navigation, surfaces, borders, controls, and ordinary copy neutral. Provider and status signals use deliberate high-contrast color without tinting the canvas.
- Semantic values may use `--success`, `--warning`, `--danger`, or `--info` when the color communicates positive, neutral/caution, negative, or informational state.
- Provider identity is scoped in `frontend/src/lib/provider.ts`: Claude is coral and Codex is blue. Provider color does not become the application accent.
- Empty data is neutral. A zero or missing reading must not look successful or dangerous unless the backend supplies that meaning.

## Corner contract

Rounded shapes use macOS Tahoe's continuous corners: `corner-shape: squircle` on every element, set once in `frontend/src/styles/global.css`, with the radius tokens `--radius-xs` to `--radius-xl` (7, 11, 15, 21 and 28 px). Components use the tokens, never loose pixel values. Circles (`50%`) and capsules (`--radius-full`) declare `corner-shape: round`, and the Discord preview keeps Discord's round corners. A nested shape takes the token one step below its container, so corners stay concentric.

WebView2 supports `corner-shape`. WebKitGTK 2.54 has it behind the `CSSCornerShapeEnabled` feature, which Pulse turns on when it creates the window (`src-tauri/src/platform.rs`). Where the engine lacks it (macOS WebKit has not been checked), corners fall back to circular arcs one step smaller (5 to 20 px), which read about as round.

The window itself stays opaque on every platform. A transparent window is unsafe on Linux: with the NVIDIA DMA-BUF workaround, WebKitGTK 2.54 paints only the regions that change and the desktop shows through the rest ([CapSoftware/Cap#2624](https://github.com/CapSoftware/Cap/issues/2624)), and without it NVIDIA crashes or ghosts ([tauri-apps/tauri#14924](https://github.com/tauri-apps/tauri/issues/14924)). Windows 11 rounds the frameless window itself because its shadow stays on (Tauri's default); Linux and macOS draw the window edge the window manager gives it.

## Information hierarchy

- Every primary view has one concise title and one control group. Kicker/description copy appears only when it adds information rather than repeating navigation.
- Repeated KPIs use the shared flat `.metric-strip`; individual `StatCard` cells do not render as unrelated gray cards.
- Loading and empty states use `.state-panel` and explain the next observable state without fake analytics or decorative glyphs.
- One fact has one visible owner. The Dashboard switcher owns live-session selection, Context owns active context-window detail, account quota owns provider usage, and the session-status rail is reserved for the selected session's live signals.
- Provider limits form a content-driven horizontal ledger above the live
  workspace. They never reserve a full-height sidebar after the reported
  windows end.
- Dark panels use `--surface-panel` and `.surface-matte`; view-local sheens or gray/navy elevated fills are not alternate themes.
- Tables and detailed surfaces keep their existing backend/API owners. The redesign changes presentation, not analytics semantics.
- Reports prompt excerpts are sensitive content. The backend normalizes and bounds each preview, while the UI keeps it out of the DOM until the user explicitly reveals that session.

## Responsive and accessibility contract

- Desktop navigation is labeled; compact widths collapse to the existing icon rail.
- Persistent controls remain keyboard reachable with visible focus.
- Tables may scroll inline rather than hiding columns or expanding the page past its minimum window.
- Discord's field grid stacks at 1180px, before the content column can clip controls; live-instance selectors collapse from four columns to two and then one.
- The shared provider strip fills the operational width as one compact ledger.
  Its health control occupies the final grid cell instead of floating over
  empty chrome. At narrow widths the provider cells scroll inline without
  changing analytics or Discord provider ownership.
- Reduced-motion preferences disable nonessential broadcast animation, and loading motion never carries information by itself.

## Loading and bundle contract

- Home remains in the entry chunk so the operational workspace paints without
  a route waterfall.
- Sessions, Costs, Reports, Discord, Accounts and Settings load on demand and
  expose a compact live region while their chunk is resolving.
- Route requests are generation-guarded: a slower prior import cannot replace
  the view selected most recently.

## Persistence and updates

- Discord settings save through the existing Tauri commands. The UI exposes `Saving changes...` and `Saved automatically`; a failed write still rolls back to backend truth.
- Update discovery runs automatically. `New Update Available` exposes **Update** only after a signed platform updater passes preflight; otherwise it offers **Open release**. A successful signed install relaunches Pulse.

## Focused validators

```bash
bun run test
cargo nextest run --workspace
bun run typecheck
```

For browser review, start `bun run dev` and inspect the real Rust-backed interface. Check light/dark themes, keyboard focus, narrow layouts and error states. Browser review does not prove native installation or Discord rendering.
