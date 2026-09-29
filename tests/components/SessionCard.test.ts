import { describe, it, expect } from "vitest";
import { fireEvent, render } from "@testing-library/svelte";
import SessionCard from "@/components/SessionCard.svelte";
import type { SessionInfo } from "@/lib/api";

function makeSession(overrides: Partial<SessionInfo> = {}): SessionInfo {
  return {
    session_id: "s1",
    session_name: null,
    project: "pulse",
    model: "Claude Opus 4.8",
    model_id: "claude-opus-4-8",
    provider: "claude",
    context_window: "200K",
    cost: 1.23,
    cost_available: true,
    cost_basis: "exact",
    tokens: 1000,
    input_tokens: 500,
    output_tokens: 300,
    cache_write_tokens: 100,
    cache_read_tokens: 100,
    branch: null,
    activity: "Idle",
    activity_target: null,
    effort: "High",
    effort_explicit: true,
    is_idle: false,
    started_at: null,
    duration_secs: 60,
    has_thinking: false,
    workflow_label: null,
    subagent_count: 0,
    subagents: [],
    tokens_per_sec: 0,
    input_cost: 0,
    output_cost: 0,
    cache_write_cost: 0,
    cache_read_cost: 0,
    speed: "standard",
    fast: false,
    service_tier: null,
    app_name: null,
    has_inflated_tokenizer: false,
    ...overrides,
  };
}

