import { existsSync, readFileSync, readdirSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { describe, expect, it } from "bun:test";

const root = resolve(import.meta.dir, "../..");
const read = (path: string) =>
    new TextDecoder("utf-8", { fatal: true }).decode(
        readFileSync(resolve(root, path)),
    );

function markdownFiles(directory: string): string[] {
    return readdirSync(resolve(root, directory), {
        withFileTypes: true,
    }).flatMap((entry) => {
        const path = `${directory}/${entry.name}`;
        return entry.isDirectory()
            ? markdownFiles(path)
            : entry.name.endsWith(".md")
              ? [path]
              : [];
    });
}

const documents = [
    "README.md",
    "PRODUCT.md",
    "CHANGELOG.md",
    "CONTRIBUTING.md",
    "llms.txt",
    "tests/index.md",
    ...markdownFiles("docs"),
];

describe("Public documentation contract", () => {
    it("keeps maintained documentation valid UTF-8 without Windows-1252 mojibake", () => {
        const mojibake =
            /(?:\u00c3[\u0080-\u00ff\u2010-\u203a]|\u00c2[\u00a0\u00b7]|\u00e2[\u0080\u20ac\u2020]|\ufffd)/u;
        for (const path of documents) {
            expect(read(path), path).not.toMatch(mojibake);
        }
    });

    it("keeps local Markdown and HTML links connected to existing files", () => {
        for (const path of documents) {
            const source = read(path);
            const links = [
                ...Array.from(
                    source.matchAll(/\]\(([^\s)]+)/g),
                    (match) => match[1],
                ),
                ...Array.from(
                    source.matchAll(/(?:href|src|srcset)="([^"]+)"/g),
                    (match) => match[1],
                ),
            ];
            for (const link of links) {
                if (/^[a-z][a-z\d+.-]*:/i.test(link) || link.startsWith("//"))
                    continue;
                const target = decodeURIComponent(link.split("#")[0]);
                if (!target) continue;
                const fullPath = target.startsWith("/")
                    ? resolve(root, target.slice(1))
                    : resolve(root, dirname(path), target);
                expect(existsSync(fullPath), `${path}: ${link}`).toBe(true);
            }
        }
    });

    it("names all three providers in discoverable product metadata", () => {
        const sources = [
            read("README.md"),
            read("llms.txt"),
            JSON.parse(read("package.json")).description,
            JSON.parse(read("src-tauri/tauri.conf.json")).bundle
                .longDescription,
        ];
        for (const source of sources) {
            for (const provider of ["Claude Code", "Codex", "OpenCode"]) {
                expect(source).toContain(provider);
            }
        }
        expect(read(".gitignore")).toContain("!/docs/models/claude.md");
        expect(read("README.md")).toContain("&middot;");
    });
});
