//! Usage telemetry from `telemetry/usage.sqlite` through `Collector::poll`.
//! Expected values are worked out from the rows each test inserts: Claude Opus
//! 5.5 costs $4 input, $20 output, $5 cache write and $0.20 cache read per
//! million tokens, and the median and speed follow from the stream timings.

use super::reader_tests::{DEEPSEEK, OPUS, close, done, world};
use super::testkit::{
    self, Msg, Row, create_usage, gzip, insert_session, insert_usage, message_line,
    write_transcript,
};

const DAY_MS: i64 = 86_400_000;

#[test]
fn telemetry_covers_the_session_and_its_subagents_and_splits_helper_requests() {
    let w = world();
    let now = w.now;
    insert_session(&w.index, &Row::root("root", now - 3_600_000, now));
    insert_session(
        &w.index,
        &Row::child("child1", "root", now - 1_800_000, now - 1_000),
    );
    insert_session(
        &w.index,
        &Row::child("child2", "root", now - 900_000, now - 500),
    );
    write_transcript(
        w.root(),
        "-work-pulse/root",
        &done(&w, "m1", 5_000, (100, 10, 0, 0)),
    );

    let usage = create_usage(w.root());
    let rows = [
        // Streamed requests: TTFT 1000, 3000 and 2000 ms; 700 output tokens over 5 s.
        testkit::Usage {
            ttft_and_stream: Some((1_000, 2_000)),
            input: 2_000,
            output: 400,
            ..testkit::Usage::new("root", "main_turn", OPUS)
        },
        testkit::Usage {
            ttft_and_stream: Some((3_000, 2_000)),
            input: 1_500,
            output: 200,
            ..testkit::Usage::new("root", "main_turn", OPUS)
        },
        testkit::Usage {
            agent: Some("orion-Explore"),
            ttft_and_stream: Some((2_000, 1_000)),
            input: 500,
            output: 100,
            ..testkit::Usage::new("child1", "subagent", OPUS)
        },
        testkit::Usage {
            agent: Some("orion-Explore"),
            input: 1_500,
            output: 50,
            cache_read: 1_000,
            ..testkit::Usage::new("child1", "subagent", OPUS)
        },
        testkit::Usage {
            agent: Some("orion-general-purpose"),
            input: 300,
            output: 30,
            ..testkit::Usage::new("child2", "subagent", DEEPSEEK)
        },
        // Helper requests: one priced, one on an unpriced model, one still running.
        testkit::Usage {
            input: 1_000,
            output: 10,
            cache_read: 400,
            cache_write: 100,
            ..testkit::Usage::new("root", "composer_prediction", OPUS)
        },
        testkit::Usage {
            input: 200,
            output: 20,
            ..testkit::Usage::new("root", "session_title", DEEPSEEK)
        },
        testkit::Usage {
            status: "running",
            input: 5_000,
            ..testkit::Usage::new("root", "compact", OPUS)
        },
        // Another session: must not leak into this one.
        testkit::Usage {
            ttft_and_stream: Some((99_000, 1)),
            input: 9,
            output: 9,
            ..testkit::Usage::new("other", "main_turn", OPUS)
        },
    ];
    for row in &rows {
        insert_usage(&usage, row);
    }
    drop(usage);

    let live = w.live();
    close(live.ttft_ms_median.unwrap(), 2_000.0);
    close(live.output_tokens_per_sec.unwrap(), 140.0);
    let helper = live.helper_requests.clone().unwrap();
    assert_eq!(
        (helper.count, helper.input_tokens, helper.output_tokens),
        (2, 1_200, 30)
    );
    // Only the Opus request is priced: 500 fresh input, 10 output, 100 write, 400 read.
    close(helper.cost_usd.unwrap(), 0.002 + 0.0002 + 0.0005 + 0.00008);
    let agents = live.subagents_by_agent.clone().unwrap();
    assert_eq!(agents.len(), 2);
    assert_eq!(agents[0].agent, "orion-Explore", "most expensive first");
    assert_eq!(
        (
            agents[0].requests,
            agents[0].input_tokens,
            agents[0].output_tokens
        ),
        (2, 2_000, 150)
    );
    // 500 fresh in + 100 out, then 500 fresh in + 50 out + 1000 cache read.
    close(agents[0].cost_usd.unwrap(), 0.004 + 0.0032);
    assert_eq!(agents[1].agent, "orion-general-purpose");
    assert_eq!(agents[1].requests, 1);
    assert_eq!(agents[1].cost_usd, None, "an unpriced model has no cost");
    // The transcript still decides the token totals.
    assert_eq!(live.usage.input, 100);
}

