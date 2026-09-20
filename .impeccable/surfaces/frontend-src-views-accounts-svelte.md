---
version: 1
slug: "frontend-src-views-accounts-svelte"
primary_target: "frontend/src/views/Accounts.svelte"
related_targets: ["frontend/src/components/AllowanceRail.svelte"]
---

# Accounts surface

Mode: Operate. Audience: developers who use multiple coding providers and accounts. This surface implements the user-approved plan and extends Pulse's established visual system.

## Direction contract

THESIS: An account index and focused allowance detail make identity and available usage easy to compare without a wall of identical cards.

OWN-WORLD: Inherit Pulse's monochrome theme tokens, compact Inter text, thin separators, accessible controls, official provider marks, and restrained status colors.

STORY: Select an account, read its provider-native windows and balances, then connect or unlink a Pulse connection without changing coding-agent logins.

FIRST VIEWPORT: Accounts title and Add account action at the top. A grouped account rail occupies the left column. The selected identity, status, allowance meters, credits, and resets occupy the larger right column. The add flow replaces the detail panel. At narrow widths, an account selector replaces the rail.

FORM: User-approved master-detail structure from the implementation plan. No concept seed: composition and established-world constraints are already fixed.

FINISH: reviewer disposition `ship`, supplied by the completed refinement handoff. Documentation captured on September 19, 2026 in `DESIGN.md` and `.impeccable/design.json`. This is finish-review acceptance, not a commit, publication, or cross-platform release claim.


## Observed implementation

The [incumbent design reference](../../DESIGN.md) records existing shared tokens. This surface remains Operate and extends that system. No new world or global-token change is approved.

- **Layout:** Accounts uses a local maximum width of 1480 px. Its index column is `minmax(210px,280px)` beside `minmax(0,1fr)`. At 1000 px the index becomes 220 px and allowance rows become one column. At 700 px the index is hidden and a labeled account selector replaces it.
- **Hierarchy:** Local headings use 30 px / 650, 21 px / 620, and 14 px / 650. The first two become 25 px and 19 px at 700 px. Numeric allowance values use 24 px / 550 with tabular numerals. These are component rules, not additions to the global type scale.
- **Structure:** Identity and connection status precede separate allowance, balance, and reset sections. Thin borders separate sections. Allowance rows use two columns on wide screens; the only row or final odd row spans both columns.
- **States:** Selected rows use elevated neutral fill and a strong border. Unavailable rows use a dashed border and secondary text. Connected status uses success color. Missing windows, balances, and reset times retain explicit unavailable or not-reported text.
- **Controls:** The add flow replaces the detail panel. Actions remain labeled; focused controls receive a 2 px accent outline with a 3 px offset. Removing a Pulse connection uses a confirmation dialog and does not sign the coding client out. Reset tiles are read-only.
- **Related Home surface:** AllowanceRail uses flat rows in the Dashboard-owned panel, official marks, content-sized columns, and info-colored meters. It keeps provider proof, freshness, native windows, balances, and reset entitlements distinct. Accounts' neutral meters do not replace this existing Home treatment.

## Accepted evidence

Evidence root: a local review directory outside this checkout. All seven supplied screenshots were opened and visually checked during this documentation pass. Dimensions and hashes identify those exact files, not a future build. Dimensions and the native capture hash were remeasured on 2026-09-20.

| File | Observed coverage | Pixels | SHA256 |
| --- | --- | --- | --- |
| `accounts-codex-wide.png` | Dark Accounts; index, weekly allowance, credits, reset tiles | 1495 x 1272 | `77EF34816C3FC28416883EAAE2906A7BF7D8353C044A6F8EFC3538E0C036760A` |
| `accounts-codex-light.png` | Light Accounts with the same hierarchy | 1495 x 1272 | `1AEBB45C72A4B10707ED7AB6A44C66BF9F7DB87E86D26B9B2C9F6A8A8B19993D` |
| `accounts-codex-mobile.png` | Narrow selector, identity, allowance, and balance | 390 x 844 | `5FA158309E9DD0EB524FC9D0632D3359834DF26F0E3EC60F2DA6CE4C86B4A84D` |
| `accounts-codex-mobile-resets.png` | Narrow reset stack and footer | 390 x 844 | `2326DE7A58264A393DB8DEB19A6A184606F1AED4B38A3F40F0189A3D8C9C118D` |
| `accounts-command-wide.png` | Official Command Code mark, three windows, native credit units | 1495 x 1272 | `9E72BA3B502408DD37F53DDCAA3E88D318724A4A91F63C9C4AED02DD92612DBD` |
| `home-providers-wide.png` | Home provider limits and official provider marks | 1495 x 1272 | `DECA44B89613BC07EF165E2D95286F3F175D85E6FBEE1174B22C034B8B1D39C7` |
| `native-accounts.png` | Native Accounts, Claude detail, and unavailable-value text | 1280 x 860 | `570B6813CAC623710B0F80FB7BD608FB015C580C66771506329C807DBFA8D3C8` |

