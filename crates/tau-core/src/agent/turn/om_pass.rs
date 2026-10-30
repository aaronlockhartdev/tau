use crate::agent::{AgentError, AgentSession, Inner, SessionStore, TurnProviderRef};
use crate::om_integration::{OmError, OmStage};
use tau_protocol::Diagnostic;

/// The stage a compaction failure belongs to (#338): the provider arm carries
/// the stage the retry loop ran under; a storage failure names the store, the
/// one place no LLM pass ran.
fn om_failure_stage(e: &OmError) -> OmStage {
    match e {
        OmError::Provider(stage, _) => *stage,
        OmError::Session(_) => OmStage::Session,
    }
}

/// The OM model to use: the configured OM model, or the model the session's
/// requests address when it is empty (spec §4). The fallback is the address and
/// not the session's id, so a menu-qualified id does not reach the endpoint with
/// its provider still attached (§6.7).
fn resolve_om_model(om_model: &str, address: &str) -> String {
    if om_model.is_empty() {
        address.to_owned()
    } else {
        om_model.to_owned()
    }
}

impl AgentSession {
    /// The OM provider: a forwarding provider's inner (the registry-backed
    /// one the OM pass calls), or the provider itself when it has none.
    fn om_provider(&self) -> TurnProviderRef {
        self.provider
            .forwarding_inner()
            .unwrap_or_else(|| self.provider.clone())
    }

    /// The ladder's first rung (#338): promote the buffered chunks — the
    /// compaction that costs no LLM call. The mid-loop trigger may already have
    /// done it; promotion is idempotent (an empty buffer is `false`).
    pub(super) fn om_promote(&self) {
        let outcome = {
            let mut guard = self.inner.lock();
            let Inner { om, store, .. } = &mut *guard;
            match om.as_mut() {
                None => Ok(()),
                Some(om) => om.promote(store).map(|_| ()),
            }
        };
        if let Err(e) = outcome {
            // Promotion runs no LLM pass, so the only failure it can have is
            // the record save: the stage is `session`, never `buffer` (#338).
            self.record_om_failure(om_failure_stage(&e), &e);
        }
    }

    /// The ladder's last rung (#338): reflect at the escalating levels, then
    /// demote the frozen prefix. The turn-end pass reaches demotion only at
    /// `reflect_threshold`; the gate runs it because the prompt — not the
    /// threshold — is over budget, and the prefix is the material that remains
    /// when the raw window is empty. A failure is recorded, never swallowed:
    /// the gate reads the next re-pricing and refuses when it still does not
    /// fit (the #86 "a failed pass must not kill the turn" contract stands for
    /// a turn under budget, which never reaches this rung).
    pub(super) async fn om_overflow_compact(&self) {
        let (state, model, hook) = {
            let inner = self.inner.lock();
            (
                inner.om.clone(),
                resolve_om_model(&inner.om_model, &inner.turn.request_address(&inner.model)),
                inner.om_status_hook.clone(),
            )
        };
        let Some(mut state) = state else { return };
        // The re-lock, as in `om_turn_end`: the session lock is never held
        // across a provider call.
        let mut with_store =
            |f: &mut dyn FnMut(&mut SessionStore) -> Result<(), crate::om_integration::OmError>| {
                let mut guard = self.inner.lock();
                f(&mut guard.store)
            };
        let om_provider = self.om_provider();
        let outcome = state
            .overflow_compact(&mut with_store, &om_provider, &model, hook.as_ref())
            .await;
        if let Err(e) = &outcome {
            self.record_om_failure(om_failure_stage(e), e);
            if let Some(hook) = &hook {
                hook("idle");
            }
        }
        self.write_back_om(state);
    }