#[test]
fn a_cold_gzip_session_past_the_usage_retention_keeps_its_totals_but_not_its_sums() {
    let w = world();
    let created = w.now - 40 * DAY_MS;
    insert_session(&w.index, &Row::root("cold", created, created));
    let text = message_line(1, &Msg::new("m1").created(created).tokens(300, 30, 0, 0));
    let plain = write_transcript(w.root(), "-work-pulse/cold", "");
    std::fs::remove_file(&plain).unwrap();
    gzip(&plain.with_file_name("cold.jsonl.gz"), &text);
    // Orion keeps 30 days: only a late request of this session is still there.
    let usage = create_usage(w.root());
    insert_usage(
        &usage,
        &testkit::Usage {
            started_at: w.now - DAY_MS,
            ttft_and_stream: Some((500, 1_000)),
            input: 10,
            output: 10,
            ..testkit::Usage::new("cold", "main_turn", OPUS)
        },
    );
    drop(usage);

    let batch = w.poll();
    assert!(batch.diagnostics.is_empty(), "{:?}", batch.diagnostics);
    assert!(batch.live.is_empty(), "cold sessions are history, not live");
    let cold = &batch.changed[0];
    assert_eq!(
        (cold.usage.input, cold.usage.output, cold.messages),
        (300, 30, 1)
    );
    close(cold.known_cost.unwrap(), 0.0012 + 0.0006);
    assert_eq!(
        cold.model_label(),
        "Claude Opus 5.5",
        "model from the transcript"
    );
    assert_eq!(
        cold.context_used,
        Some(330),
        "latest completed request, no Orion entry"
    );
    assert_eq!(cold.cache_hit_rate, None);
    // Averages survive a partial window; additive sums would be too low.
    close(cold.ttft_ms_median.unwrap(), 500.0);
    assert_eq!(cold.helper_requests, None);
    assert_eq!(cold.subagents_by_agent, None);
}

#[test]
fn missing_usage_rows_leave_every_telemetry_field_unavailable_instead_of_zero() {
    let w = world();
    insert_session(&w.index, &Row::root("root", w.now - 60_000, w.now));
    write_transcript(
        w.root(),
        "-work-pulse/root",
        &done(&w, "m1", 1_000, (100, 10, 0, 0)),
    );

    // A telemetry database that only knows another session.
    let usage = create_usage(w.root());
    insert_usage(&usage, &testkit::Usage::new("other", "main_turn", OPUS));
    drop(usage);
    let session = w.live();
    assert_eq!(
        session.usage.input, 100,
        "transcripts stay the source of totals"
    );
    for unavailable in [
        session.ttft_ms_median.is_none(),
        session.output_tokens_per_sec.is_none(),
        session.helper_requests.is_none(),
        session.subagents_by_agent.is_none(),
    ] {
        assert!(unavailable);
    }

    // No telemetry database at all.
    std::fs::remove_file(super::layout::usage_path(w.root())).unwrap();
    let session = w.live();
    assert!(session.helper_requests.is_none() && session.ttft_ms_median.is_none());
}

#[test]
fn a_covered_session_without_helper_rows_has_zero_helper_requests_and_no_cost() {
    let w = world();
    insert_session(&w.index, &Row::root("root", w.now - 60_000, w.now));
    write_transcript(
        w.root(),
        "-work-pulse/root",
        &done(&w, "m1", 1_000, (100, 10, 0, 0)),
    );
    let usage = create_usage(w.root());
    insert_usage(&usage, &testkit::Usage::new("root", "main_turn", OPUS));
    drop(usage);
    let session = w.live();
    let helper = session.helper_requests.unwrap();
    assert_eq!((helper.count, helper.cost_usd), (0, None));
    assert_eq!(session.subagents_by_agent, Some(Vec::new()));
}