describe("SessionCard", () => {
  it("shows the fast badge when fast is true", () => {
    const { getByText } = render(SessionCard, {
      props: { session: makeSession({ fast: true }) },
    });
    expect(getByText(/Fast/)).toBeTruthy();
  });

  it("omits the fast badge when fast is false", () => {
    const { queryByText } = render(SessionCard, {
      props: { session: makeSession({ fast: false }) },
    });
    expect(queryByText(/⚡ Fast/)).toBeNull();
  });

  it("renders singular and plural subagent badges cleanly", async () => {
    const { getByText, queryByText, rerender } = render(SessionCard, {
      props: { session: makeSession({ subagent_count: 1 }) },
    });

    expect(getByText("1 agent")).toBeTruthy();
    expect(queryByText("1 agents")).toBeNull();

    await rerender({
      session: makeSession({ subagent_count: 2 }),
    });
    expect(getByText("2 agents")).toBeTruthy();
  });

  it("shows the inflated-tokenizer marker for opus 4.7+", () => {
    const { getByTitle } = render(SessionCard, {
      props: {
        session: makeSession({ model_id: "claude-opus-4-7", has_inflated_tokenizer: true }),
      },
    });
    expect(getByTitle(/Inflated tokenizer/i)).toBeTruthy();
  });

  it("omits the inflated-tokenizer marker for opus 4.6", () => {
    const { queryByTitle } = render(SessionCard, {
      props: {
        session: makeSession({ model_id: "claude-opus-4-6", has_inflated_tokenizer: false }),
      },
    });
    expect(queryByTitle(/Inflated tokenizer/i)).toBeNull();
  });

  it("shows the inflated-tokenizer marker for Sonnet 5 sourced from the backend flag, not a local model_id regex", () => {
    const { getByTitle } = render(SessionCard, {
      props: {
        session: makeSession({
          model: "Claude Sonnet 5",
          model_id: "claude-sonnet-5",
          has_inflated_tokenizer: true,
        }),
      },
    });
    expect(getByTitle(/Inflated tokenizer/i)).toBeTruthy();
  });

  it("omits the inflated-tokenizer marker for the 5.5 generation (Opus 5.5 and Sonnet 5.5)", async () => {
    // Product decision: no 5.5-generation model carries the marker. The backend
    // reports false for both, and the card must render the model without it.
    const { getByText, queryByTitle, rerender } = render(SessionCard, {
      props: {
        session: makeSession({
          model: "Claude Sonnet 5.5",
          model_id: "claude-sonnet-5-5",
          context_window: "1M",
          has_inflated_tokenizer: false,
        }),
      },
    });
    expect(getByText(/Claude Sonnet 5\.5/)).toBeTruthy();
    expect(queryByTitle(/Inflated tokenizer/i)).toBeNull();

    await rerender({
      session: makeSession({
        model: "Claude Opus 5.5",
        model_id: "claude-opus-5-5",
        context_window: "1M",
        has_inflated_tokenizer: false,
      }),
    });
    expect(getByText(/Claude Opus 5\.5/)).toBeTruthy();
    expect(queryByTitle(/Inflated tokenizer/i)).toBeNull();
  });

  it("renders the Opus 4.8 model display name", () => {
    const { getByText } = render(SessionCard, {
      props: { session: makeSession({ model: "Claude Opus 4.8" }) },
    });
    expect(getByText(/Claude Opus 4\.8/)).toBeTruthy();
  });

  it("renders the Opus 5 model display name", () => {
    const { getByText } = render(SessionCard, {
      props: { session: makeSession({ model: "Claude Opus 5", model_id: "claude-opus-5" }) },
    });
    expect(getByText(/Claude Opus 5/)).toBeTruthy();
  });

  it("omits the inflated-tokenizer marker for Opus 5", () => {
    // Anthropic publishes no per-model tokenizer figure for Opus 5, so the
    // backend reports false and the card must stay clean.
    const { queryByTitle } = render(SessionCard, {
      props: {
        session: makeSession({
          model: "Claude Opus 5",
          model_id: "claude-opus-5",
          has_inflated_tokenizer: false,
        }),
      },
    });
    expect(queryByTitle(/Inflated tokenizer/i)).toBeNull();
  });

  it("renders Fable and Mythos model badges without inflated-tokenizer warnings", async () => {
    const { getByText, queryByTitle, rerender } = render(SessionCard, {
      props: {
        session: makeSession({
          model: "Claude Fable 5",
          model_id: "claude-fable-5",
          context_window: "1M",
        }),
      },
    });
    expect(getByText(/Claude Fable 5/).classList.contains("mythos-class")).toBe(true);
    expect(queryByTitle(/Inflated tokenizer/i)).toBeNull();

    await rerender({
      session: makeSession({
        model: "Claude Mythos 5",
        model_id: "claude-mythos-5",
        context_window: "1M",
      }),
    });
    expect(getByText(/Claude Mythos 5/).classList.contains("mythos-class")).toBe(true);
    expect(queryByTitle(/Inflated tokenizer/i)).toBeNull();
  });

  it("fails missing pricing proof closed in both summary and expanded details", async () => {
    const { container, getByText } = render(SessionCard, {
      props: {
        session: makeSession({ cost_available: undefined, cost_basis: undefined }),
      },
    });

    expect(container.textContent).not.toContain("$1.23");
    await fireEvent.click(container.querySelector(".session-card")!);
    expect(container.textContent).not.toContain("Reported value");
    expect(container.textContent).not.toContain("Session cost");
    expect(container.textContent).toContain("Session details");
  });

  describe("Orion App sessions", () => {
    // Claude Opus 5.5 over 1M each of input, output, cache write and cache read.
    function orion(overrides: Partial<SessionInfo> = {}): SessionInfo {
      return makeSession({
        provider: "orion",
        app_name: "Orion App",
        model: "Claude Opus 5.5 · Medium",
        model_id: "anthropic/claude-opus-5-5",
        context_window: "1M",
        cost: 29.2,
        cost_available: true,
        cost_basis: "estimated",
        cost_source: "api_equivalent",
        tokens: 4_000_000,
        input_tokens: 3_000_000,
        output_tokens: 1_000_000,
        cache_write_tokens: 1_000_000,
        cache_read_tokens: 1_000_000,
        input_cost: 4,
        output_cost: 20,
        cache_write_cost: 5,
        cache_read_cost: 0.2,
        tokens_per_sec: 145,
        ...overrides,
      });
    }

    async function expanded(session: SessionInfo) {
      const view = render(SessionCard, { props: { session } });
      await fireEvent.click(view.container.querySelector(".session-card")!);
      return view;
    }

    function costGrid(container: HTMLElement): Record<string, string> {
      const grid = container.querySelector(".cost-grid");
      if (!grid) return {};
      const cells = [...grid.children].map((cell) => cell.textContent?.trim() ?? "");
      const values: Record<string, string> = {};
      for (let index = 0; index + 1 < cells.length; index += 2) values[cells[index]] = cells[index + 1];
      return values;
    }

    it("shows the same per-category cost grid a Claude session shows", async () => {
      const { container } = await expanded(orion());
      expect(costGrid(container)).toEqual({
        Input: "$4.00",
        Output: "$20.00",
        "Cache Write": "$5.00",
        "Cache Read": "$0.20",
      });
      expect(container.querySelector(".stat.cost")?.textContent).toBe("$29.20");
      expect(container.querySelector(".detail-title")?.textContent).toBe("Token Breakdown");
      expect(container.textContent).toContain("Session cost");
    });

    it("states the calculation honestly without Orion-specific billing copy", async () => {
      const { container } = await expanded(orion());
      expect(container.textContent).toContain("Calculated from token counts at public API rates. Not a provider invoice.");
      expect(container.textContent).not.toContain("does not record a billed amount");
      expect(container.textContent).not.toContain("API-equivalent estimate:");
      expect(container.textContent).not.toContain("API-equivalent value");
    });

    it("reports the measured output speed instead of a dash", async () => {
      const { container } = await expanded(orion());
      expect(container.querySelector(".stat.tps")?.textContent).toBe("145/s");
      const labels = [...container.querySelectorAll(".perf-label")].map((label) => label.textContent);
      const values = [...container.querySelectorAll(".perf-val")].map((value) => value.textContent);
      expect(values[labels.indexOf("Output Speed")]).toBe("145/s");
    });

    it("keeps a lower bound partial and still lists the priced categories", async () => {
      const { container } = await expanded(orion({ cost_basis: "partial", cost: 29.2 }));
      expect(container.textContent).toContain("Known subtotal");
      expect(container.textContent).toContain("Partial estimate. Some usage components are not priced.");
      expect(costGrid(container).Output).toBe("$20.00");
    });

    it("hides the cost section when no model in the session has a known rate", async () => {
      const { container } = await expanded(
        orion({ cost_available: false, cost_basis: "unavailable", cost: 0, input_cost: 0, output_cost: 0, cache_write_cost: 0, cache_read_cost: 0 }),
      );
      expect(container.querySelector(".stat.cost")?.textContent).toBe("—");
      expect(container.querySelector(".cost-grid")).toBeNull();
      expect(container.textContent).not.toContain("Session cost");
      expect(container.textContent).not.toContain("$0.00");
    });

    it("words Claude and Orion calculated costs identically", async () => {
      const claude = await expanded(
        makeSession({ cost_basis: "estimated", cost_source: "api_equivalent", input_cost: 1, output_cost: 1 }),
      );
      const claudeText = claude.container.querySelector(".cost-provenance")?.textContent;
      claude.unmount();
      const { container } = await expanded(orion());
      expect(container.querySelector(".cost-provenance")?.textContent).toBe(claudeText);
      expect(claudeText).toContain("Calculated from token counts");
    });
  });
});
