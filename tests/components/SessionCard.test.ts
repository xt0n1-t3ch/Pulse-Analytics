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
    expect(container.textContent).toContain("Session details");
  });
});
