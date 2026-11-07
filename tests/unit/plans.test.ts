import { describe, it, expect } from "bun:test";
import {
    planLabelForKey,
    planOptionsFor,
    verifiedPlanLabel,
} from "@/lib/plans";

describe("provider plan catalogs", () => {
    it("keeps the exact Codex catalog provider-scoped", () => {
        expect(planOptionsFor("codex")).toEqual([
            { value: "free", label: "Free" },
            { value: "go", label: "Go" },
            { value: "plus", label: "Plus" },
            { value: "business", label: "Business" },
            { value: "enterprise", label: "Enterprise" },
            { value: "edu", label: "Edu" },
            { value: "pro_5x", label: "Pro 5x" },
            { value: "pro_20x", label: "Pro 20x" },
        ]);
        expect(planLabelForKey("codex", "max_20x")).toBeNull();
    });

    it("keeps the exact Claude catalog provider-scoped", () => {
        expect(planOptionsFor("claude")).toEqual([
            { value: "free", label: "Free" },
            { value: "pro", label: "Pro" },
            { value: "team", label: "Teams" },
            { value: "enterprise", label: "Enterprise" },
            { value: "max_5x", label: "Max 5x" },
            { value: "max_20x", label: "Max 20x" },
        ]);
        expect(planLabelForKey("claude", "pro_20x")).toBeNull();
    });

    it("rejects cross-provider and ambiguous plan claims", () => {
        expect(verifiedPlanLabel("claude", "Claude Max 20x")).toBe("Max 20x");
        expect(verifiedPlanLabel("codex", "Codex Pro 20x")).toBe("Pro 20x");
        expect(verifiedPlanLabel("claude", "max_20x")).toBe("Max 20x");
        expect(verifiedPlanLabel("codex", "pro_20x")).toBe("Pro 20x");
        expect(verifiedPlanLabel("claude", "Pro 20x")).toBeNull();
        expect(verifiedPlanLabel("claude", "Max")).toBeNull();
        expect(verifiedPlanLabel("codex", "Max 20x")).toBeNull();
    });
});
