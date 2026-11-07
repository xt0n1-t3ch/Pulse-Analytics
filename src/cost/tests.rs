//! Unit tests for Claude model classification, rates and cost arithmetic.

use super::*;

#[test]
fn cost_calculation_sonnet() {
    let cost = calculate_cost("claude-sonnet-4-20250514", 1_000_000, 500_000, 0, 0);
    assert!((cost - 10.5).abs() < 0.001);
}

#[test]
fn cost_calculation_opus_new() {
    let cost = calculate_cost("claude-opus-4-5-20251101", 1_000_000, 100_000, 0, 0);
    let expected = 5.0 + 2.5;
    assert!((cost - expected).abs() < 0.001);
}

#[test]
fn cost_calculation_opus_legacy() {
    let cost = calculate_cost("claude-opus-4-1-20250414", 1_000_000, 100_000, 0, 0);
    let expected = 15.0 + 7.5;
    assert!((cost - expected).abs() < 0.001);
}

#[test]
fn cost_with_cache_tokens_new_opus() {
    let cost = calculate_cost("claude-opus-4-6-20260213", 3, 9, 12_487, 22_766);
    let expected = (3.0 * 5.0 / 1_000_000.0)
        + (9.0 * 25.0 / 1_000_000.0)
        + (12_487.0 * 6.25 / 1_000_000.0)
        + (22_766.0 * 0.50 / 1_000_000.0);
    assert!((cost - expected).abs() < 0.001);
}

#[test]
fn cost_with_cache_tokens_legacy_opus() {
    let cost = calculate_cost(
        "claude-opus-4-20250414",
        1_000_000,
        100_000,
        50_000,
        200_000,
    );
    let expected = 15.0 + 7.5 + (50_000.0 * 18.75 / 1_000_000.0) + (200_000.0 * 1.50 / 1_000_000.0);
    assert!((cost - expected).abs() < 0.001);
}

#[test]
fn unknown_model_uses_sonnet_pricing() {
    let cost = calculate_cost("claude-unknown-model", 1_000_000, 0, 0, 0);
    assert!((cost - 3.0).abs() < 0.001);
}

#[test]
fn pricing_opus_generation_detection() {
    let p1 = model_pricing("claude-opus-4-5-20251101");
    assert!((p1.input_per_million - 5.0).abs() < 0.001);
    assert!((p1.output_per_million - 25.0).abs() < 0.001);

    let p2 = model_pricing("claude-opus-4-6-20260213");
    assert!((p2.input_per_million - 5.0).abs() < 0.001);

    let p3 = model_pricing("claude-opus-4-6");
    assert!((p3.input_per_million - 5.0).abs() < 0.001);

    let p4 = model_pricing("claude-opus-4-1-20250414");
    assert!((p4.input_per_million - 15.0).abs() < 0.001);
    assert!((p4.output_per_million - 75.0).abs() < 0.001);

    let p5 = model_pricing("claude-opus-4-20250414");
    assert!((p5.input_per_million - 15.0).abs() < 0.001);

    let p6 = model_pricing("opus");
    assert!((p6.input_per_million - 15.0).abs() < 0.001);
}

#[test]
fn pricing_haiku_generations() {
    let p1 = model_pricing("claude-haiku-4-5-20251001");
    assert!((p1.input_per_million - 1.0).abs() < 0.001);

    let p2 = model_pricing("claude-haiku-3-5-20241022");
    assert!((p2.input_per_million - 0.80).abs() < 0.001);
    assert!((p2.output_per_million - 4.0).abs() < 0.001);

    let p3 = model_pricing("claude-haiku-3-20240307");
    assert!((p3.input_per_million - 0.25).abs() < 0.001);
    assert!((p3.output_per_million - 1.25).abs() < 0.001);

    let p4 = model_pricing("haiku");
    assert!((p4.input_per_million - 1.0).abs() < 0.001);
}

#[test]
fn display_names_dated() {
    assert_eq!(
        model_display_name("claude-opus-4-6-20260213"),
        "Claude Opus 4.6"
    );
    assert_eq!(
        model_display_name("claude-opus-4-5-20251101"),
        "Claude Opus 4.5"
    );
    assert_eq!(
        model_display_name("claude-sonnet-4-5-20250929"),
        "Claude Sonnet 4.5"
    );
    assert_eq!(
        model_display_name("claude-sonnet-4-20250514"),
        "Claude Sonnet 4"
    );
    assert_eq!(
        model_display_name("claude-haiku-4-5-20251001"),
        "Claude Haiku 4.5"
    );
}

#[test]
fn display_names_short() {
    assert_eq!(model_display_name("claude-opus-4-6"), "Claude Opus 4.6");
    assert_eq!(model_display_name("claude-opus-4-5"), "Claude Opus 4.5");
    assert_eq!(model_display_name("claude-sonnet-4-5"), "Claude Sonnet 4.5");
    assert_eq!(model_display_name("claude-sonnet-4"), "Claude Sonnet 4");
    assert_eq!(model_display_name("claude-haiku-4-5"), "Claude Haiku 4.5");
}

#[test]
fn cost_1m_ga_no_surcharge() {
    let cost = calculate_cost_with_context("claude-sonnet-4-6", 50_000, 5_000, 100_000, 100_000);
    let standard = calculate_cost("claude-sonnet-4-6", 50_000, 5_000, 100_000, 100_000);
    assert!((cost - standard).abs() < 0.0001);
}

#[test]
fn cost_1m_beta_still_has_surcharge() {
    let cost = calculate_cost_with_context("claude-sonnet-4-5", 50_000, 5_000, 100_000, 100_000);
    let standard = calculate_cost("claude-sonnet-4-5", 50_000, 5_000, 100_000, 100_000);
    assert!(cost > standard);
}

#[test]
fn cost_1m_below_threshold_uses_standard() {
    let cost_std = calculate_cost("claude-sonnet-4-6", 50_000, 5_000, 25_000, 25_000);
    let cost_ctx = calculate_cost_with_context("claude-sonnet-4-6", 50_000, 5_000, 25_000, 25_000);
    assert!((cost_std - cost_ctx).abs() < 0.0001);
}

#[test]
fn cost_1m_haiku_no_surcharge() {
    let cost_std = calculate_cost("claude-haiku-4-5", 50_000, 5_000, 100_000, 100_000);
    let cost_ctx = calculate_cost_with_context("claude-haiku-4-5", 50_000, 5_000, 100_000, 100_000);
    assert!((cost_std - cost_ctx).abs() < 0.0001);
}

#[test]
fn context_suffix_stripped_for_display() {
    assert_eq!(model_display_name("claude-opus-4-6[1m]"), "Claude Opus 4.6");
    assert_eq!(
        model_display_name("claude-sonnet-4-6[1m]"),
        "Claude Sonnet 4.6"
    );
}

#[test]
fn context_suffix_stripped_for_pricing() {
    let p = model_pricing("claude-opus-4-6[1m]");
    assert!((p.input_per_million - 5.0).abs() < 0.001);
    assert!((p.output_per_million - 25.0).abs() < 0.001);
}

#[test]
fn sonnet_5_is_ga_1m_context_not_beta_surcharge() {
    assert!(is_ga_1m_context("claude-sonnet-5"));
    assert!(is_ga_1m_context("claude-sonnet-5-20260625"));
    assert!(is_ga_1m_context("claude-sonnet-5[1m]"));
}

#[test]
fn sonnet_5_supports_1m_context() {
    assert!(supports_1m_context("claude-sonnet-5"));
    assert!(supports_1m_context("claude-sonnet-5-20260625"));
    assert!(supports_1m_context("claude-sonnet-5[1m]"));
}

#[test]
fn sonnet_5_no_1m_surcharge_regardless_of_promo_state() {
    let with = calculate_cost_with_context("claude-sonnet-5", 50_000, 5_000, 100_000, 100_000);
    let plain = calculate_cost("claude-sonnet-5", 50_000, 5_000, 100_000, 100_000);
    assert!((with - plain).abs() < 0.0001);
}

#[test]
fn sonnet_5_display_with_context_always_shows_1m() {
    assert_eq!(
        model_display_with_context("claude-sonnet-5", "Claude Sonnet 5", 0),
        "Claude Sonnet 5 (1M)"
    );
}

