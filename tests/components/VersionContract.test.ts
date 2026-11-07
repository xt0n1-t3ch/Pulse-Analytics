import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "bun:test";

const VERSION = "2.0.0";

describe("Pulse version contract", () => {
    const read = (path: string) =>
        readFileSync(resolve(import.meta.dir, "../..", path), "utf8");

    it("builds portable releases through the Tauri asset-embedding command", () => {
        const script = JSON.parse(read("package.json")).scripts[
            "build:portable"
        ];
        expect(script).toContain("bun run build:frontend");
        expect(script).toContain("tauri build --no-bundle");
    });

    it("keeps every product and release owner on v2.0.0", () => {
        expect(read("Cargo.toml")).toContain(`version = "${VERSION}"`);
        expect(read("src-tauri/Cargo.toml")).toContain(
            `version = "${VERSION}"`,
        );
        expect(JSON.parse(read("src-tauri/tauri.conf.json")).version).toBe(
            VERSION,
        );
        expect(JSON.parse(read("frontend/package.json")).version).toBe(VERSION);
        expect(read("bun.lock")).toContain('"pulse-frontend"');
        expect(JSON.parse(read("package.json")).version).toBe(VERSION);
        expect(
            JSON.parse(read("scripts/release-contract.json")).product.version,
        ).toBe(VERSION);
        const upstream = JSON.parse(read("src/codex/UPSTREAM.json"));
        expect(upstream.compatibility.pulse).toBe(VERSION);
        expect(upstream.canonical_release).toBe("v1.11.3");
        expect(upstream.canonical_commit).toBe(
            "77d5dd637d33f6876a63cc8760fdc0a1a86ba4f8",
        );
        expect(upstream.integration.version).toBe("2.0.1");
        expect(
            JSON.parse(read("scripts/release-contract.json")).core.version,
        ).toBe("2.0.1");
        expect(read("CHANGELOG.md")).toContain(`## [${VERSION}] - `);
    });
});
