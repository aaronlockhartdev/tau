//! The context gate ladder (spec §4, #338): an over-budget prompt is compacted
//! before it is sent, cheapest rung first, and a prompt the ladder cannot fit
//! is refused pre-wire. The doomed 400 the #338 incident sent onto the wire is
//! the failure this module makes unproducible.
//!
//! Rung order, from the issue: promote buffered chunks (no LLM call) →
//! synchronous observe → reflect, escalating to `prefix_demoted` → refuse.
//! The refusal keeps #255's shape: a durable `context_overflow` record and
//! [`AgentError::ContextOverflow`].
use super::{Assembled, prompt_size};
use crate::agent::{AgentError, AgentSession, ContextGate, Inner, clamp_max_output, prompt_budget};
use tau_protocol::Diagnostic;

/// The prompt the gate is pricing, with everything a compaction invalidates:
/// a pass replaces the prefix an anchored count describes, so the assembly,
/// the count and the plan state move together (#338). Bundled by name because
/// every rung passes all four.
struct Priced<'a> {
    assembled: &'a mut Assembled,
    prompt: &'a mut u32,
    plan_state: &'a mut Option<String>,
    tools_json: &'a str,
}

/// What a compaction rung can change: the observe cursor, the observation
/// log's size, whether the frozen prefix is demoted, and the buffer's length.
/// Compared before and after a rung to decide whether a re-pricing is earned.
type OmFingerprint = (Option<String>, u32, bool, usize);

impl AgentSession {
    /// The output cap for one assembled prompt: the gate's derived cap
    /// (`min(max_tokens, window − prompt − margin)`), running the ladder first
    /// when the prompt starts over budget.
    ///
    /// A gate that cannot be armed is a refusal, never an unbounded prompt
    /// (scope 1), and a derived cap of `0` is a refusal too: a model with a
    /// declared window is never allowed to answer 0 tokens. Both refusals are
    /// pre-wire, durable, and cost the turn.
    pub(super) async fn gate_cap(
        &self,
        gate: &ContextGate,
        assembled: &mut Assembled,
        prompt: &mut u32,
        plan_state: &mut Option<String>,
        tools_json: &str,
    ) -> Result<u64, AgentError> {
        let model = self.model();
        let (budget, wish, window) = match gate {
            // Scope 1: a `Missing` gate is a refusal, never an unbounded prompt.
            ContextGate::Missing { reasons } => {
                self.record_gate_gap(&model, reasons)?;
                return Err(AgentError::GateUnavailable {
                    model,
                    reasons: reasons.clone(),
                });
            }
            // The budget the armed gate enforces, from the window: the ladder,
            // the refusal and the derived cap all price against it.
            ContextGate::Ready { window, cap } => (prompt_budget(*window), *cap, *window),
        };
        let priced = &mut Priced {
            assembled,
            prompt,
            plan_state,
            tools_json,
        };
        if *priced.prompt > budget {
            self.ladder(budget, priced).await?;
        }
        // The cap derives from the prompt that survived the ladder, so the
        // request the loop builds next carries the same arithmetic the gate
        // just priced (`prompt + cap + margin <= window`). Zero is no room.
        let cap = clamp_max_output(wish, window, *priced.prompt);
        if cap == 0 {
            self.record_gate_gap(&model, &["context_room".to_owned()])?;
            return Err(AgentError::GateUnavailable {
                model,
                reasons: vec!["context_room".to_owned()],
            });
        }
        Ok(cap)
    }