#[test]
fn test_is_ga_1m_context() {
    assert!(is_ga_1m_context("claude-opus-4-6"));
    assert!(is_ga_1m_context("claude-opus-4-6-20260213"));
    assert!(is_ga_1m_context("claude-opus-4-6[1m]"));
    assert!(is_ga_1m_context("claude-sonnet-4-6"));
    assert!(is_ga_1m_context("claude-sonnet-4-6-20260213"));
    assert!(!is_ga_1m_context("claude-opus-4-5"));
    assert!(!is_ga_1m_context("claude-sonnet-4-5"));
    assert!(!is_ga_1m_context("claude-sonnet-4"));
    assert!(!is_ga_1m_context("claude-haiku-4-5"));
    assert!(!is_ga_1m_context("opus"));
}

#[test]
fn test_supports_1m_context() {
    assert!(supports_1m_context("claude-opus-4-6[1m]"));
    assert!(supports_1m_context("claude-sonnet-4-6[1m]"));
    assert!(supports_1m_context("claude-opus-4-6-20260213"));
    assert!(supports_1m_context("claude-opus-4-5-20251101"));
    assert!(supports_1m_context("claude-opus-4-6"));
    assert!(!supports_1m_context("claude-opus-4-1-20250414"));
    assert!(!supports_1m_context("claude-opus-4-20250514"));
    assert!(supports_1m_context("claude-sonnet-4-6-20260213"));
    assert!(supports_1m_context("claude-sonnet-4-5-20250929"));
    assert!(supports_1m_context("claude-sonnet-4-20250514"));
    assert!(supports_1m_context("claude-sonnet-4-6"));
    assert!(!supports_1m_context("claude-3-7-sonnet-20250219"));
    assert!(!supports_1m_context("claude-3-5-sonnet-20241022"));
    assert!(!supports_1m_context("claude-haiku-4-5-20251001"));
    assert!(!supports_1m_context("claude-haiku-3-5-20241022"));
}

#[test]
fn test_model_display_with_context() {
    assert_eq!(
        model_display_with_context("claude-sonnet-4-6", "Claude Sonnet 4.6", 300_000),
        "Claude Sonnet 4.6 (1M)"
    );
    assert_eq!(
        model_display_with_context("claude-sonnet-4-6", "Claude Sonnet 4.6", 0),
        "Claude Sonnet 4.6 (1M)"
    );
    assert_eq!(
        model_display_with_context("claude-opus-4-6[1m]", "Claude Opus 4.6", 0),
        "Claude Opus 4.6 (1M)"
    );
    assert_eq!(
        model_display_with_context("claude-sonnet-4-5", "Claude Sonnet 4.5", 300_000),
        "Claude Sonnet 4.5 (1M Context)"
    );
    assert_eq!(
        model_display_with_context("claude-sonnet-4-5", "Claude Sonnet 4.5", 150_000),
        "Claude Sonnet 4.5"
    );
    assert_eq!(
        model_display_with_context("claude-haiku-4-5", "Claude Haiku 4.5", 500_000),
        "Claude Haiku 4.5"
    );
}

#[test]
fn test_strip_claude_prefix() {
    assert_eq!(strip_claude_prefix("Claude Opus 4.6"), "Opus 4.6");
    assert_eq!(strip_claude_prefix("Claude Sonnet 4.5"), "Sonnet 4.5");
    assert_eq!(strip_claude_prefix("Claude Haiku 4.5"), "Haiku 4.5");
    assert_eq!(strip_claude_prefix("Claude"), "Claude");
    assert_eq!(strip_claude_prefix("Unknown Model"), "Unknown Model");
}

#[test]
fn opus_4_7_pricing_matches_4_6() {
    let p = model_pricing("claude-opus-4-7");
    assert!((p.input_per_million - 5.0).abs() < 0.001);
    assert!((p.output_per_million - 25.0).abs() < 0.001);
    assert!((p.cache_write_per_million - 6.25).abs() < 0.001);
    assert!((p.cache_read_per_million - 0.50).abs() < 0.001);
}

#[test]
fn opus_4_7_pricing_dated_id() {
    let p = model_pricing("claude-opus-4-7-20260301");
    assert!((p.input_per_million - 5.0).abs() < 0.001);
    assert!((p.output_per_million - 25.0).abs() < 0.001);
}

#[test]
fn opus_4_7_1m_suffix_strips_for_pricing() {
    let p = model_pricing("claude-opus-4-7[1m]");
    assert!((p.input_per_million - 5.0).abs() < 0.001);
}

#[test]
fn opus_4_7_is_ga_1m_context() {
    assert!(is_ga_1m_context("claude-opus-4-7"));
    assert!(is_ga_1m_context("claude-opus-4-7-20260301"));
    assert!(is_ga_1m_context("claude-opus-4-7[1m]"));
}

#[test]
fn opus_4_7_supports_1m_context() {
    assert!(supports_1m_context("claude-opus-4-7"));
    assert!(supports_1m_context("claude-opus-4-7-20260301"));
}

#[test]
fn opus_4_7_display_name() {
    assert_eq!(model_display_name("claude-opus-4-7"), "Claude Opus 4.7");
    assert_eq!(
        model_display_name("claude-opus-4-7-20260301"),
        "Claude Opus 4.7"
    );
    assert_eq!(model_display_name("claude-opus-4-7[1m]"), "Claude Opus 4.7");
}

#[test]
fn opus_4_7_display_with_context_always_shows_1m() {
    assert_eq!(
        model_display_with_context("claude-opus-4-7", "Claude Opus 4.7", 300_000),
        "Claude Opus 4.7 (1M)"
    );
    assert_eq!(
        model_display_with_context("claude-opus-4-7", "Claude Opus 4.7", 0),
        "Claude Opus 4.7 (1M)"
    );
}

#[test]
fn opus_4_7_no_1m_surcharge() {
    let with = calculate_cost_with_context("claude-opus-4-7", 50_000, 5_000, 100_000, 100_000);
    let plain = calculate_cost("claude-opus-4-7", 50_000, 5_000, 100_000, 100_000);
    assert!((with - plain).abs() < 0.0001);
}

#[test]
fn has_inflated_tokenizer_opus_4_7() {
    assert!(has_inflated_tokenizer("claude-opus-4-7"));
    assert!(has_inflated_tokenizer("claude-opus-4-7-20260301"));
    assert!(has_inflated_tokenizer("claude-opus-4-7[1m]"));
}

#[test]
fn has_inflated_tokenizer_older_opus_false() {
    assert!(!has_inflated_tokenizer("claude-opus-4-6"));
    assert!(!has_inflated_tokenizer("claude-opus-4-5"));
    assert!(!has_inflated_tokenizer("claude-opus-4-1"));
}

#[test]
fn has_inflated_tokenizer_non_opus_false() {
    assert!(!has_inflated_tokenizer("claude-sonnet-4-6"));
    assert!(!has_inflated_tokenizer("claude-haiku-4-5"));
    assert!(!has_inflated_tokenizer("unknown"));
}

#[test]
fn sonnet_5_has_inflated_tokenizer() {
    assert!(has_inflated_tokenizer("claude-sonnet-5"));
    assert!(has_inflated_tokenizer("claude-sonnet-5-20260625"));
    assert!(has_inflated_tokenizer("claude-sonnet-5[1m]"));
}

#[test]
fn sonnet_4_x_does_not_have_inflated_tokenizer() {
    assert!(!has_inflated_tokenizer("claude-sonnet-4-5"));
    assert!(!has_inflated_tokenizer("claude-sonnet-4-6"));
}

#[test]
fn sonnet_5_display_name() {
    assert_eq!(model_display_name("claude-sonnet-5"), "Claude Sonnet 5");
    assert_eq!(
        model_display_name("claude-sonnet-5-20260625"),
        "Claude Sonnet 5"
    );
    assert_eq!(model_display_name("claude-sonnet-5[1m]"), "Claude Sonnet 5");
}

#[test]
fn sonnet_5_strip_display_for_discord() {
    let display = model_display_name("claude-sonnet-5");
    assert_eq!(strip_claude_prefix(&display), "Sonnet 5");
}

#[test]
fn sonnet_5_is_not_fast_capable() {
    assert!(!is_fast_capable("claude-sonnet-5"));
}

#[test]
fn display_names_bare() {
    assert_eq!(model_display_name("opus"), "Claude Opus");
    assert_eq!(model_display_name("sonnet"), "Claude Sonnet");
    assert_eq!(model_display_name("haiku"), "Claude Haiku");
    assert_eq!(
        model_display_name("totally-unknown"),
        "Claude (totally-unknown)"
    );
    assert_eq!(model_display_name("<synthetic>"), "Claude");
}