    /// The turn-end OM pass (ticket #22, R4): one call — `settle_turn`
    /// owns the whole sequence (the unobserved read, activation, the plan,
    /// the observe/reflect round-trips, the commit). The state runs on a
    /// clone and is written back at the end; the store is handed to it
    /// phase by phase through the re-lock, so the session lock is never
    /// held across a provider call (a std guard cannot cross an await in a
    /// spawned future).
    pub(super) async fn om_turn_end(&self) -> Result<(), AgentError> {
        // The OM model (global config, spec §4); empty = the session's
        // model. Resolved with the state clone, before the pass.
        let (state, model, hook) = {
            let inner = self.inner.lock();
            (
                inner.om.clone(),
                resolve_om_model(&inner.om_model, &inner.turn.request_address(&inner.model)),
                inner.om_status_hook.clone(),
            )
        };
        let Some(mut state) = state else {
            return Ok(());
        };
        // D13.1: join the in-flight buffer cycle BEFORE the pass — the
        // pass's activation (promote) reads the buffer the cycle writes.
        // 60s bound: a cycle outliving it is fine, the write-back merge
        // below picks up its commit (D13.2) and the turn doesn't hang.
        let handle = {
            let mut inner = self.inner.lock();
            inner.om_inflight.take()
        };
        if let Some(handle) = handle {
            let timed_out = tokio::time::timeout(std::time::Duration::from_secs(60), handle)
                .await
                .is_err();
            if timed_out {
                tracing::warn!(
                    "om: buffer cycle outlived the 60s join; the turn-end merge picks up its commit"
                );
            }
        }
        // The re-lock: each sync store phase takes the session lock and
        // releases it before the next provider call.
        let mut with_store =
            |f: &mut dyn FnMut(&mut SessionStore) -> Result<(), crate::om_integration::OmError>| {
                let mut guard = self.inner.lock();
                f(&mut guard.store)
            };
        // The pass goes through the session's inner provider, not the
        // forwarding seam (ticket #86): OM calls are not user-facing
        // streams, and keeping them out of the forwarding call registry
        // means a failed pass cannot surface as an interrupted `StreamEnd`
        // in the post-turn reconciliation (the #82 class). A plain
        // (non-forwarding) provider has no registry — use it as-is.
        let om_provider = self.om_provider();
        // A failed pass (a provider error that survived its retries, a
        // storage error) must not kill the turn (ticket #86): the entries
        // stay unobserved and the next turn-end retries; the partial state
        // is written back as-is.
        if let Err(e) = state
            .settle_turn(&mut with_store, &om_provider, &model, hook.as_ref())
            .await
        {
            // #338: an exhausted retry is a durable `om_failed` record, not a
            // log line — a dead `om_model` route hid in this `warn!` for a
            // whole incident. The turn still lives (ticket #86): the record is
            // the fact, the gate's ladder is the consequence.
            self.record_om_failure(om_failure_stage(&e), &e);
            // Close the status gauge: a failed pass never reaches the
            // loop's `Done` arm, the only place it goes idle.
            if let Some(hook) = &hook {
                hook("idle");
            }
        }
        self.write_back_om(state);
        // A pass that leaves the window at or past activation is a fact the
        // harness has to be able to see afterwards (ADR-0034): it is the
        // condition that ends in a provider cutoff.
        self.record_om_stall();
        Ok(())
    }

    /// D13.2: write a pass's state back — a field-split merge over the file's
    /// record, so a cycle that committed during the pass's LLM round-trip (the
    /// join timed out) keeps its chunks and the pass's observation fields win.
    fn write_back_om(&self, mut state: crate::om_integration::OmState) {
        {
            let mut inner = self.inner.lock();
            match crate::om_integration::OmState::load_record(&mut inner.store) {
                Ok(fresh) => {
                    state.merge_turn_end(&fresh);
                    // Persist only when the merge changed the DURABLE chunk state — a
                    // racing cycle's commit (the join timed out) or a
                    // sync-buffered chunk. Compare the chunk fields, not
                    // the whole record: the pass refreshes
                    // `pending_tokens` on every run, so a full equality
                    // would re-save (append an `om` entry) on every turn
                    // of any session with unobserved entries.
                    let chunk_delta = state.record.buffered_chunks != fresh.buffered_chunks
                        || state.record.last_buffered_at_tokens > fresh.last_buffered_at_tokens;
                    if !chunk_delta {
                        // The common case: the merge is a no-op over the
                        // file's chunk state, so nothing is re-saved.
                    } else if let Err(e) = state.save(&mut inner.store) {
                        tracing::warn!("om: write-back merge save failed: {e}");
                    }
                    inner.om = Some(state);
                }
                Err(e) => {
                    tracing::warn!("om: record load for the write-back merge failed: {e}");
                    inner.om = Some(state);
                }
            }
        }
    }

    /// The `om_failed` record, through the state's latch-and-append (the one
    /// implementation `Inner::record_om_failure` owns).
    fn record_om_failure(&self, stage: OmStage, e: &OmError) {
        self.inner.lock().record_om_failure(stage, e);
    }
}

impl Inner {
    /// The durable `om_failed` record of an exhausted OM round-trip (#338): the
    /// stage that died and the failure that killed it, on the diagnostic
    /// channel. Latched once per stage per run (the #306 rule): a dead
    /// `om_model` route fails every pass, and a record per pass is spam that
    /// buries the one fact. The background buffer cycle holds this state, not an
    /// `AgentSession`, so the latch lives here and both sites share it.
    pub(crate) fn record_om_failure(&mut self, stage: OmStage, e: &OmError) {
        let stage = stage.as_str();
        if self.om_failed_recorded.iter().any(|s| s == stage) {
            return;
        }
        self.om_failed_recorded.push(stage.to_owned());
        let outcome = self.record_diagnostic(&Diagnostic::OmFailed {
            stage: stage.to_owned(),
            reason: e.to_string(),
        });
        if let Err(e) = outcome {
            // The record is the point of this call, so the failure to append is
            // itself worth a line, and the latch clears so the next pass
            // retries it (the `record_om_stall` trade).
            self.om_failed_recorded.retain(|s| s != stage);
            tracing::warn!("om: the om_failed record failed to append: {e}");
        }
    }
}