`native-proof.json` records `nativeIpc: true` and `proxyRequests: 0`. Its local accounts are Claude, Codex, and Command Code with connected status; OpenCode is unavailable with no windows. Claude reports `five_hour`, `weekly`, `fable`, and `extra_usage`; Codex reports only `weekly`; Command Code reports `fiveHour`, `weekly`, and `monthly`. This is the supplied native Tauri inter-process communication (IPC) capture, not a new runtime test performed by the documenter.

Native receipt SHA256: `8BFA34938ED80EA9BA032B4B717D94B5BA15DC0989E3EBCDAE565D9AC4F4F37F`.

The handoff reports seven passing Accounts tests and seven passing backend account tests. It also reports finish-review disposition `ship`. Those results are accepted handoff evidence; this documentation-only pass did not rerun tests or independently recreate the review verdict. The review directory contains screenshots, the native IPC receipt, and the Discord diagnostic receipt, but no separate test log or written reviewer report.

## Command Code raster provenance

- **Shipped file:** [commandcode.png](../../frontend/src/assets/rp/commandcode.png).
- **Original source:** [CommandCodeAI official spaced black symbol](https://raw.githubusercontent.com/CommandCodeAI/command-code/refs/heads/main/.github/commandcode/symbols/spaced-bg-black-symbol-commandcode.png).
- **Original SHA256 supplied by handoff:** `5D1BF3183D6CACC975D12F9F3043D313DCD374FF5F104CF3187718DE946297AF`.
- **Local SHA256 checked in this pass:** `5D1BF3183D6CACC975D12F9F3043D313DCD374FF5F104CF3187718DE946297AF`.
- **Treatment:** Official image used unmodified. No raster generation, recoloring, or replacement. CSS sizing and clipping do not alter source bytes.
- **Public Discord identity, verified in the supplied handoff:** application `1551026507806281829`, asset name `commandcode`, asset ID `1551028194629521428`. This pass did not repeat the remote Discord check or claim new client-render proof.

`commandcode-discord-proof.json` was added by concurrent work during documentation validation. The inspected receipt confirms application `1551026507806281829`, large-image asset `1551028194629521428`, and acknowledged diagnostic activity set and clear operations. It explicitly records `visual_rendering_verified: false`; this is IPC acknowledgment, not proof that the Discord client rendered the image. Receipt SHA256: `D38B458BAFF1A15096F28AAD94B3ADA6DC0AB311D869F527E7794C6858F6F4B2`.

Existing Claude, Codex, and OpenCode assets remain incumbent dependencies. This refinement does not generate or replace them. The receipt above covers the new Command Code raster, not a new provenance audit of every historical asset.

## Documentation and artifact limits

The evidence remains in the supplied local temporary directory. No images or receipt were copied into `assets/evidence/` or `docs/evidence/`; those writes are outside the authorized boundary. A durable repository evidence archive remains a separate delivery gap. The screenshot contents include local account information, so they are referenced without copying that identity data into this document.

The new design reference is linked from this surface brief. `docs/index.md`, `llms.txt`, and `tests/index.md` remain unchanged under the explicit write boundary; discoverability from those indexes is not completed here. No product code, configuration, test, asset, commit, or publication is part of this documentation pass.

The sidecar contains extracted shared component specimens, not a substitute for native runtime proof. Its JSON and token references are checked, but its Impeccable panel rendering is not tested here. No synthesized color ramps were added because this task records incumbent values only. Existing decorative kickers and stale raised-panel comments are not promoted into the design system or repaired.