#[test]
fn opus_4_8_pricing_matches_new_opus_rates() {
    let p = model_pricing("claude-opus-4-8");
    assert!((p.input_per_million - 5.0).abs() < 0.001);
    assert!((p.output_per_million - 25.0).abs() < 0.001);
    assert!((p.cache_write_per_million - 6.25).abs() < 0.001);
    assert!((p.cache_read_per_million - 0.50).abs() < 0.001);
}

#[test]
fn opus_4_8_pricing_dated_and_suffix_variants() {
    let dated = model_pricing("claude-opus-4-8-20260528");
    assert!((dated.input_per_million - 5.0).abs() < 0.001);
    assert!((dated.output_per_million - 25.0).abs() < 0.001);

    let suffixed = model_pricing("claude-opus-4-8[1m]");
    assert!((suffixed.input_per_million - 5.0).abs() < 0.001);
    assert!((suffixed.output_per_million - 25.0).abs() < 0.001);
}

#[test]
fn opus_4_8_display_name() {
    assert_eq!(model_display_name("claude-opus-4-8"), "Claude Opus 4.8");
    assert_eq!(
        model_display_name("claude-opus-4-8-20260528"),
        "Claude Opus 4.8"
    );
    assert_eq!(model_display_name("claude-opus-4-8[1m]"), "Claude Opus 4.8");
}

#[test]
fn opus_4_8_is_ga_1m_context() {
    assert!(is_ga_1m_context("claude-opus-4-8"));
    assert!(is_ga_1m_context("claude-opus-4-8-20260528"));
    assert!(is_ga_1m_context("claude-opus-4-8[1m]"));
}

#[test]
fn opus_4_8_has_inflated_tokenizer() {
    assert!(has_inflated_tokenizer("claude-opus-4-8"));
    assert!(has_inflated_tokenizer("claude-opus-4-8-20260528"));
    assert!(has_inflated_tokenizer("claude-opus-4-8[1m]"));
}

#[test]
fn opus_4_8_dated_and_suffix_price_identically() {
    let short = calculate_cost("claude-opus-4-8", 12_345, 6_789, 4_321, 9_876);
    let dated = calculate_cost("claude-opus-4-8-20260528", 12_345, 6_789, 4_321, 9_876);
    let suffixed = calculate_cost("claude-opus-4-8[1m]", 12_345, 6_789, 4_321, 9_876);
    assert!((short - dated).abs() < 0.0000001);
    assert!((short - suffixed).abs() < 0.0000001);
}

#[test]
fn opus_4_8_is_fast_capable() {
    assert!(is_fast_capable("claude-opus-4-8"));
    assert!(is_fast_capable("claude-opus-4-8-20260528"));
    assert!(is_fast_capable("claude-opus-4-8[1m]"));
}

#[test]
fn older_models_are_not_fast_capable() {
    assert!(!is_fast_capable("claude-opus-4-7"));
    assert!(!is_fast_capable("claude-opus-4-6"));
    assert!(!is_fast_capable("claude-opus-4-5"));
    assert!(!is_fast_capable("claude-sonnet-4-6"));
    assert!(!is_fast_capable("claude-haiku-4-5"));
}

#[test]
fn opus_4_8_fast_is_exactly_2x_standard() {
    let model = "claude-opus-4-8";
    let (input, output, cache_write, cache_read) = (123_456u64, 78_901u64, 45_678u64, 234_567u64);

    let standard =
        calculate_cost_with_context_and_speed(model, input, output, cache_write, cache_read, false);
    let fast =
        calculate_cost_with_context_and_speed(model, input, output, cache_write, cache_read, true);

    assert!((fast - standard * FAST_RATE_MULTIPLIER).abs() < 0.0000001);
    assert!((fast - standard * 2.0).abs() < 0.0000001);
}

#[test]
fn opus_4_8_fast_doubles_every_token_category() {
    let model = "claude-opus-4-8";
    let std_in = calculate_cost_with_context_and_speed(model, 1_000_000, 0, 0, 0, false);
    let fast_in = calculate_cost_with_context_and_speed(model, 1_000_000, 0, 0, 0, true);
    assert!((fast_in - std_in * 2.0).abs() < 0.0000001);
    let std_out = calculate_cost_with_context_and_speed(model, 0, 1_000_000, 0, 0, false);
    let fast_out = calculate_cost_with_context_and_speed(model, 0, 1_000_000, 0, 0, true);
    assert!((fast_out - std_out * 2.0).abs() < 0.0000001);
    let std_cw = calculate_cost_with_context_and_speed(model, 0, 0, 1_000_000, 0, false);
    let fast_cw = calculate_cost_with_context_and_speed(model, 0, 0, 1_000_000, 0, true);
    assert!((fast_cw - std_cw * 2.0).abs() < 0.0000001);
    let std_cr = calculate_cost_with_context_and_speed(model, 0, 0, 0, 1_000_000, false);
    let fast_cr = calculate_cost_with_context_and_speed(model, 0, 0, 0, 1_000_000, true);
    assert!((fast_cr - std_cr * 2.0).abs() < 0.0000001);
}

#[test]
fn fast_flag_is_ignored_on_non_fast_capable_models() {
    let std = calculate_cost_with_context_and_speed(
        "claude-opus-4-7",
        50_000,
        5_000,
        10_000,
        20_000,
        false,
    );
    let fast = calculate_cost_with_context_and_speed(
        "claude-opus-4-7",
        50_000,
        5_000,
        10_000,
        20_000,
        true,
    );
    assert!((std - fast).abs() < 0.0000001);
}

#[test]
fn category_costs_sum_to_headline_total_standard_and_fast() {
    let model = "claude-opus-4-8";
    let (input, output, cache_write, cache_read) = (123_456u64, 78_901u64, 45_678u64, 234_567u64);

    for fast in [false, true] {
        let bd = calculate_category_costs(model, input, output, cache_write, cache_read, fast);
        let headline = calculate_cost_with_context_and_speed(
            model,
            input,
            output,
            cache_write,
            cache_read,
            fast,
        );
        assert!((bd.total() - headline).abs() < 0.0000001, "fast={fast}");
    }
}

#[test]
fn category_costs_fast_doubles_every_component() {
    let model = "claude-opus-4-8";
    let std = calculate_category_costs(model, 100_000, 20_000, 8_000, 60_000, false);
    let fast = calculate_category_costs(model, 100_000, 20_000, 8_000, 60_000, true);
    assert!((fast.input_cost - std.input_cost * 2.0).abs() < 0.0000001);
    assert!((fast.output_cost - std.output_cost * 2.0).abs() < 0.0000001);
    assert!((fast.cache_write_cost - std.cache_write_cost * 2.0).abs() < 0.0000001);
    assert!((fast.cache_read_cost - std.cache_read_cost * 2.0).abs() < 0.0000001);
}

#[test]
fn category_costs_beta_surcharge_matches_headline() {
    let model = "claude-sonnet-4-5";
    let (input, output, cache_write, cache_read) = (50_000u64, 5_000u64, 100_000u64, 100_000u64);
    let bd = calculate_category_costs(model, input, output, cache_write, cache_read, false);
    let headline = calculate_cost_with_context(model, input, output, cache_write, cache_read);
    assert!((bd.total() - headline).abs() < 0.0000001);
}

#[test]
fn standard_matrix_prices_every_model_correctly() {
    let cases = [
        ("claude-opus-4-5-20251101", 5.0, 25.0),
        ("claude-opus-4-6-20260213", 5.0, 25.0),
        ("claude-opus-4-7-20260301", 5.0, 25.0),
        ("claude-opus-4-8-20260528", 5.0, 25.0),
        ("claude-sonnet-4-5-20250929", 3.0, 15.0),
        ("claude-haiku-4-5-20251001", 1.0, 5.0),
        ("claude-fable-5-20260609", 10.0, 50.0),
        ("claude-mythos-5", 10.0, 50.0),
    ];
    for (model, input_rate, output_rate) in cases {
        let p = model_pricing(model);
        assert!(
            (p.input_per_million - input_rate).abs() < 0.001,
            "{model} input rate"
        );
        assert!(
            (p.output_per_million - output_rate).abs() < 0.001,
            "{model} output rate"
        );
    }
}