impl AgentSession {
    /// Record the stalled-window fact when a completed pass left the unobserved
    /// window at or past the activation threshold: OM runs but can no longer
    /// keep the prompt inside what it has observed (ADR-0034). Once per run —
    /// `om_stall_recorded` mirrors the record once the append lands, so a
    /// restart re-records the stall it resumes into instead of re-scanning the
    /// log.
    fn record_om_stall(&self) {
        let stalled = {
            let mut guard = self.inner.lock();
            let Inner {
                om,
                store,
                om_stall_recorded,
                ..
            } = &mut *guard;
            let Some(om) = om.as_mut() else {
                return;
            };
            let Ok(unobserved) = om.unobserved(store) else {
                return;
            };
            let pending = om.pending_tokens(&unobserved);
            let activation = om
                .config
                .observe_threshold_for(om.record.observation_tokens);
            if !om.activation_reached(pending) || *om_stall_recorded {
                return;
            }
            *om_stall_recorded = true;
            (pending, activation)
        };
        if let Err(e) = self.diagnostic(&Diagnostic::OmStalled {
            pending: u64::from(stalled.0),
            activation: u64::from(stalled.1),
        }) {
            // A stall worth recording is not worth killing the turn that
            // reports it, so the failure goes to the log and the latch clears:
            // the next turn-end retries the record. A run in which every retry
            // fails ends up with the fact log-only — the trade, not an
            // oversight.
            self.inner.lock().om_stall_recorded = false;
            tracing::warn!("om: the stalled-window record failed to append: {e}");
        }
    }
    /// The mid-episode interval-boundary trigger (ticket #86 P2, D3/D4):
    /// runs after each tool round — the moment new unobserved tokens
    /// exist. At the observe threshold it promotes buffered chunks
    /// (activation, no LLM call); below it, it spawns the background
    /// buffer cycle when an interval boundary was crossed. The spawn
    /// happens INSIDE the lock section: the in-flight flag is set before
    /// the task can re-acquire the lock (D14), so the cycle and the
    /// trigger never race on it. With `buffer_tokens = 0` (kill switch)
    /// this is a no-op and the pass keeps today's sync behavior.
    pub(super) fn om_mid_loop(&self) {
        let mut guard = self.inner.lock();
        let Inner {
            store,
            om,
            om_inflight,
            om_buffer_boundary,
            om_model,
            model,
            turn,
            ..
        } = &mut *guard;
        let Some(om) = om.as_mut() else {
            return;
        };
        if om.config.buffer_tokens == 0 {
            return;
        }
        let Ok(unobserved) = om.unobserved(store) else {
            // A store read failure degrades: keep the last known pending,
            // no trigger this round (the turn-end pass retries the read).
            return;
        };
        om.record.pending_tokens = om.pending_tokens(&unobserved);
        let spawn_pending = if om.activation_reached(om.record.pending_tokens) {
            // Activation: promote the buffered chunks into the log (no LLM
            // call); the sync turn-end pass owns the at/above-threshold
            // range from here (the mastra partition).
            match om.promote(store) {
                Ok(_) => None,
                Err(e) => {
                    tracing::warn!("om: mid-loop activation failed: {e}");
                    None
                }
            }
        } else if crate::om_integration::background::boundary_crossed(
            u64::from(om.record.pending_tokens),
            &om.config,
            om.record.last_buffered_at_tokens,
            *om_buffer_boundary,
        ) {
            Some(om.record.pending_tokens)
        } else {
            None
        };
        let Some(pending) = spawn_pending else {
            return;
        };
        if om_inflight.is_some() {
            // A cycle is already running: it owns the interval; the next
            // crossing (one more interval of material) triggers again.
            return;
        }
        // The in-process boundary advances at TRIGGER time (D6); the
        // persisted one only at the cycle's successful commit.
        *om_buffer_boundary = u64::from(pending);
        let inner = std::sync::Arc::clone(&self.inner);
        let config = om.config.clone();
        let provider = self.om_provider();
        let model_owned = resolve_om_model(om_model, &turn.request_address(model));
        *om_inflight = Some(tokio::spawn(
            crate::om_integration::background::buffer_cycle(
                inner,
                config,
                provider,
                model_owned,
                u64::from(pending),
            ),
        ));
    }
}
