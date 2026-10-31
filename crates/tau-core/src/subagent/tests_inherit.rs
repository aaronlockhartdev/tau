//! The inherit rule (#336) at the supervisor seam: a model-less child is built
//! from the pair the supervisor carries — the parent's model and the turn
//! options derived from it — and a parent's move takes both halves at once.

use super::*;
use crate::subagent::testkit::*;

fn facts(max_output_tokens: u64) -> TurnConfig {
    TurnConfig {
        max_output_tokens: Some(max_output_tokens),
        context_window: 32768,
        ..TurnConfig::default()
    }
}

/// A parent's move rebinds the pair together. The gate's ceiling is the model's,
/// so a model that moved alone prices a child with the old model's window — the
/// launch-time pair is what a stale supervisor hands down.
#[test]
fn a_parents_move_takes_the_inherited_model_and_its_facts_together() {
    let dir = tempfile::tempdir().unwrap();
    let (sup, _bridge, _parent) = harness(dir.path(), vec![], vec![vec![]], SubAgents::default());

    sup.set_inherited("zzz/gpt".into(), facts(8192));
    assert_eq!(sup.inherited().model, "zzz/gpt");
    assert_eq!(
        sup.inherited().turn.max_output_tokens,
        Some(8192),
        "the facts move with the model, not one turn behind it"
    );

    sup.set_inherited("aaa/other".into(), facts(4096));
    assert_eq!(sup.inherited().model, "aaa/other");
    assert_eq!(
        sup.inherited().turn.max_output_tokens,
        Some(4096),
        "a second move takes both halves again"
    );
}