#[test]
fn fable_5_pricing() {
    for model in ["claude-fable-5", "claude-mythos-5"] {
        let p = model_pricing(model);
        assert!((p.input_per_million - 10.0).abs() < 0.001, "{model} input");
        assert!(
            (p.output_per_million - 50.0).abs() < 0.001,
            "{model} output"
        );
        assert!(
            (p.cache_write_per_million - 12.5).abs() < 0.001,
            "{model} cache_write"
        );
        assert!(
            (p.cache_read_per_million - 1.0).abs() < 0.001,
            "{model} cache_read"
        );
    }
}

/// Fable/Mythos 5.1 share version 5's input, output and write rates but
/// read cache at $0.25 (0.025x input) instead of $1.
#[test]
fn fable_5_1_cache_reads_cost_a_quarter_of_version_5() {
    for model in [
        "claude-fable-5-1",
        "claude-fable-5-1-20260901",
        "claude-fable-5-1[1m]",
        "claude-mythos-5-1",
    ] {
        let p = model_pricing(model);
        assert!((p.input_per_million - 10.0).abs() < 0.001, "{model} input");
        assert!(
            (p.output_per_million - 50.0).abs() < 0.001,
            "{model} output"
        );
        assert!(
            (p.cache_write_per_million - 12.5).abs() < 0.001,
            "{model} cache_write"
        );
        assert!(
            (p.cache_read_per_million - 0.25).abs() < 0.001,
            "{model} cache_read"
        );
        assert!(model_display_name(model).ends_with("5.1"), "{model}");
    }
    for model in [
        "claude-fable-5",
        "claude-fable-5-20260609",
        "claude-mythos-5",
    ] {
        let p = model_pricing(model);
        assert!(
            (p.cache_read_per_million - 1.0).abs() < 0.001,
            "{model} keeps the version 5 read rate"
        );
    }
}

#[test]
fn fable_5_dated_variant_pricing() {
    let p = model_pricing("claude-fable-5-20260609");
    assert!((p.input_per_million - 10.0).abs() < 0.001, "input dated");
    assert!((p.output_per_million - 50.0).abs() < 0.001, "output dated");
    let p = model_pricing("claude-mythos-5-20260609");
    assert!(
        (p.input_per_million - 10.0).abs() < 0.001,
        "mythos input dated"
    );
    assert!(
        (p.output_per_million - 50.0).abs() < 0.001,
        "mythos output dated"
    );
}

#[test]
fn fable_5_display_name() {
    assert_eq!(model_display_name("claude-fable-5"), "Claude Fable 5");
    assert_eq!(
        model_display_name("claude-fable-5-20260609"),
        "Claude Fable 5"
    );
    assert_eq!(model_display_name("claude-fable-5[1m]"), "Claude Fable 5");
    assert_eq!(model_display_name("claude-mythos-5"), "Claude Mythos 5");
    assert_eq!(
        model_display_name("claude-mythos-5-20260609"),
        "Claude Mythos 5"
    );
}

#[test]
fn fable_5_is_ga_1m_context() {
    assert!(is_ga_1m_context("claude-fable-5"));
    assert!(is_ga_1m_context("claude-fable-5-20260609"));
    assert!(is_ga_1m_context("claude-mythos-5"));
    assert!(is_ga_1m_context("claude-mythos-5-20260609"));
}

#[test]
fn fable_5_supports_1m_context() {
    assert!(supports_1m_context("claude-fable-5"));
    assert!(supports_1m_context("claude-fable-5-20260609"));
    assert!(supports_1m_context("claude-mythos-5"));
    assert!(supports_1m_context("claude-mythos-5-20260609"));
}

#[test]
fn fable_5_no_1m_surcharge() {
    let with = calculate_cost_with_context("claude-fable-5", 50_000, 5_000, 100_000, 100_000);
    let plain = calculate_cost("claude-fable-5", 50_000, 5_000, 100_000, 100_000);
    assert!((with - plain).abs() < 0.0001, "GA model: no surcharge");
    let with = calculate_cost_with_context("claude-mythos-5", 50_000, 5_000, 100_000, 100_000);
    let plain = calculate_cost("claude-mythos-5", 50_000, 5_000, 100_000, 100_000);
    assert!(
        (with - plain).abs() < 0.0001,
        "Mythos GA model: no surcharge"
    );
}

#[test]
fn fable_5_display_with_context_shows_1m() {
    assert_eq!(
        model_display_with_context("claude-fable-5", "Claude Fable 5", 0),
        "Claude Fable 5 (1M)"
    );
    assert_eq!(
        model_display_with_context("claude-mythos-5", "Claude Mythos 5", 0),
        "Claude Mythos 5 (1M)"
    );
}

#[test]
fn fable_5_not_inflated_tokenizer() {
    assert!(!has_inflated_tokenizer("claude-fable-5"));
    assert!(!has_inflated_tokenizer("claude-mythos-5"));
}

#[test]
fn fable_5_not_fast_capable() {
    assert!(!is_fast_capable("claude-fable-5"));
    assert!(!is_fast_capable("claude-mythos-5"));
}

#[test]
fn fable_5_strip_display_for_discord() {
    let display = model_display_name("claude-fable-5");
    assert_eq!(display, "Claude Fable 5");
    assert_eq!(strip_claude_prefix(&display), "Fable 5");
    let display = model_display_name("claude-mythos-5");
    assert_eq!(display, "Claude Mythos 5");
    assert_eq!(strip_claude_prefix(&display), "Mythos 5");
}

/// Sonnet 5's launch price became its standard price when Anthropic
/// cancelled the September 1, 2026 increase, so every id shape bills at
/// $2 / $10 / $2.50 / $0.20 with no date dependency.
#[test]
fn sonnet_5_bills_at_the_permanent_2_and_10_rates() {
    for id in [
        "claude-sonnet-5",
        "claude-sonnet-5-20260625",
        "claude-sonnet-5[1m]",
        "CLAUDE-SONNET-5",
    ] {
        let p = model_pricing(id);
        assert!((p.input_per_million - 2.0).abs() < 0.001, "{id} input");
        assert!((p.output_per_million - 10.0).abs() < 0.001, "{id} output");
        assert!(
            (p.cache_write_per_million - 2.50).abs() < 0.001,
            "{id} cache_write"
        );
        assert!(
            (p.cache_read_per_million - 0.20).abs() < 0.001,
            "{id} cache_read"
        );
    }
}

#[test]
fn sonnet_5_lookalike_ids_do_not_get_sonnet_5_rates() {
    for id in [
        "claude-sonnet-50",
        "claude-sonnet-5x",
        "claude-sonnet-5-50",
        "claude-sonnet-55",
    ] {
        let p = model_pricing(id);
        assert!(
            (p.input_per_million - 3.0).abs() < 0.001,
            "{id} must use flat sonnet pricing, not the Sonnet 5 rate"
        );
        assert!(!is_sonnet_5_class(id), "{id} is not Sonnet 5");
        assert!(!is_sonnet_5_5_class(id), "{id} is not Sonnet 5.5");
    }
}

#[test]
fn sonnet_4_x_pricing_is_unchanged() {
    for id in [
        "claude-sonnet-4-5",
        "claude-sonnet-4-5-20250929",
        "claude-sonnet-4-6",
    ] {
        let p = model_pricing(id);
        assert!((p.input_per_million - 3.0).abs() < 0.001, "{id} input");
        assert!((p.output_per_million - 15.0).abs() < 0.001, "{id} output");
    }
}

// Opus 5 is the first Claude model whose ID carries a single version
// segment (`claude-opus-5`, dateless per Anthropic's model-ID docs).
// Every value below is taken literally from
// <https://platform.claude.com/docs/en/about-claude/pricing>.

/// Every published Opus 5 ID shape must resolve to the official
/// $5 / $25 / $6.25 / $0.50 rates.
#[test]
fn opus_5_pricing_matches_official_rates() {
    for model in [
        "claude-opus-5",
        "claude-opus-5-20260724",
        "claude-opus-5[1m]",
    ] {
        let p = model_pricing(model);
        assert!((p.input_per_million - 5.0).abs() < 0.001, "{model} input");
        assert!(
            (p.output_per_million - 25.0).abs() < 0.001,
            "{model} output"
        );
        assert!(
            (p.cache_write_per_million - 6.25).abs() < 0.001,
            "{model} cache write"
        );
        assert!(
            (p.cache_read_per_million - 0.50).abs() < 0.001,
            "{model} cache read"
        );
    }
}

