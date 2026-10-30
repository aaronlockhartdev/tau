//! Fleet reclamation (ticket #250, design `subagent-cleanup-policy.md`):
//! spawn-triggered eviction of the oldest *terminal* children once the
//! handle fleet exceeds its soft bound, a hard cap on total handles as a
//! defensive backstop, and re-adopt on address — a resolve miss falls back
//! to the disk records and rebuilds a `Child` around a freshly opened
//! `AgentSession`, so a reclaimed child is "cold", not a zombie. The
//! reclamation unit is the in-memory handle only: the child's session file
//! and its live session are never touched.

use super::restore::{self, DiskChild};
use super::{
    AgentSession, Arc, Child, ReadoptNotice, ReclaimNotice, SessionStore, Supervisor, TurnOrigin,
};
use crate::harness::{SessionRole, info_to_protocol};
use crate::om_integration::OmState;
use crate::tools;
use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize};

/// The fleet's soft bound: `None` (or zero) means no terminal reclamation.
#[must_use]
fn soft_bound(caps: &crate::config::SubAgents) -> Option<usize> {
    caps.fleet_soft_bound
        .map(|b| usize::try_from(b).unwrap_or(0))
        .filter(|&b| b > 0)
}

/// The fleet's hard cap: `None` (or zero) means unbounded.
#[must_use]
fn hard_cap(caps: &crate::config::SubAgents) -> Option<usize> {
    caps.fleet_hard_cap
        .map(|b| usize::try_from(b).unwrap_or(0))
        .filter(|&b| b > 0)
}

impl Supervisor {
    /// The hard cap as a spawn-time backstop (design §6.3): the soft bound
    /// exists to keep the fleet under this, so a spawn that would still
    /// exceed it is a defensive refusal, not a user-facing failure path.
    pub(super) fn check_fleet_cap(&self) -> Result<(), String> {
        let Some(hard) = hard_cap(&self.caps) else {
            return Ok(());
        };
        let total = self.children.lock().len();
        if total + 1 > hard {
            return Err(format!("subagent_spawn: fleet cap ({hard}) reached"));
        }
        Ok(())
    }

    /// Spawn-triggered terminal reclamation (design §6.2): after a spawn
    /// inserts its child, evict the oldest *terminal* children (mint order)
    /// until the fleet is back under the soft bound. `Running`, `Idle`, and
    /// `Awaiting` children are never evicted — a parked child is resting on
    /// a live interaction (the #187/#188 resume contract).
    pub(super) fn reclaim_terminal(&self) {
        let Some(soft) = soft_bound(&self.caps) else {
            return;
        };
        let mut children = self.children.lock();
        if children.len() < soft {
            return;
        }
        // Candidates: terminal children, oldest first. The handle's mint
        // counter is the age (a fresh parent mints from 1); `created` is a
        // coarse (epoch-ms) tie-break only — a burst of spawns in the same
        // millisecond shares a `created` while the mint counter stays
        // unique per parent. Owned handles + clones: the candidates must not
        // keep a borrow of the map live across the `remove` below.
        let mut candidates: Vec<(String, Arc<Child>)> = children
            .iter()
            .filter(|(_, c)| c.state().is_terminal())
            .map(|(h, c)| (h.clone(), c.clone()))
            .collect();
        candidates.sort_by(|(ha, ca), (hb, cb)| {
            ca.created
                .cmp(&cb.created)
                .then_with(|| mint_counter(ha).cmp(&mint_counter(hb)))
        });
        for (handle, child) in candidates {
            if children.len() <= soft {
                break;
            }
            children.remove(&handle);
            self.bridge.reclaimed(&ReclaimNotice {
                parent: self.parent_session.clone(),
                handle,
                child: child.session_id.clone(),
            });
        }
    }

