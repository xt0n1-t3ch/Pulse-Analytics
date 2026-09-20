import { writable, derived, get, type Writable } from "svelte/store";
import {
    getActiveProvider,
    getProviderCopy,
    setActiveProvider as persistActiveProvider,
    type ProviderCopyInfo,
} from "./api";

export type Provider = "claude" | "codex" | "opencode" | "commandcode";

export interface ProviderProfile {
    id: Provider;
    /** Short label used in compact UI (chips, toggles). */
    label: string;
    /** Full product name — used in Discord activity/subtitles. */
    productName: string;
    /** Line shown next to the Pulse wordmark. */
    tagline: string;
    /** Accent color for the provider pill/indicator (still passes through theme tokens elsewhere). */
    accent: string;
    /** Default Discord Rich Presence asset key. Backend may override. */
    defaultAssetKey: string;
    /** Whether provider exposes plan overage / extra-usage style controls. */
    supportsExtraUsage: boolean;
    /** Primary local session path shown in settings. */
    sessionsPath: string;
    /** Top-level instruction file (CLAUDE.md, AGENTS.md, …). */
    instructionFile: string;
    /** ~/.claude, ~/.codex, … */
    homeDir: string;
    /** "Fix with Claude Code", "Fix with Codex", … */
    fixLabel: string;
    /** Global state source label for the Data Sources card. */
    globalStateSource: string;
}

const BASE: Record<Provider, ProviderProfile> = {
    commandcode: {
        id: "commandcode", label: "Command Code", productName: "Command Code", tagline: "Command Code Analytics",
        accent: "var(--text-primary)", defaultAssetKey: "commandcode", supportsExtraUsage: false,
        sessionsPath: "~/.commandcode/projects/**/*.jsonl", instructionFile: "AGENTS.md",
        homeDir: "~/.commandcode", fixLabel: "Fix with Command Code", globalStateSource: "Local transcripts and account API",
    },
    opencode: {
        id: "opencode", label: "OpenCode", productName: "OpenCode", tagline: "OpenCode Analytics",
        accent: "var(--text-primary)", defaultAssetKey: "opencode-v2", supportsExtraUsage: false,
        sessionsPath: "~/.local/share/opencode/opencode*.db", instructionFile: "AGENTS.md",
        homeDir: "~/.config/opencode", fixLabel: "Fix with OpenCode", globalStateSource: "Local SQLite session metadata",
    },
    claude: {
        id: "claude",
        label: "Claude",
        productName: "Claude Code",
        tagline: "Claude Code Analytics",
        accent: "#d97757",
        defaultAssetKey: "large",
        supportsExtraUsage: true,
        sessionsPath: "~/.claude/projects/",
        instructionFile: "CLAUDE.md",
        homeDir: "~/.claude",
        fixLabel: "Fix with Claude Code",
        globalStateSource: "~/.claude + usage API",
    },
    codex: {
        id: "codex",
        label: "Codex",
        productName: "Codex",
        tagline: "Codex Analytics",
        accent: "#3b82f6",
        defaultAssetKey: "codex-logo",
        supportsExtraUsage: false,
        sessionsPath: "~/.codex/sessions/",
        instructionFile: "AGENTS.md",
        homeDir: "~/.codex",
        fixLabel: "Fix with Codex",
        globalStateSource: "~/.codex + session telemetry",
    },
};

export const PROVIDERS: Record<Provider, ProviderProfile> = { ...BASE };

/** Neutral profile used on first-ever load before the backend resolves the
 *  active provider. Avoids a brief "Claude Code Analytics" flash for Codex
 *  users who have never launched Pulse before. Existing users hit a stored
 *  value in localStorage and skip this path entirely. */
const NEUTRAL_PROFILE: ProviderProfile = {
    id: "claude",
    label: "Pulse",
    productName: "Pulse",
    tagline: "Session Analytics",
    accent: "#9aa0a6",
    defaultAssetKey: "large",
    supportsExtraUsage: false,
    sessionsPath: "—",
    instructionFile: "—",
    homeDir: "—",
    fixLabel: "Copy Fix Prompt",
    globalStateSource: "—",
};