/// The short, dated, and `[1m]`-suffixed IDs are the same model, so an
/// identical turn must cost the same through all three.
#[test]
fn opus_5_id_variants_price_identically() {
    let short = calculate_cost("claude-opus-5", 12_345, 6_789, 4_321, 9_876);
    let dated = calculate_cost("claude-opus-5-20260724", 12_345, 6_789, 4_321, 9_876);
    let suffixed = calculate_cost("claude-opus-5[1m]", 12_345, 6_789, 4_321, 9_876);
    assert!((short - dated).abs() < 0.000_001);
    assert!((short - suffixed).abs() < 0.000_001);
}

#[test]
fn opus_5_supports_and_ga_1m_context() {
    for model in [
        "claude-opus-5",
        "claude-opus-5-20260724",
        "claude-opus-5[1m]",
    ] {
        assert!(supports_1m_context(model), "{model} supports 1M");
        assert!(is_ga_1m_context(model), "{model} is GA 1M");
    }
}

/// 1M is Opus 5's native window at standard pricing, so a turn above the
/// 200K beta threshold must not pick up the 2x/1.5x long-context surcharge.
#[test]
fn opus_5_never_takes_the_beta_long_context_surcharge() {
    let model = "claude-opus-5";
    let (input, output, cache_write, cache_read) = (300_000u64, 20_000u64, 50_000u64, 40_000u64);
    assert!(
        input + cache_write + cache_read > 200_000,
        "above threshold"
    );
    let with_ctx = calculate_cost_with_context(model, input, output, cache_write, cache_read);
    let plain = calculate_cost(model, input, output, cache_write, cache_read);
    assert!((with_ctx - plain).abs() < 0.000_001);
}

#[test]
fn opus_5_is_fast_capable() {
    for model in [
        "claude-opus-5",
        "claude-opus-5-20260724",
        "claude-opus-5[1m]",
    ] {
        assert!(is_fast_capable(model), "{model}");
    }
}

/// Fast mode for Opus 5 is published as $10 / $50 per MTok, exactly 2x the
/// $5 / $25 standard rate, and the multiplier scales the whole turn.
#[test]
fn opus_5_fast_mode_matches_published_10_and_50_rates() {
    let model = "claude-opus-5";
    let fast_input = calculate_cost_with_context_and_speed(model, 1_000_000, 0, 0, 0, true);
    let fast_output = calculate_cost_with_context_and_speed(model, 0, 1_000_000, 0, 0, true);
    assert!((fast_input - 10.0).abs() < 0.001, "fast input $10/MTok");
    assert!((fast_output - 50.0).abs() < 0.001, "fast output $50/MTok");
}

#[test]
fn opus_5_fast_doubles_every_token_category() {
    let model = "claude-opus-5";
    let (input, output, cache_write, cache_read) = (50_000u64, 5_000u64, 100_000u64, 100_000u64);
    let standard = calculate_category_costs(model, input, output, cache_write, cache_read, false);
    let fast = calculate_category_costs(model, input, output, cache_write, cache_read, true);
    assert!((fast.input_cost - standard.input_cost * 2.0).abs() < 0.000_001);
    assert!((fast.output_cost - standard.output_cost * 2.0).abs() < 0.000_001);
    assert!((fast.cache_write_cost - standard.cache_write_cost * 2.0).abs() < 0.000_001);
    assert!((fast.cache_read_cost - standard.cache_read_cost * 2.0).abs() < 0.000_001);
    let headline =
        calculate_cost_with_context_and_speed(model, input, output, cache_write, cache_read, true);
    assert!((fast.total() - headline).abs() < 0.000_001);
}

/// Anthropic documents the newer tokenizer for "Claude 4.7 and later", but
/// no Opus 5 tokenizer figure is published, so Pulse does not infer one.
/// The flag is display-only and feeds no cost math.
#[test]
fn opus_5_has_no_inflated_tokenizer_flag() {
    for model in [
        "claude-opus-5",
        "claude-opus-5-20260724",
        "claude-opus-5[1m]",
    ] {
        assert!(!has_inflated_tokenizer(model), "{model}");
    }
    // The exclusion must stay surgical: 4.7 and 4.8 keep the flag.
    assert!(has_inflated_tokenizer("claude-opus-4-7"));
    assert!(has_inflated_tokenizer("claude-opus-4-8"));
}

#[test]
fn opus_5_display_names() {
    assert_eq!(model_display_name("claude-opus-5"), "Claude Opus 5");
    assert_eq!(
        model_display_name("claude-opus-5-20260724"),
        "Claude Opus 5"
    );
    assert_eq!(model_display_name("claude-opus-5[1m]"), "Claude Opus 5");
    assert_eq!(
        model_display_with_context("claude-opus-5", "Claude Opus 5", 0),
        "Claude Opus 5 (1M)"
    );
    assert_eq!(strip_claude_prefix("Claude Opus 5"), "Opus 5");
}

/// Guard rail for the relaxed single-segment parser: a hypothetical
/// "opus-50" is a different model and must not inherit Opus 5 handling.
#[test]
fn unrelated_opus_ids_are_not_absorbed_as_opus_5() {
    let p = model_pricing("claude-opus-50");
    assert!(
        (p.input_per_million - 5.0).abs() < 0.001,
        "opus-50 parses as major 50, which is still a new-Opus rate"
    );
    // The single-segment relaxation must not reach down to bare/legacy ids.
    assert!(!is_fast_capable("claude-opus-4"));
    assert!(!is_fast_capable("opus"));
    assert!(!is_ga_1m_context("claude-opus-4"));
    assert!(!has_inflated_tokenizer("claude-opus-4"));
}

// `claude-opus-5-5` is a two-segment id that begins with the Opus 5 id,
// so it must never inherit Opus 5 rates. Every value below is taken
// literally from <https://platform.claude.com/docs/en/about-claude/pricing>
// (verified 2026-09-22).

const OPUS_5_5_IDS: [&str; 3] = [
    "claude-opus-5-5",
    "claude-opus-5-5-20261001",
    "claude-opus-5-5[1m]",
];

/// $4 input, $20 output, $5 5-minute cache write and $0.20 cache read.
/// The cache read is 0.05x input, not the standard 0.10x.
#[test]
fn opus_5_5_pricing_matches_official_rates() {
    for model in OPUS_5_5_IDS {
        let p = model_pricing(model);
        assert!((p.input_per_million - 4.0).abs() < 0.001, "{model} input");
        assert!(
            (p.output_per_million - 20.0).abs() < 0.001,
            "{model} output"
        );
        assert!(
            (p.cache_write_per_million - 5.0).abs() < 0.001,
            "{model} cache write"
        );
        assert!(
            (p.cache_read_per_million - 0.20).abs() < 0.001,
            "{model} cache read"
        );
    }
}

#[test]
fn opus_5_5_is_not_classified_as_opus_5() {
    assert!(!is_opus_5_class("claude-opus-5-5"));
    assert!(!is_opus_5_class("claude-opus-5-5-20261001"));
    assert!(is_opus_5_5_class("claude-opus-5-5"));
    assert!(is_opus_5_5_class("claude-opus-5-5-20261001"));
    assert!(is_opus_5_class("claude-opus-5"));
    assert!(is_opus_5_class("claude-opus-5-20260724"));
    assert!(!is_opus_5_5_class("claude-opus-5"));
    assert!(!is_opus_5_5_class("claude-opus-5-20260724"));
    assert!(!is_opus_5_5_class("claude-opus-5-50"));
    assert!(!is_opus_5_class("claude-opus-50"));
}

/// Adding Opus 5.5 must not move Opus 5 off its own rates.
#[test]
fn opus_5_keeps_its_rates_next_to_opus_5_5() {
    let opus_5 = model_pricing("claude-opus-5");
    let opus_5_5 = model_pricing("claude-opus-5-5");
    assert!((opus_5.input_per_million - 5.0).abs() < 0.001);
    assert!((opus_5.cache_read_per_million - 0.50).abs() < 0.001);
    assert_ne!(opus_5, opus_5_5);
}

/// One turn with every category, checked against hand-computed arithmetic.
#[test]
fn opus_5_5_turn_cost_arithmetic() {
    let cost = calculate_cost("claude-opus-5-5", 100_000, 20_000, 50_000, 1_000_000);
    // 0.1M x $4 + 0.02M x $20 + 0.05M x $5 + 1M x $0.20
    let expected = 0.40 + 0.40 + 0.25 + 0.20;
    assert!((cost - expected).abs() < 0.000_001, "{cost} != {expected}");
}

