import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";

const VERSION = "1.9.0";

describe("Pulse version contract", () => {
  const read = (path: string) => readFileSync(resolve(process.cwd(), "..", path), "utf8");

  it("builds portable releases through the Tauri asset-embedding command", () => {
    const script = JSON.parse(read("package.json")).scripts["build:portable"];
    expect(script).toContain("npm --prefix frontend run build");
    expect(script).toContain("cargo tauri build --no-bundle");
  });

  it("keeps every product and release owner on v1.9.0", () => {
    expect(read("Cargo.toml")).toContain(`version = "${VERSION}"`);
    expect(read("src-tauri/Cargo.toml")).toContain(`version = "${VERSION}"`);
    expect(JSON.parse(read("src-tauri/tauri.conf.json")).version).toBe(VERSION);
    expect(JSON.parse(read("frontend/package.json")).version).toBe(VERSION);
    expect(JSON.parse(read("frontend/package-lock.json")).version).toBe(VERSION);
    expect(JSON.parse(read("package.json")).version).toBe(VERSION);
    expect(JSON.parse(read("scripts/release-contract.json")).product.version).toBe(VERSION);
    const upstream = JSON.parse(read("src/codex/UPSTREAM.json"));
    expect(upstream.compatibility.pulse).toBe(VERSION);
    expect(upstream.canonical_release).toBe("v1.11.2");
    expect(upstream.canonical_commit).toBe("0caece60a71eb657d7b829e3b6c7c4896a0a536c");
    expect(upstream.integration.version).toBe("2.0.1");
    expect(JSON.parse(read("scripts/release-contract.json")).core.version).toBe("2.0.1");
    expect(read("CHANGELOG.md")).toContain(`## [${VERSION}] - `);
  });
});