const STORAGE_KEY = "pulse-provider";
const storage = globalThis.localStorage;
const stored = storage?.getItem(STORAGE_KEY) ?? null;
const hasStoredProvider = stored === "codex" || stored === "claude" || stored === "opencode" || stored === "commandcode";
const initialProvider: Provider = stored === "commandcode" ? "commandcode" : stored === "opencode" ? "opencode" : stored === "codex" ? "codex" : "claude";

export const provider: Writable<Provider> = writable<Provider>(initialProvider);
export const providerCopy: Writable<ProviderCopyInfo | null> = writable(null);
export const providerRevision: Writable<number> = writable(0);
/** True once we trust the active provider — either read from localStorage or
 *  confirmed by the backend. While false, `providerProfile` yields neutral
 *  branding so the UI never briefly labels Codex users as Claude. */
export const providerResolved: Writable<boolean> = writable(hasStoredProvider);

let providerInitialized = false;
provider.subscribe((p) => {
    try {
        storage?.setItem(STORAGE_KEY, p);
        if (get(providerResolved)) {
            document.documentElement.setAttribute("data-provider", p);
        }
    } catch {}
    if (providerInitialized) {
        providerResolved.set(true);
    }
});
providerInitialized = true;

if (hasStoredProvider) {
    try {
        document.documentElement.setAttribute("data-provider", initialProvider);
    } catch {}
}

export const providerProfile = derived(
    [provider, providerCopy, providerResolved],
    ([$p, $copy, $ready]) => {
        if (!$ready) return NEUTRAL_PROFILE;
        const base = PROVIDERS[$p];
        if (!$copy || $copy.provider !== $p) return base;
        return {
            ...base,
            productName: $copy.provider_label || base.productName,
            instructionFile: $copy.instruction_file || base.instructionFile,
            homeDir: $copy.home_dir || base.homeDir,
            fixLabel: $copy.fix_label || base.fixLabel,
            globalStateSource: $copy.global_state_source || base.globalStateSource,
            sessionsPath: $copy.sessions_store || base.sessionsPath,
        };
    },
);

let providerGeneration = 0;
let confirmedProvider: Provider = initialProvider;
let providerMutation: Promise<void> = Promise.resolve();

function publishProvider(p: Provider): void {
    provider.set(p);
    providerResolved.set(true);
    try {
        document.documentElement.setAttribute("data-provider", p);
    } catch {}
}

export function setProvider(p: Provider): Promise<void> {
    const generation = ++providerGeneration;
    const mutation = providerMutation.then(async () => {
        let persisted = false;
        try {
            await persistActiveProvider(p);
            persisted = true;
            confirmedProvider = p;
            const copy = await getProviderCopy();
            if (generation === providerGeneration) {
                publishProvider(p);
                providerCopy.set(copy);
                providerRevision.update((value) => value + 1);
            }
        } catch (error) {
            if (generation === providerGeneration) {
                if (persisted) {
                    publishProvider(p);
                    providerCopy.set(null);
                    providerRevision.update((value) => value + 1);
                } else {
                    publishProvider(confirmedProvider);
                }
            }
            throw error;
        }
    });
    providerMutation = mutation.catch(() => undefined);
    return mutation;
}

const bootstrapGeneration = providerGeneration;
void (async () => {
    try {
        const info = await getActiveProvider();
        if (bootstrapGeneration !== providerGeneration) return;
        const p = info.active_provider === "commandcode" ? "commandcode" : info.active_provider === "opencode" ? "opencode" : info.active_provider === "codex" ? "codex" : "claude";
        confirmedProvider = p;
        publishProvider(p);
        const copy = await getProviderCopy();
        if (bootstrapGeneration !== providerGeneration) return;
        providerCopy.set(copy);
        providerRevision.update((value) => value + 1);
    } catch {
        if (bootstrapGeneration !== providerGeneration) return;
        // Backend unreachable — promote whatever we have so the UI doesn't
        // stay stuck in neutral branding forever.
        providerResolved.set(true);
    }
})();
