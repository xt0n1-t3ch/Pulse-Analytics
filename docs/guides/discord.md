# Discord Rich Presence assets

Discord **only reliably renders Rich Presence images that live in the app's
Developer Portal**. Plain `https://...` URLs get silently dropped by most
Discord client versions. If your Pulse install shows `Claude Code` in your
Discord status but no logo, this is the fix.

## Background

Before schema v3, the default config pointed `large_image_key` at a raw
GitHub URL that was never actually committed to `origin/main`, so Discord
resolved it to a 404 and showed nothing. Schema v3 changed the default to the
asset key `"claude-code"`, but that portal asset was uploaded as a wide image
and Discord letterboxed it inside the square activity art slot. Schema v4 uses
the square `"large"` portal asset instead.

## Upload checklist

1. Open the app in the Developer Portal:
   <https://discord.com/developers/applications/1466664856261230716/rich-presence/assets>
   (replace the id if you've configured a custom `CC_DISCORD_CLIENT_ID`).
2. Click **Add Image(s)** and upload the files listed below, using the file
   **stem** as the key (Discord lowercases it automatically).

| Asset key       | Source file                                               | Used for                      |
| --------------- | --------------------------------------------------------- | ----------------------------- |
| `large`         | square Claude mascot/logo uploaded in the default app       | `large_image` (main artwork)  |
| `claude-code`   | legacy wide mascot asset                                    | kept for existing custom use   |
| `thinking`      | 512² icon — Claude thinking                               | Small badge when reasoning    |
| `reading`       | 512² icon — file/eye                                      | Small badge when reading      |
| `editing`       | 512² icon — pencil                                        | Small badge when editing      |
| `running`       | 512² icon — terminal                                      | Small badge when running cmd  |
| `waiting`       | 512² icon — hourglass                                     | Small badge when idle-waiting |
| `idle`          | 512² icon — muted                                         | Small badge when idle         |

3. Click **Save Changes**.

### Codex apps

Codex uses two Discord applications for the selectable desktop designs. Surface detection remains separate: Pulse labels CLI, VS Code Extension, Codex App, and OpenCode from observed runtime metadata, while the desktop design chooses the application name/artwork shown for a desktop session.

| Design | Client ID | Portal | Asset key | Artwork |
| --- | --- | --- | --- | --- |
| ChatGPT App | `1470480085453770854` | <https://discord.com/developers/applications/1470480085453770854/rich-presence/assets> | `codex-logo` | black ChatGPT knot used by Codex CLI / VS Code Extension |
| Codex App | `1478395304624652345` | <https://discord.com/developers/applications/1478395304624652345/rich-presence/assets> | `codex-app` | blue Codex App artwork |

Reasoning, Standard/Fast speed, and surface are resolved independently from local Codex session/config state. The Discord application title itself must be `ChatGPT App` or `Codex App`; it is not reconstructed as a details line. See [plan-detection.md](plans.md).

## In-app Live Preview art

The Pulse **Discord** view bundles the real Rich Presence artwork locally
(`frontend/src/assets/rp/`, mapped by provider/surface in
`frontend/src/lib/rpArt.ts`) so the Live Preview is faithful **regardless of the
Developer Portal**. This is preview-only; it does not change what Discord
broadcasts, which still depends on the uploads above.

## Fallback tiers (no manual upload required)

If the asset key isn't in the portal, Pulse falls back through three tiers
automatically (`src/discord.rs::resolve_image_ref`):

1. **Key in portal** — the reliable path. Use asset keys uploaded above.
2. **`mp:` prefix** — pass through any pre-computed Discord media reference.
3. **`https://` URL** — wrapped as `mp:external/https/<host>/<path>`, relying
   on Discord's Media Proxy. Works sometimes, not guaranteed.
4. **Raw key** — passed straight to Discord; if the key doesn't exist, Discord
   silently drops the image.

## Diagnosing a missing logo

From the Pulse GUI, open **Settings** → the Discord diagnostic panel shows:
- Which resolution tier is currently active
- Known asset keys fetched from the portal (refreshes every 5 min)
- Suggested fix if the active key isn't in the portal

From the CLI:
```bash
cc-discord-presence doctor
```
Look at `discord_client_id` and the active session's resolved `large_image`.

## Customising assets per user

Users can override any activity key via the config file
(`~/.pulse-analytics/claude/discord-presence-config.json`):

```jsonc
{
  "display": {
    "large_image_key": "my-custom-logo",
    "activity_small_image_keys": {
      "thinking": "my-thinking-icon",
      "editing": "my-editing-icon"
    }
  }
}
```

Any key you reference must exist in the Developer Portal for the
configured `discord_client_id`.

## OpenCode in Pulse

OpenCode uses Application ID `1545590419763761303` and image key `opencode-v2`. The supplied PNG and its provenance live in `assets/branding/`; the preview uses the same image. CLI, Desktop and OpenChamber share one Discord identity. See [OpenCode and Astra](opencode.md) for data sources and attribution limits.

## Command Code

CLI and Desktop both use application `1551026507806281829`, the name **Command Code**, and asset key `commandcode`. The public inventory and a local diagnostic acknowledgement resolved asset `1551028194629521428`. See [Command Code](command-code.md) for artwork provenance and acceptance limits.

Codex publishes an enabled known subtotal as currency only. Pulse retains coverage and monetary provenance. Unknown cost remains absent. The shared compositor in `codex-presence-core` 2.0.1 reserves room for an enabled cost without changing field privacy settings.

### Command Code idle presence

With Command Code selected and Rich Presence enabled, Pulse publishes its own name and artwork even when no session is active. The status is Idle, without historical session fields or an invented session timer. Disabling the switch clears presence. A disconnected Discord client is shown as waiting, not as a successful idle broadcast.