    /// The over-budget ladder, cheapest rung first, against the armed gate's
    /// `budget`.
    ///
    /// A rung earns a re-pricing only by compacting something: the count the
    /// gate holds is the provider's charged one once a call has been made
    /// (§6.9), and re-estimating over a branch that did not shrink would
    /// replace that fact with a smaller guess and send the doomed request the
    /// gate exists to stop. So each rung compares the compaction state before
    /// and after, and the loop leaves as soon as the prompt fits.
    ///
    /// A compaction that fails is recorded (`om_failed`) and shrinks nothing,
    /// so a ladder whose compaction has failed ends in the refusal: the gate
    /// refuses when the compaction it depends on has failed, and a turn under
    /// budget never gets here, so the #86 contract ("a failed pass must not
    /// kill the turn") stands where it is right.
    async fn ladder(&self, budget: u32, priced: &mut Priced<'_>) -> Result<(), AgentError> {
        let Some(mut compacted) = self.om_fingerprint() else {
            // No OM state: no rung can shrink this branch. The count the gate
            // holds is the provider's own or the whole-branch estimate, and it
            // says the prompt does not fit — refuse pre-wire (#255).
            return Err(self.refuse_over_budget(*priced.prompt, budget));
        };
        // Rung 1: promote the buffered chunks — the compaction with no LLM call.
        self.om_promote();
        if self.rung_fits(budget, &mut compacted, priced)? {
            return Ok(());
        }
        // Rung 2: the synchronous observe/reflect pass. Its prompt is the
        // unobserved raw, so an over-window main prompt is not necessarily an
        // over-window observe call.
        self.om_turn_end().await?;
        if self.rung_fits(budget, &mut compacted, priced)? {
            return Ok(());
        }
        // Rung 3: reflect at the escalating levels, then demote the frozen
        // prefix. The gate invokes `prefix_demoted` itself: the turn-end pass
        // reaches it only at `reflect_threshold`, and a prompt over budget is
        // the fact that should open it (scope 3).
        self.om_overflow_compact().await;
        if self.rung_fits(budget, &mut compacted, priced)? {
            return Ok(());
        }
        // Every rung ran and the prompt still does not fit: refuse loudly and
        // durably, before the wire.
        Err(self.refuse_over_budget(*priced.prompt, budget))
    }
    /// Whether a rung moved the compaction state, and if it did, whether the
    /// re-priced prompt now fits `budget`. `since` carries the state the last
    /// counted rung saw, so a rung that repeats an earlier rung's work earns
    /// nothing.
    fn rung_fits(
        &self,
        budget: u32,
        since: &mut OmFingerprint,
        priced: &mut Priced<'_>,
    ) -> Result<bool, AgentError> {
        let Some(now) = self.om_fingerprint() else {
            return Ok(false);
        };
        if &now == since {
            return Ok(false);
        }
        *since = now;
        self.reprice(priced)?;
        Ok(*priced.prompt <= budget)
    }

    /// The durable refusal every exhausted ladder ends in: the `context_overflow`
    /// record and the error that carries #255's shape. The record is the trace,
    /// not the point: a failure to append it goes to the log and the refusal
    /// stands (the prompt is over budget whatever the record channel does).
    fn refuse_over_budget(&self, prompt: u32, budget: u32) -> AgentError {
        if let Err(e) = self.diagnostic(&Diagnostic::ContextOverflow {
            prompt: u64::from(prompt),
            budget: u64::from(budget),
        }) {
            tracing::warn!("gate: the context_overflow record failed to append: {e}");
        }
        AgentError::ContextOverflow { prompt, budget }
    }

    /// The compaction state the ladder can shrink the prompt with, read under
    /// the session lock. `None` means no OM state, so compaction is
    /// unachievable for this session.
    fn om_fingerprint(&self) -> Option<OmFingerprint> {
        let guard = self.inner.lock();
        let Inner { om, .. } = &*guard;
        om.as_ref().map(|state| {
            (
                state
                    .record
                    .cursor
                    .as_ref()
                    .map(|cursor| cursor.entry_id.clone()),
                state.record.observation_tokens,
                state.record.prefix_demoted,
                state.buffered.len(),
            )
        })
    }

    /// Re-assemble and re-price after a compaction: the pass replaced the
    /// prefix an anchored count describes, so the prompt is priced whole, and a
    /// mid-turn compaction is a plan-state re-injection point (design).
    fn reprice(&self, priced: &mut Priced<'_>) -> Result<(), AgentError> {
        *priced.plan_state = self.plan_state();
        *priced.assembled = self.assemble(priced.plan_state.as_deref())?;
        *priced.prompt = prompt_size(None, priced.assembled, priced.tools_json);
        Ok(())
    }

    /// The durable trace of a gate with no budget: one `gate_unavailable`
    /// record per model per run (the #306 latch — one per request is spam, a
    /// silent refusal is the defect the incident showed).
    fn record_gate_gap(&self, model: &str, reasons: &[String]) -> Result<(), AgentError> {
        let fresh = {
            let mut guard = self.inner.lock();
            let Inner {
                gate_gap_recorded, ..
            } = &mut *guard;
            if gate_gap_recorded.as_deref() == Some(model) {
                false
            } else {
                *gate_gap_recorded = Some(model.to_owned());
                true
            }
        };
        if fresh {
            self.diagnostic(&Diagnostic::GateUnavailable {
                model: model.to_owned(),
                missing: reasons.to_vec(),
            })?;
        }
        Ok(())
    }
}