#[test]
fn opus_5_5_id_variants_price_identically() {
    let costs: Vec<f64> = OPUS_5_5_IDS
        .iter()
        .map(|model| calculate_cost(model, 12_345, 6_789, 4_321, 9_876))
        .collect();
    assert!((costs[0] - costs[1]).abs() < 0.000_001);
    assert!((costs[0] - costs[2]).abs() < 0.000_001);
}

/// 1M is Opus 5.5's default and maximum window at standard pricing.
#[test]
fn opus_5_5_has_ga_1m_context_without_surcharge() {
    for model in OPUS_5_5_IDS {
        assert!(supports_1m_context(model), "{model} supports 1M");
        assert!(is_ga_1m_context(model), "{model} is GA 1M");
    }
    let (input, output, cache_write, cache_read) = (300_000u64, 20_000u64, 50_000u64, 40_000u64);
    let with_ctx =
        calculate_cost_with_context("claude-opus-5-5", input, output, cache_write, cache_read);
    let plain = calculate_cost("claude-opus-5-5", input, output, cache_write, cache_read);
    assert!((with_ctx - plain).abs() < 0.000_001);
}

/// Fast mode for Opus 5.5 is published as $8 / $40 per MTok, 2x standard.
#[test]
fn opus_5_5_fast_mode_matches_published_8_and_40_rates() {
    let model = "claude-opus-5-5";
    for id in OPUS_5_5_IDS {
        assert!(is_fast_capable(id), "{id}");
    }
    let fast_input = calculate_cost_with_context_and_speed(model, 1_000_000, 0, 0, 0, true);
    let fast_output = calculate_cost_with_context_and_speed(model, 0, 1_000_000, 0, 0, true);
    assert!((fast_input - 8.0).abs() < 0.001, "fast input $8/MTok");
    assert!((fast_output - 40.0).abs() < 0.001, "fast output $40/MTok");

    let standard = calculate_category_costs(model, 50_000, 5_000, 100_000, 100_000, false);
    let fast = calculate_category_costs(model, 50_000, 5_000, 100_000, 100_000, true);
    assert!((fast.cache_read_cost - standard.cache_read_cost * 2.0).abs() < 0.000_001);
    assert!((fast.total() - standard.total() * 2.0).abs() < 0.000_001);
}

/// Opus 5.5 carries no inflation flag (product decision, 2026-09-29).
#[test]
fn opus_5_5_has_no_inflated_tokenizer_flag() {
    for model in OPUS_5_5_IDS {
        assert!(!has_inflated_tokenizer(model), "{model}");
    }
}

#[test]
fn opus_5_5_display_names() {
    for model in OPUS_5_5_IDS {
        assert_eq!(model_display_name(model), "Claude Opus 5.5", "{model}");
    }
    assert_eq!(
        model_display_with_context("claude-opus-5-5", "Claude Opus 5.5", 0),
        "Claude Opus 5.5 (1M)"
    );
    assert_eq!(strip_claude_prefix("Claude Opus 5.5"), "Opus 5.5");
}

/// Opus 4.7 gained the tokenizer change but not fast mode; fast mode
/// starts at Opus 4.8.
#[test]
fn opus_4_7_still_not_fast_capable() {
    assert!(!is_fast_capable("claude-opus-4-7"));
    assert!(!is_fast_capable("claude-opus-4-6"));
}

// Sonnet 5.5 (API id and alias `claude-sonnet-5-5`, released 2026-09-28)
// costs the same as Sonnet 5 but is a distinct model, so it is classified
// explicitly instead of matching Sonnet 5 through a shared id prefix. Every
// value below is taken literally from
// <https://platform.claude.com/docs/en/about-claude/pricing> and
// <https://platform.claude.com/docs/en/about-claude/models/overview>
// (verified 2026-09-29).

const SONNET_5_5_IDS: [&str; 4] = [
    "claude-sonnet-5-5",
    "claude-sonnet-5-5-20261001",
    "claude-sonnet-5-5[1m]",
    "CLAUDE-SONNET-5-5",
];

/// $2 input, $10 output, $2.50 5-minute cache write and $0.10 cache read
/// (5% of input; Sonnet 5 keeps $0.20).
#[test]
fn sonnet_5_5_pricing_matches_official_rates() {
    for model in SONNET_5_5_IDS {
        let p = model_pricing(model);
        assert!((p.input_per_million - 2.0).abs() < 0.001, "{model} input");
        assert!(
            (p.output_per_million - 10.0).abs() < 0.001,
            "{model} output"
        );
        assert!(
            (p.cache_write_per_million - 2.50).abs() < 0.001,
            "{model} cache write"
        );
        assert!(
            (p.cache_read_per_million - 0.10).abs() < 0.001,
            "{model} cache read"
        );
    }
    assert_eq!(model_pricing("claude-sonnet-5-5"), SONNET_5_5_PRICING);
}

#[test]
fn sonnet_5_5_is_not_classified_as_sonnet_5() {
    assert!(is_sonnet_5_5_class("claude-sonnet-5-5"));
    assert!(is_sonnet_5_5_class("claude-sonnet-5-5-20261001"));
    assert!(!is_sonnet_5_class("claude-sonnet-5-5"));
    assert!(!is_sonnet_5_class("claude-sonnet-5-5-20261001"));
    assert!(is_sonnet_5_class("claude-sonnet-5"));
    assert!(is_sonnet_5_class("claude-sonnet-5-20260625"));
    assert!(!is_sonnet_5_5_class("claude-sonnet-5"));
    assert!(!is_sonnet_5_5_class("claude-sonnet-5-20260625"));
    assert!(!is_sonnet_5_5_class("claude-sonnet-4-5"));
    assert!(!is_sonnet_5_5_class("claude-sonnet-4-6"));
    assert!(!is_sonnet_5_5_class("claude-opus-5-5"));
}

/// Sonnet 5.5 reads cache at $0.10 while Sonnet 5 keeps $0.20; every other
/// rate is shared. Pin both so a change to one cannot move the other.
#[test]
fn sonnet_5_keeps_its_rates_next_to_sonnet_5_5() {
    assert_eq!(model_pricing("claude-sonnet-5"), SONNET_5_PRICING);
    assert_eq!(model_pricing("claude-sonnet-5-20260625"), SONNET_5_PRICING);
    assert_eq!(model_pricing("claude-sonnet-5-5"), SONNET_5_5_PRICING);
    assert!((model_pricing("claude-sonnet-5").cache_read_per_million - 0.20).abs() < 1e-12);
    assert!((model_pricing("claude-sonnet-5-5").cache_read_per_million - 0.10).abs() < 1e-12);
}

/// The other families must not drift when Sonnet 5.5 is added.
#[test]
fn sonnet_5_5_leaves_neighbouring_families_alone() {
    let opus_5_5 = model_pricing("claude-opus-5-5");
    assert!((opus_5_5.input_per_million - 4.0).abs() < 0.001);
    let sonnet_4_6 = model_pricing("claude-sonnet-4-6");
    assert!((sonnet_4_6.input_per_million - 3.0).abs() < 0.001);
    assert!((sonnet_4_6.output_per_million - 15.0).abs() < 0.001);
}

/// One million input tokens plus one million output tokens is $2 + $10.
#[test]
fn sonnet_5_5_turn_cost_arithmetic() {
    let base = calculate_cost("claude-sonnet-5-5", 1_000_000, 1_000_000, 0, 0);
    assert!((base - 12.0).abs() < 0.000_001, "{base} != 12");

    let cost = calculate_cost("claude-sonnet-5-5", 100_000, 20_000, 50_000, 1_000_000);
    // 0.1M x $2 + 0.02M x $10 + 0.05M x $2.50 + 1M x $0.10
    let expected = 0.20 + 0.20 + 0.125 + 0.10;
    assert!((cost - expected).abs() < 0.000_001, "{cost} != {expected}");
}

#[test]
fn sonnet_5_5_id_variants_price_identically() {
    let costs: Vec<f64> = SONNET_5_5_IDS
        .iter()
        .map(|model| calculate_cost(model, 12_345, 6_789, 4_321, 9_876))
        .collect();
    for cost in &costs[1..] {
        assert!((costs[0] - cost).abs() < 0.000_001);
    }
}