    /// The fleet the snapshot and `subagent_list` project (design §6.6):
    /// the registry's live handles plus the on-disk children that are no
    /// longer in the registry (reclaimed). Without the disk half, a
    /// partially-reclaimed fleet makes the early children disappear from the
    /// snapshot until a restart.
    #[must_use]
    pub fn fleet_view(&self) -> Vec<tau_protocol::SubagentInfo> {
        let live = self
            .handles()
            .iter()
            .filter_map(|h| self.state_info(h).map(|i| info_to_protocol(&i)))
            .collect::<Vec<_>>();
        let live_children: std::collections::HashSet<String> =
            live.iter().map(|i| i.child.clone()).collect();
        let from_disk = restore::disk_children(
            &self.cwd,
            &self.parent_session,
            self.parent_entries.as_deref(),
        )
        .into_iter()
        .filter(|i| !live_children.contains(&i.child))
        .map(|i| restore::disk_child_to_info(&i))
        .collect::<Vec<_>>();
        let mut out = live;
        out.extend(from_disk);
        // Spawn order (#226 variant): `created`, then the child session id.
        out.sort_by_key(|i| (i.created, i.child.clone()));
        out
    }

    /// Re-adopt a reclaimed child on a `resolve` miss (design §4): the disk
    /// record matched by name / session id / handle, whose last recorded
    /// state is terminal, is rebuilt into a `Child` around a freshly opened
    /// `AgentSession` and re-inserted. The cost is one disk read + session
    /// open, paid only when the parent (or the GUI) actually addresses the
    /// cold child. A missing or unreadable file, an unparseable record, a
    /// non-terminal (parked) child, and a hard-cap overflow all return
    /// `None` — re-adopt never fabricates a child.
    pub(super) fn re_adopt_from_disk(&self, name_or_id: &str) -> Option<Arc<Child>> {
        // A re-adopt adds one handle: the hard cap bounds it (the soft
        // bound is not a hard limit, so it does not gate re-adopt).
        self.check_fleet_cap().ok()?;
        // The child's link needs an `Arc<Supervisor>`: upgrade the self
        // ref (`new` always sets it).
        let sup = {
            let guard = self.self_weak.lock().clone();
            guard.and_then(|w| w.upgrade())?
        };
        let disk = restore::disk_children(&self.cwd, &self.parent_session, None);
        let d: &DiskChild = disk
            .iter()
            .find(|i| i.name == name_or_id || i.child == name_or_id || i.handle == name_or_id)?;
        let mut store = SessionStore::for_workspace(&self.cwd, &d.child);
        store.open().ok()?;
        let record = OmState::load_record(&mut store).unwrap_or_default();
        let agent = AgentSession::launch(
            store,
            SessionRole::Child {
                supervisor: sup,
                // The re-adopted child is addressed at the provider its model
                // id names, the same route a spawn takes (#336).
                provider: self.provider.create(&d.child, &self.model),
                system_prompt: self.system_prompt.clone(),
                model: self.model.clone(),
                tools: tools::surface::child_specs(None),
                record,
                handle: d.handle.clone(),
            },
        )
        .ok()?;
        let child = Arc::new(Child {
            handle: d.handle.clone(),
            name: d.name.clone(),
            session_id: d.child.clone(),
            agent_type: d.agent_type.clone(),
            context_mode: d.context_mode,
            agent: agent.clone(),
            created: d.created,
            // Set the terminal state directly (no `set_state`): a re-adopt
            // must not write a duplicate state entry to the child's file —
            // the file already records the terminal state that re-adopt
            // read.
            state: Mutex::new(d.state.clone()),
            origin: Mutex::new(TurnOrigin::Message),
            rest_requested: AtomicBool::new(false),
            declared_output: Mutex::new(None),
            declared_waiting: Mutex::new(None),
            last_message: Mutex::new(None),
            drive_gen: AtomicUsize::new(0),
            last_ended_gen: AtomicUsize::new(0),
            wake: tokio::sync::Notify::new(),
        });
        self.children
            .lock()
            .insert(child.handle.clone(), child.clone());
        // The reclamation dropped the mint record: re-register the handle
        // (the bridge re-records `child_handles`; the child's live session
        // and file are untouched).
        self.bridge.re_adopted(&ReadoptNotice {
            parent: self.parent_session.clone(),
            handle: child.handle.clone(),
            child: child.session_id.clone(),
        });
        Some(child)
    }
}

/// A handle's mint counter (`{parent}-{n}` → `n`): the child's age in mint
/// order. An unparseable handle (a re-adopted session-id handle) is 0 —
/// the oldest, which is correct: it has no mint-order evidence of being
/// newer.
fn mint_counter(handle: &str) -> usize {
    handle
        .rsplit('-')
        .next()
        .and_then(|n| n.parse().ok())
        .unwrap_or(0)
}
