import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "bun:test";
import { fireEvent, render, within } from "@testing-library/svelte";

// Source contracts ignore formatting, but preserve every selector and declaration.
function expectSource(source: string) {
    const normalize = (text: string) =>
        text.replace(/\s+/g, "").replace(/;}/g, "}");
    return {
        toContain: (text: string) =>
            expect(normalize(source)).toContain(normalize(text)),
        not: {
            toContain: (text: string) =>
                expect(normalize(source)).not.toContain(normalize(text)),
        },
    };
}

describe("Signal Ledger design system", () => {
    it("fits allowance and session collections to their actual content", () => {
        const dashboard = readFileSync(
            resolve(
                import.meta.dir,
                "../../frontend/src/views/Dashboard.svelte",
            ),
            "utf8",
        );
        const allowances = readFileSync(
            resolve(
                import.meta.dir,
                "../../frontend/src/components/AllowanceRail.svelte",
            ),
            "utf8",
        );
        expectSource(dashboard).not.toContain(
            "max-height:calc(100dvh - 160px)",
        );
        expectSource(dashboard).toContain(
            ".instance-grid { display: flex; flex-wrap: wrap; }",
        );
        expectSource(allowances).toContain("height: auto");
        expectSource(allowances).toContain(
            "repeat(auto-fit, minmax(min(250px, 100%), 1fr))",
        );
    });

    it("uses a neutral theme-aware active tab and shared centered mobile headings", () => {
        const nav = readFileSync(
            resolve(
                import.meta.dir,
                "../../frontend/src/components/TopBar.svelte",
            ),
            "utf8",
        );
        const css = readFileSync(
            resolve(import.meta.dir, "../../frontend/src/styles/global.css"),
            "utf8",
        );
        expectSource(nav).not.toContain("button.active::after");
        expectSource(nav).toContain(
            "color: var(--text-primary); background: var(--surface-raised); border-color: var(--text-muted)",
        );
        expectSource(css).toContain(
            ".main-content .app-view > .view-header > :first-child",
        );
    });

    it("owns the dark neutral palette and shared operator-console patterns centrally", () => {
        const css = readFileSync(
            resolve(import.meta.dir, "../../frontend/src/styles/global.css"),
            "utf8",
        );

        expectSource(css).toContain("--bg-primary: #050505");
        expectSource(css).toContain("--surface-panel: #070707");
        expectSource(css).toContain("--accent: #ffffff");
        expectSource(css).toContain("--success: #22c55e");
        expectSource(css).toContain("--warning: #fbbf24");
        expectSource(css).toContain("--danger: #ef4444");
        expectSource(css).not.toContain("--accent: #f5ad19");
        expectSource(css).toContain(".metric-strip");
        expectSource(css).toContain(".view-kicker");
        expectSource(css).toContain(".state-panel");
        expectSource(css).toContain(".surface-matte");
    });

    it("keeps provider identity scoped and uses blue for Codex", () => {
        const provider = readFileSync(
            resolve(import.meta.dir, "../../frontend/src/lib/provider.ts"),
            "utf8",
        );
        expectSource(provider).toContain("codex: {");
        expectSource(provider).toContain('accent: "#3b82f6"');
    });

    it("gives every primary view one concise header and one centralized matte surface language", () => {
        const views = (name: string) =>
            readFileSync(
                resolve(
                    import.meta.dir,
                    `../../frontend/src/views/${name}.svelte`,
                ),
                "utf8",
            );
        const sessions = views("Sessions");
        const costs = views("Costs");
        const reports = views("Reports");
        const discord = views("Discord");
        const settings = views("Settings");

        expectSource(sessions).not.toContain('class="view-kicker"');
        expectSource(sessions).toContain(
            '<div class="stats-row metric-strip">',
        );
        expectSource(sessions).not.toContain('<div class="empty-icon">✳</div>');
        expectSource(sessions).not.toContain("var(--panel-sheen)");
        expectSource(costs).not.toContain('class="view-kicker"');
        expectSource(reports).not.toContain('class="view-kicker"');
        expectSource(reports).toContain("{windowLabel} window");
        expectSource(discord).not.toContain('class="view-kicker"');
        expectSource(discord).toContain('<h2 class="view-title">Discord</h2>');
        expectSource(settings).not.toContain('class="view-kicker"');
        expectSource(settings).toContain(
            '<span class="version-chip">{$health?.version ? `v${$health.version}` : "Version unavailable"}</span>',
        );
        expectSource(settings).not.toContain("<DataSourceInspector");
        expect(
            readFileSync(
                resolve(
                    import.meta.dir,
                    "../../frontend/src/views/Dashboard.svelte",
                ),
                "utf8",
            ),
        ).not.toContain("<DataSourceInspector");
    });

    it("shows labeled navigation at desktop widths instead of an icon-only mystery rail", async () => {
        const Sidebar = (await import("@/components/Sidebar.svelte")).default;
        const { container } = render(Sidebar);

        const labels = Array.from(container.querySelectorAll(".nav-label")).map(
            (node) => node.textContent?.trim(),
        );
        expect(labels).toEqual([
            "Dashboard",
            "Sessions",
            "Costs",
            "Reports",
            "Discord",
            "Settings",
        ]);
    });

    it("keeps the seven primary routes and header actions accessible without a browser harness", async () => {
        const TopBar = (await import("@/components/TopBar.svelte")).default;
        const { currentView } = await import("@/lib/stores");
        const { get } = await import("svelte/store");
        let toggles = 0;
        const view = render(TopBar, {
            onToggleTheme: () => {
                toggles++;
            },
        });
        const nav = within(
            view.getByRole("navigation", { name: "Primary navigation" }),
        );
        for (const [name, id] of [
            ["Home", "dashboard"],
            ["Sessions", "sessions"],
            ["Costs", "costs"],
            ["Reports", "reports"],
            ["Discord", "discord"],
            ["Accounts", "accounts"],
            ["Settings", "settings"],
        ]) {
            const button = nav.getByRole("button", { name });
            await fireEvent.click(button);
            expect(get(currentView)).toBe(id);
            expect(button.getAttribute("aria-current")).toBe("page");
        }
        await fireEvent.click(
            view.getByRole("button", { name: "Toggle dark or light" }),
        );
        expect(toggles).toBe(1);
        await fireEvent.click(
            view.getByRole("button", { name: "Notifications" }),
        );
        expect(
            await view.findByRole("dialog", { name: "Notification center" }),
        ).toBeTruthy();
        expect(view.queryByRole("button", { name: "Help" })).toBeNull();
        currentView.set("dashboard");
    });
});