/// 1M is Sonnet 5.5's window at standard pricing, with no long-context
/// surcharge at any size.
#[test]
fn sonnet_5_5_has_ga_1m_context_without_surcharge() {
    for model in SONNET_5_5_IDS {
        assert!(supports_1m_context(model), "{model} supports 1M");
        assert!(is_ga_1m_context(model), "{model} is GA 1M");
    }
    let (input, output, cache_write, cache_read) = (300_000u64, 20_000u64, 50_000u64, 400_000u64);
    let with_ctx =
        calculate_cost_with_context("claude-sonnet-5-5", input, output, cache_write, cache_read);
    let plain = calculate_cost("claude-sonnet-5-5", input, output, cache_write, cache_read);
    assert!((with_ctx - plain).abs() < 0.000_001);

    let breakdown = calculate_category_costs(
        "claude-sonnet-5-5",
        input,
        output,
        cache_write,
        cache_read,
        false,
    );
    assert!((breakdown.total() - plain).abs() < 0.000_001);
}

/// Product decision (2026-09-29): 5.5-generation models never carry the
/// inflated-tokenizer marker, while Sonnet 5 keeps it.
#[test]
fn sonnet_5_5_has_no_inflated_tokenizer_flag() {
    for model in SONNET_5_5_IDS {
        assert!(!has_inflated_tokenizer(model), "{model}");
    }
    for model in OPUS_5_5_IDS {
        assert!(!has_inflated_tokenizer(model), "{model}");
    }
    assert!(has_inflated_tokenizer("claude-sonnet-5"));
    assert!(has_inflated_tokenizer("claude-sonnet-5-20260625"));
    assert!(has_inflated_tokenizer("claude-opus-4-7"));
    assert!(has_inflated_tokenizer("claude-opus-4-8"));
    assert!(!has_inflated_tokenizer("claude-opus-5"));
}

/// Fast mode belongs to Opus 4.8+ only. Sonnet 5.5 never bills at 2x, even
/// when the transcript says the turn ran at fast speed.
#[test]
fn sonnet_5_5_is_not_fast_capable() {
    for model in SONNET_5_5_IDS {
        assert!(!is_fast_capable(model), "{model}");
        assert!((speed_multiplier(model, true) - 1.0).abs() < 0.000_001);
    }
    let standard = calculate_cost_with_context_and_speed(
        "claude-sonnet-5-5",
        1_000_000,
        1_000_000,
        0,
        0,
        false,
    );
    let fast = calculate_cost_with_context_and_speed(
        "claude-sonnet-5-5",
        1_000_000,
        1_000_000,
        0,
        0,
        true,
    );
    assert!((standard - 12.0).abs() < 0.000_001);
    assert!((fast - standard).abs() < 0.000_001);
}

#[test]
fn sonnet_5_5_display_names() {
    for model in SONNET_5_5_IDS {
        assert_eq!(model_display_name(model), "Claude Sonnet 5.5", "{model}");
    }
    assert_eq!(
        model_display_with_context("claude-sonnet-5-5", "Claude Sonnet 5.5", 0),
        "Claude Sonnet 5.5 (1M)"
    );
    assert_eq!(strip_claude_prefix("Claude Sonnet 5.5"), "Sonnet 5.5");
    // Sonnet 5 must keep its own label.
    assert_eq!(model_display_name("claude-sonnet-5"), "Claude Sonnet 5");
}

// Haiku 5.5 tiers, 1-hour cache writes and Sonnet 5.5 cache reads.
// Rates from <https://platform.claude.com/docs/en/about-claude/pricing>,
// verified 2026-11-07. Haiku 5.5 bills a request at $0.10 / $0.50 / $0.125 /
// $0.01 (input / output / 5m write / cache read) when its prompt (input +
// cache read + cache write) is at most 100,000 tokens, and at $0.50 / $2.50 /
// $0.625 / $0.05 above that. A 1-hour write costs 2x base input.

const HAIKU_5_5_IDS: [&str; 4] = [
    "claude-haiku-5-5",
    "claude-haiku-5-5-20261101",
    "claude-haiku-5-5[1m]",
    "CLAUDE-HAIKU-5-5",
];

fn close(actual: f64, expected: f64) {
    assert!((actual - expected).abs() < 1e-9, "{actual} != {expected}");
}

#[test]
fn haiku_5_5_is_its_own_class() {
    for id in ["claude-haiku-5-5", "claude-haiku-5-5-20261101"] {
        assert_eq!(family_version(id, "haiku"), Some((5, 5)), "{id}");
        assert!(is_haiku_5_5_class(id), "{id}");
    }
    for id in [
        "claude-haiku-4-5",
        "claude-haiku-4-5-20251001",
        "claude-haiku-3-5-20241022",
        "claude-haiku-5-50",
        "claude-haiku-55",
        "haiku",
    ] {
        assert!(!is_haiku_5_5_class(id), "{id}");
    }
}

#[test]
fn haiku_5_5_low_tier_request_uses_the_low_row() {
    for model in HAIKU_5_5_IDS {
        // 50,000 x $0.10 + 1,000 x $0.50 = $0.0055 (per million, prompt 50,000)
        let b = calculate_category_costs(model, 50_000, 1_000, 0, 0, false);
        close(b.input_cost, 0.005);
        close(b.output_cost, 0.0005);
        close(b.total(), 0.0055);
        close(calculate_cost(model, 50_000, 1_000, 0, 0), 0.0055);
        close(
            calculate_cost_with_context(model, 50_000, 1_000, 0, 0),
            0.0055,
        );
    }
    let p = model_pricing("claude-haiku-5-5");
    close(p.input_per_million, 0.10);
    close(p.output_per_million, 0.50);
    close(p.cache_write_per_million, 0.125);
    close(p.cache_read_per_million, 0.01);
}

#[test]
fn haiku_5_5_prompt_of_exactly_100_000_tokens_is_the_low_tier() {
    // 100,000 x $0.10 = $0.01
    close(
        calculate_category_costs("claude-haiku-5-5", 100_000, 0, 0, 0, false).total(),
        0.01,
    );
    // The prompt counts every input category: 40,000 + 10,000 + 50,000 = 100,000.
    // 40,000 x $0.10 + 10,000 x $0.125 + 50,000 x $0.01 = $0.00575
    close(
        calculate_category_costs("claude-haiku-5-5", 40_000, 0, 10_000, 50_000, false).total(),
        0.005_75,
    );
}

#[test]
fn haiku_5_5_prompt_of_100_001_tokens_is_the_high_tier_for_every_category() {
    // 100,001 x $0.50 = $0.0500005
    close(
        calculate_category_costs("claude-haiku-5-5", 100_001, 0, 0, 0, false).total(),
        0.050_000_5,
    );
    // 40,001 + 10,000 + 50,000 = 100,001 tokens: the whole request moves up.
    // 40,001 x $0.50 + 10,000 x $0.625 + 50,000 x $0.05 = $0.0287505
    let b = calculate_category_costs("claude-haiku-5-5", 40_001, 0, 10_000, 50_000, false);
    close(b.input_cost, 0.020_000_5);
    close(b.cache_write_cost, 0.006_25);
    close(b.cache_read_cost, 0.002_5);
    close(b.total(), 0.028_750_5);
    // Output follows the prompt: 1,000 x $2.50 = $0.0025 in the high tier.
    close(
        calculate_category_costs("claude-haiku-5-5", 100_001, 1_000, 0, 0, false).output_cost,
        0.002_5,
    );
}

#[test]
fn haiku_5_5_high_tier_request_with_cache_reads_and_writes() {
    // Prompt 20,000 + 30,000 + 150,000 = 200,000.
    // 20,000 x $0.50 + 30,000 x $0.625 + 150,000 x $0.05 + 2,000 x $2.50 = $0.04125
    let b = calculate_category_costs("claude-haiku-5-5", 20_000, 2_000, 30_000, 150_000, false);
    close(b.input_cost, 0.01);
    close(b.cache_write_cost, 0.018_75);
    close(b.cache_read_cost, 0.007_5);
    close(b.output_cost, 0.005);
    close(b.total(), 0.041_25);
}

