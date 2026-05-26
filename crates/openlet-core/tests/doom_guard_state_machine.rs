//! Property-based invariants on `doom_guard::check`.
//!
//! Pure-stateless detector — perfect proptest target. The properties
//! lock down the rule of "abort only when every recent turn is a
//! tool-only loop with non-growing call sets". A regression here would
//! mean either false-positive aborts (wrecks long sessions) or
//! false-negatives (lets the loop run forever).

use openlet_core::runtime::doom_guard::{
    DEFAULT_THRESHOLD, DoomVerdict, ToolCallSig, TurnSummary, check,
};
use proptest::prelude::*;
use std::collections::BTreeSet;

fn arb_sig() -> impl Strategy<Value = ToolCallSig> {
    ("[a-z]{3,8}", "[a-z0-9]{4,16}").prop_map(|(name, payload)| {
        let v = serde_json::json!({ "x": payload });
        ToolCallSig::new(name, &v)
    })
}

fn arb_turn() -> impl Strategy<Value = TurnSummary> {
    (
        any::<bool>(),
        any::<bool>(),
        prop::collection::btree_set(arb_sig(), 0..4),
    )
        .prop_map(|(text, writes, tool_calls)| TurnSummary {
            had_text_output: text,
            had_successful_writes: writes,
            tool_calls,
        })
}

fn tool_only_turn(sigs: BTreeSet<ToolCallSig>) -> TurnSummary {
    TurnSummary {
        had_text_output: false,
        had_successful_writes: false,
        tool_calls: sigs,
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 128, .. ProptestConfig::default() })]

    /// Threshold of 0 disables the guard entirely. Holds for ANY history.
    #[test]
    fn threshold_zero_is_always_ok(history in prop::collection::vec(arb_turn(), 0..20)) {
        prop_assert_eq!(check(&history, 0), DoomVerdict::Ok);
    }

    /// History strictly shorter than the threshold cannot abort.
    #[test]
    fn history_too_short_is_always_ok(
        history in prop::collection::vec(arb_turn(), 0..3),
        threshold in 4usize..10,
    ) {
        prop_assert_eq!(check(&history, threshold), DoomVerdict::Ok);
    }

    /// If any turn within the last `threshold` had text output, the
    /// guard MUST stay Ok regardless of tool-call shape.
    #[test]
    fn any_text_output_in_window_short_circuits_ok(
        sigs in prop::collection::btree_set(arb_sig(), 1..3),
        which in 0usize..3,
    ) {
        let mut turns = vec![
            tool_only_turn(sigs.clone()),
            tool_only_turn(sigs.clone()),
            tool_only_turn(sigs.clone()),
        ];
        // Flip text on exactly one of the three turns. Must be Ok.
        turns[which].had_text_output = true;
        prop_assert_eq!(check(&turns, DEFAULT_THRESHOLD), DoomVerdict::Ok);
    }

    /// Same for successful writes.
    #[test]
    fn any_successful_writes_in_window_short_circuits_ok(
        sigs in prop::collection::btree_set(arb_sig(), 1..3),
        which in 0usize..3,
    ) {
        let mut turns = vec![
            tool_only_turn(sigs.clone()),
            tool_only_turn(sigs.clone()),
            tool_only_turn(sigs.clone()),
        ];
        turns[which].had_successful_writes = true;
        prop_assert_eq!(check(&turns, DEFAULT_THRESHOLD), DoomVerdict::Ok);
    }

    /// Empty tool-call set in any recent turn → Ok. Text-only turns can't
    /// loop, by definition.
    #[test]
    fn any_empty_tool_set_in_window_short_circuits_ok(
        sigs in prop::collection::btree_set(arb_sig(), 1..3),
        which in 0usize..3,
    ) {
        let mut turns = vec![
            tool_only_turn(sigs.clone()),
            tool_only_turn(sigs.clone()),
            tool_only_turn(sigs.clone()),
        ];
        turns[which].tool_calls.clear();
        prop_assert_eq!(check(&turns, DEFAULT_THRESHOLD), DoomVerdict::Ok);
    }

    /// Three identical tool-only turns → Abort. The full positive case
    /// for the guard's reason for existence.
    #[test]
    fn three_identical_tool_only_turns_abort(
        sigs in prop::collection::btree_set(arb_sig(), 1..4),
    ) {
        let turns = vec![
            tool_only_turn(sigs.clone()),
            tool_only_turn(sigs.clone()),
            tool_only_turn(sigs.clone()),
        ];
        let verdict = check(&turns, DEFAULT_THRESHOLD);
        let aborted = matches!(verdict, DoomVerdict::Abort { .. });
        prop_assert!(aborted, "expected Abort, got Ok");
    }

    /// Strict-superset growth across turns → Ok. Locks the direction of
    /// the subset check (cur ⊆ prev, not prev ⊆ cur).
    #[test]
    fn strict_superset_growth_does_not_abort(
        base in prop::collection::btree_set(arb_sig(), 1..3),
        extra1 in arb_sig(),
        extra2 in arb_sig(),
    ) {
        // Skip the (rare) case where the random extras collide with base.
        prop_assume!(!base.contains(&extra1) && !base.contains(&extra2) && extra1 != extra2);

        let mut t2 = base.clone();
        t2.insert(extra1.clone());
        let mut t3 = t2.clone();
        t3.insert(extra2);

        let turns = vec![
            tool_only_turn(base),
            tool_only_turn(t2),
            tool_only_turn(t3),
        ];
        prop_assert_eq!(check(&turns, DEFAULT_THRESHOLD), DoomVerdict::Ok);
    }

    /// Cross-turn arg differences do NOT abort even if the tool name is
    /// identical. Locks blake3-of-canonical-JSON: different `args_json`
    /// → different signature.
    #[test]
    fn differing_args_break_the_loop(
        cmd1 in "[a-z]{3,8}",
        cmd2 in "[a-z]{3,8}",
    ) {
        prop_assume!(cmd1 != cmd2);
        let s1 = ToolCallSig::new("bash", &serde_json::json!({"cmd": cmd1}));
        let s2 = ToolCallSig::new("bash", &serde_json::json!({"cmd": cmd2}));

        let turns = vec![
            tool_only_turn([s1.clone()].into_iter().collect()),
            tool_only_turn([s2.clone()].into_iter().collect()),
            tool_only_turn([s1].into_iter().collect()),
        ];
        prop_assert_eq!(check(&turns, DEFAULT_THRESHOLD), DoomVerdict::Ok);
    }

    /// Adding a healthy turn after an abort-state suffix breaks the
    /// abort: only the LAST `threshold` turns are inspected.
    #[test]
    fn healthy_turn_appended_resets_window(
        sigs in prop::collection::btree_set(arb_sig(), 1..3),
    ) {
        let mut turns = vec![
            tool_only_turn(sigs.clone()),
            tool_only_turn(sigs.clone()),
            tool_only_turn(sigs.clone()),
        ];
        // Append a healthy turn (text output present).
        let mut healthy = tool_only_turn(sigs);
        healthy.had_text_output = true;
        turns.push(healthy);

        // The window is now turns 1..4, including the healthy one.
        prop_assert_eq!(check(&turns, DEFAULT_THRESHOLD), DoomVerdict::Ok);
    }
}
