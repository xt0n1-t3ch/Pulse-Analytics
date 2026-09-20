// Real Rich Presence art, bundled and mapped by provider/surface so the Live
// Preview renders the same large image Discord shows. Claude Code uses the
// official mascot; Codex uses the official Codex mark (the blue blossom with the
// terminal prompt). Small badge is null until a distinct small asset exists, so
// the card matches Discord's large-image-only rendering rather than duplicating
// the large art.
import openCode from "../assets/rp/opencode-v2.png";
import commandCode from "../assets/rp/commandcode.png";
import claudeCode from "../assets/rp/claude-code.png";
import codexApp from "../assets/rp/codex-app.png";
import chatGptApp from "../assets/rp/chatgpt-app.jpg";

export interface RpArt {
  /** Large image URL (bundled). */
  large: string;
  /** Small overlay badge URL, or null when there is no distinct small asset. */
  small: string | null;
  /** Hover text for the large image (Discord `large_text`). */
  largeText: string;
  /** Asset key Discord would request for the large image. */
  assetKey: string;
}

export function rpArtFor(
  provider: string,
  assetKey?: string | null,
  largeText?: string | null,
): RpArt {
  if (provider === "commandcode") return { large: commandCode, small: null, largeText: "Command Code", assetKey: "commandcode" };
  if (provider === "opencode") return { large: openCode, small: null, largeText: "OpenCode", assetKey: "opencode-v2" };
  if (provider === "codex") {
    const isCodexApp = assetKey === "codex-app";
    return {
      large: isCodexApp ? codexApp : chatGptApp,
      small: null,
      largeText: largeText || (isCodexApp ? "Codex App" : "ChatGPT App"),
      assetKey: assetKey || (isCodexApp ? "codex-app" : "codex-logo"),
    };
  }
  return {
    large: claudeCode,
    small: null,
    largeText: largeText || "Claude Code",
    assetKey: assetKey || "large",
  };
}
