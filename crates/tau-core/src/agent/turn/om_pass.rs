use crate::agent::{AgentError, AgentSession, Inner, SessionStore, TurnProviderRef};

/// The OM model to use: the configured OM model, or the session's model when
/// it is empty (spec §4).
fn resolve_om_model(om_model: &str, model: &str) -> String {
    if om_model.is_empty() {
        model.to_owned()
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
            let inner = self
                .inner
                .lock()
                .expect("agent inner: no panic while the lock is held");
            (
                inner.om.clone(),
                resolve_om_model(&inner.om_model, &inner.model),
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
            let mut inner = self
                .inner
                .lock()
                .expect("agent inner: no panic while the lock is held");
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
                let mut guard = self
                    .inner
                    .lock()
                    .expect("agent inner: no panic while the lock is held");
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
            tracing::warn!("om: turn-end pass failed: {e}");
            // Close the status gauge: a failed pass never reaches the
            // loop's `Done` arm, the only place it goes idle.
            if let Some(hook) = &hook {
                hook("idle");
            }
        }
        // D13.2: the write-back is a field-split merge, not a clobber — a
        // cycle that committed during the pass's LLM round-trip (the
        // join timed out) keeps its chunks; the pass's observation
        // fields win.
        {
            let mut inner = self
                .inner
                .lock()
                .expect("agent inner: no panic while the lock is held");
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
        Ok(())
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
        let mut guard = self
            .inner
            .lock()
            .expect("agent inner: no panic while the lock is held");
        let Inner {
            store,
            om,
            om_inflight,
            om_buffer_boundary,
            om_model,
            model,
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
        let model_owned = resolve_om_model(om_model, model);
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