#[test]
fn haiku_5_5_tier_is_chosen_per_request_not_per_session() {
    // Two 60,000-token requests are two low-tier requests, not one 120,000 one.
    // 2 x (60,000 x $0.10) = $0.012
    let one = calculate_category_costs("claude-haiku-5-5", 60_000, 0, 0, 0, false).total();
    close(one * 2.0, 0.012);
    // One 120,000-token request is a high-tier request: 120,000 x $0.50 = $0.06.
    let merged = calculate_category_costs("claude-haiku-5-5", 120_000, 0, 0, 0, false).total();
    close(merged, 0.06);
}

#[test]
fn haiku_5_5_has_a_1m_window_at_standard_price_and_no_surcharge() {
    for model in HAIKU_5_5_IDS {
        assert!(supports_1m_context(model), "{model} supports 1M");
        assert!(is_ga_1m_context(model), "{model} is GA 1M");
    }
    // 300,000 prompt: 100,000 x $0.50 + 100,000 x $0.625 + 100,000 x $0.05 = $0.1175,
    // with no 2x / 1.5x long-context surcharge on top of the high row.
    let with_ctx = calculate_cost_with_context("claude-haiku-5-5", 100_000, 0, 100_000, 100_000);
    close(with_ctx, 0.1175);
    close(
        calculate_category_costs("claude-haiku-5-5", 100_000, 0, 100_000, 100_000, false).total(),
        with_ctx,
    );
    // Haiku 4.5 keeps its 200K window.
    assert!(!supports_1m_context("claude-haiku-4-5"));
    assert!(!is_ga_1m_context("claude-haiku-4-5"));
}

#[test]
fn haiku_5_5_is_neither_fast_capable_nor_tokenizer_flagged() {
    for model in HAIKU_5_5_IDS {
        assert!(!is_fast_capable(model), "{model}");
        assert!(!has_inflated_tokenizer(model), "{model}");
        let standard = calculate_cost_with_context_and_speed(model, 50_000, 1_000, 0, 0, false);
        let fast = calculate_cost_with_context_and_speed(model, 50_000, 1_000, 0, 0, true);
        close(fast, standard);
    }
}

#[test]
fn haiku_5_5_display_names() {
    for model in HAIKU_5_5_IDS {
        assert_eq!(model_display_name(model), "Claude Haiku 5.5", "{model}");
    }
    assert_eq!(
        model_display_with_context("claude-haiku-5-5", "Claude Haiku 5.5", 0),
        "Claude Haiku 5.5 (1M)"
    );
    assert_eq!(strip_claude_prefix("Claude Haiku 5.5"), "Haiku 5.5");
}

#[test]
fn haiku_4_5_keeps_its_flat_rates_next_to_haiku_5_5() {
    for prompt in [50_000u64, 300_000] {
        let p = model_pricing_for_prompt("claude-haiku-4-5", prompt);
        close(p.input_per_million, 1.0);
        close(p.output_per_million, 5.0);
        close(p.cache_write_per_million, 1.25);
        close(p.cache_read_per_million, 0.10);
    }
    assert!(!has_prompt_length_tiers("claude-haiku-4-5"));
    assert!(has_prompt_length_tiers("claude-haiku-5-5[1m]"));
}

#[test]
fn one_hour_cache_writes_cost_twice_base_input() {
    for (model, one_hour_rate) in [
        ("claude-opus-5-5", 8.0),
        ("claude-sonnet-5-5", 4.0),
        ("claude-sonnet-5", 4.0),
        ("claude-opus-4-6", 10.0),
        ("claude-opus-4-1", 30.0),
        ("claude-sonnet-4-5", 6.0),
        ("claude-haiku-4-5", 2.0),
        ("claude-fable-5-1", 20.0),
        ("claude-haiku-5-5", 0.20),
    ] {
        close(
            model_pricing(model).cache_write_1h_per_million(),
            one_hour_rate,
        );
    }
    // The Haiku 5.5 high row doubles to $1.
    close(
        model_pricing_for_prompt("claude-haiku-5-5", 100_001).cache_write_1h_per_million(),
        1.0,
    );
}

#[test]
fn one_hour_writes_on_opus_5_5_are_billed_at_8_dollars() {
    // 100,000 x $8 = $0.80
    let b = calculate_category_costs_with_cache_ttl(
        "claude-opus-5-5",
        0,
        0,
        CacheWrites::split(0, 100_000),
        0,
        false,
    );
    close(b.cache_write_cost, 0.80);
    close(b.total(), 0.80);
}

#[test]
fn mixed_five_minute_and_one_hour_writes_are_billed_each_at_their_rate() {
    // 40,000 x $5 + 60,000 x $8 = $0.68 for the writes;
    // plus 10,000 x $4 + 100,000 x $0.20 + 1,000 x $20 = $0.08.
    let b = calculate_category_costs_with_cache_ttl(
        "claude-opus-5-5",
        10_000,
        1_000,
        CacheWrites::split(40_000, 60_000),
        100_000,
        false,
    );
    close(b.cache_write_cost, 0.68);
    close(b.total(), 0.76);
}

#[test]
fn without_a_ttl_split_every_cache_write_is_a_five_minute_write() {
    // 100,000 x $5 = $0.50.
    let legacy = calculate_category_costs("claude-opus-5-5", 0, 0, 100_000, 0, false);
    close(legacy.cache_write_cost, 0.50);
    let explicit = calculate_category_costs_with_cache_ttl(
        "claude-opus-5-5",
        0,
        0,
        CacheWrites::all_five_minute(100_000),
        0,
        false,
    );
    close(explicit.cache_write_cost, 0.50);
    close(calculate_cost("claude-opus-5-5", 0, 0, 100_000, 0), 0.50);
}

#[test]
fn cache_writes_split_never_invents_or_loses_tokens() {
    assert_eq!(CacheWrites::all_five_minute(7).total(), 7);
    assert_eq!(CacheWrites::split(3, 4).total(), 7);
    // A reported 1h count above the request total is clamped to the total.
    let clamped = CacheWrites::from_total_and_one_hour(10, Some(25));
    assert_eq!(clamped, CacheWrites::split(0, 10));
    // The remainder of a partial split is a five-minute write.
    let partial = CacheWrites::from_total_and_one_hour(10, Some(4));
    assert_eq!(partial, CacheWrites::split(6, 4));
    // No split reported: all five-minute.
    assert_eq!(
        CacheWrites::from_total_and_one_hour(10, None),
        CacheWrites::all_five_minute(10)
    );
}

#[test]
fn one_hour_writes_on_haiku_5_5_follow_the_prompt_tier() {
    // Prompt 100,000, all 1h writes: 100,000 x $0.20 = $0.02.
    let low = calculate_category_costs_with_cache_ttl(
        "claude-haiku-5-5",
        0,
        0,
        CacheWrites::split(0, 100_000),
        0,
        false,
    );
    close(low.cache_write_cost, 0.02);
    // Prompt 200,000, all 1h writes: 200,000 x $1 = $0.20.
    let high = calculate_category_costs_with_cache_ttl(
        "claude-haiku-5-5",
        0,
        0,
        CacheWrites::split(0, 200_000),
        0,
        false,
    );
    close(high.cache_write_cost, 0.20);
}

#[test]
fn one_hour_writes_take_the_beta_surcharge_like_five_minute_writes() {
    // Sonnet 4.5 above 200K prompt: 2x cache. 300,000 x $6 x 2 = $3.60.
    let b = calculate_category_costs_with_cache_ttl(
        "claude-sonnet-4-5",
        0,
        0,
        CacheWrites::split(0, 300_000),
        0,
        false,
    );
    close(b.cache_write_cost, 3.60);
}

#[test]
fn one_hour_writes_double_with_fast_mode() {
    // Opus 4.8 fast: 100,000 x $10 x 2 = $2.00.
    let b = calculate_category_costs_with_cache_ttl(
        "claude-opus-4-8",
        0,
        0,
        CacheWrites::split(0, 100_000),
        0,
        true,
    );
    close(b.cache_write_cost, 2.0);
}

#[test]
fn sonnet_5_5_cache_reads_cost_a_tenth_of_a_dollar_per_million() {
    // 50,000 x $0.10 = $0.005
    let b = calculate_category_costs("claude-sonnet-5-5", 0, 0, 0, 50_000, false);
    close(b.cache_read_cost, 0.005);
    // Sonnet 5 keeps $0.20: 50,000 x $0.20 = $0.01
    let sonnet_5 = calculate_category_costs("claude-sonnet-5", 0, 0, 0, 50_000, false);
    close(sonnet_5.cache_read_cost, 0.01);
}
