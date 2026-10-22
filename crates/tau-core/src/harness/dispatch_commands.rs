//! The message and sub-agent arms.

use super::{
    AgentType, Arc, Command, CommandOutput, ContextMode, Core, Lane, MessageLane, Ordering, Path,
    ProtocolError, StoppedBy, info_to_protocol, lane_to_lane, run_turn,
};

impl Core {
    #[allow(clippy::too_many_lines)] // one command per arm; splitting is refactoring
    pub(crate) fn dispatch_command(
        self: &Arc<Self>,
        cmd: Command,
    ) -> Result<CommandOutput, ProtocolError> {
        match cmd {
            Command::MessageSend {
                session,
                text,
                lane,
            } => {
                let live = self.live_unarchived(&session)?;
                // A new send clears the stop flag: the previous turn is over.
                live.stop.store(false, Ordering::SeqCst);
                // A leading /skill: expands at this boundary, before the
                // entry is recorded (ticket #28); a misspelled name
                // rejects the send — nothing is recorded.
                let (expanded, skill) = self.expand_skill(&live, &text)?;
                // A child session takes messages through its supervisor
                // (the parent's subagent_message semantics: a running
                // child gets a steering-lane message; a non-running one is
                // resumed with it, ADR-0001).
                // A child session takes messages through its link (R3):
                // the link's supervisor is the parent's. The parent must
                // be live — a closed parent has no routing surface.
                if let Some(link) = live.agent.child_link() {
                    self.live(link.parent_session())?;
                    if lane == MessageLane::Force {
                        // The supervisor's running branch routes Force to
                        // `send_as`, whose kill+head-of-queue semantics cut
                        // the in-flight turn. Stop explicitly first: the cut
                        // is only reliable while the child is running, and
                        // the force must win even if the child parked in the
                        // window between the check and the send (#208).
                        link.stop()
                            .map_err(|e| ProtocolError::Other { message: e })?;
                    }
                    let lane = lane_to_lane(lane);
                    link.send(text.clone(), lane, Some("user".to_owned()))
                        .map_err(|e| ProtocolError::Other { message: e })?;
                    // The child's queue is projected like a top-level
                    // session's: the send lands on the child's lane, and the
                    // GUI's queue pane shows it (#187). The Force variant
                    // took the head of the queue, so the projection covers
                    // both.
                    self.emit_queue(&live);
                    return Ok(CommandOutput::None);
                }
                match skill {
                    Some((name, location)) => {
                        live.agent
                            .send_skill(expanded, lane_to_lane(lane), &name, &location);
                    }
                    None => live.agent.send(expanded, lane_to_lane(lane)),
                }
                // Turn start is a check-and-set on the turn flag (spec §8
                // single writer). A send that lands while a turn is in flight
                // is the queued one: it shows in the GUI's queue and the
                // in-flight process() absorbs it (spec §7). A send that starts
                // a turn IS the turn — it is not queued, so an idle send is
                // delivered immediately instead of sitting in the queue
                // section until the turn's reconciliation runs.
                let started = live
                    .turn
                    .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                    .is_ok();
                if !started && lane != MessageLane::Force {
                    // The Queue event is a projection of the agent's lane
                    // queue (the single source of truth) — it shows what the
                    // next turn will record (a /skill: send records its
                    // expansion, not the raw line).
                    self.emit_queue(&live);
                }
                if started {
                    let live = Arc::clone(&live);
                    let core = Arc::clone(self);
                    tokio::spawn(async move { run_turn(core, live).await });
                }
                Ok(CommandOutput::None)
            }
            Command::MessageStop { session } => {
                let live = self.live(&session)?;
                // A child session stops through its link (R3) into the
                // parent's supervisor: the terminal Stopped state, the
                // freed slot, the state event (no parent wake — spec
                // §5.2). A bare stream cut would leave the child running:
                // its drive ends the turn and rests. The parent must be live.
                if let Some(link) = live.agent.child_link() {
                    self.live(link.parent_session())?;
                    link.stop()
                        .map_err(|e| ProtocolError::Other { message: e })?;
                    return Ok(CommandOutput::None);
                }
                live.stop.store(true, Ordering::SeqCst);
                // The agent's own stop flag is what append_assistant reads to
                // mark the entry interrupted; the LiveSession flag above only
                // cuts the stream. Both must be set, or a user stop renders as
                // a normal completion.
                live.agent.stop_flag().store(true, Ordering::SeqCst);
                Ok(CommandOutput::None)
            }
            Command::QueueItemRemove { session, index } => {
                let live = self.live(&session)?;
                // A child session's queue is the parent's projection: there
                // is no top-level delete for a child (its items are the
                // parent's), so refuse — the GUI only offers it for
                // top-level sessions.
                if live.agent.child_link().is_some() {
                    return Err(ProtocolError::Other {
                        message: "a child session's queue is managed by its parent".into(),
                    });
                }
                // Out of bounds (the item was consumed first) is a clean,
                // specific error — never a silent success.
                if live.agent.remove_queued(index).is_none() {
                    return Err(ProtocolError::Other {
                        message: "queue index out of bounds".into(),
                    });
                }
                self.emit_queue(&live);
                Ok(CommandOutput::None)
            }

            Command::SubagentTypes => Ok(CommandOutput::Agents {
                agents: crate::agent_type::discover(
                    self.system_dir.as_deref(),
                    Path::new("/nonexistent-tau-project"),
                )
                .iter()
                .map(|t| AgentType {
                    name: t.name.clone(),
                    description: t.description.clone(),
                    builtin: t.name == "general",
                })
                .collect(),
            }),
            Command::SubagentList { session } => {
                let live = self.live(&session)?;
                let sup = live.agent.subagents().ok_or_else(|| ProtocolError::Other {
                    message: "session has no children (it is a child itself)".into(),
                })?;
                Ok(CommandOutput::Subagents {
                    // The union (design #250 §6.6): the registry's live
                    // handles plus the on-disk children no longer in the
                    // registry (reclaimed) — the same projection the
                    // snapshot uses.
                    subagents: sup.fleet_view(),
                })
            }
            Command::SubagentState { handle } => {
                // The handle is opaque (R3): the parent comes from the
                // spawn registry, never the handle's format.
                let session =
                    self.child_parent(&handle)
                        .ok_or_else(|| ProtocolError::NotFound {
                            what: format!("subagent {handle}"),
                        })?;
                let live = self.live(&session)?;
                let sup = live.agent.subagents().ok_or_else(|| ProtocolError::Other {
                    message: "session has no children (it is a child itself)".into(),
                })?;
                let info = sup
                    .state_info(&handle)
                    .ok_or_else(|| ProtocolError::NotFound {
                        what: format!("subagent {handle}"),
                    })?;
                Ok(CommandOutput::Subagent {
                    subagent: info_to_protocol(&info),
                })
            }
            Command::SubagentSpawn {
                session,
                agent_type,
                brief,
                context_mode,
            } => {
                let live = self.live_unarchived(&session)?;
                let sup = live.agent.subagents().ok_or_else(|| ProtocolError::Other {
                    message: "session has no children (it is a child itself)".into(),
                })?;
                let spawned = sup.spawn(
                    &agent_type,
                    &brief,
                    Some(match context_mode {
                        ContextMode::Fresh => crate::subagent::ContextMode::Fresh,
                        ContextMode::Compacted => crate::subagent::ContextMode::Compacted,
                        ContextMode::Fork => crate::subagent::ContextMode::Fork,
                    }),
                    "gui",
                );
                match spawned {
                    Ok(sp) => {
                        let info =
                            sup.state_info(&sp.handle)
                                .ok_or_else(|| ProtocolError::Other {
                                    message: "child vanished after spawn".into(),
                                })?;
                        Ok(CommandOutput::Subagent {
                            subagent: info_to_protocol(&info),
                        })
                    }
                    Err(e) => Err(ProtocolError::Other { message: e }),
                }
            }
            Command::SubagentMessage { handle, text } => {
                let session =
                    self.child_parent(&handle)
                        .ok_or_else(|| ProtocolError::NotFound {
                            what: format!("subagent {handle}"),
                        })?;
                let live = self.live_unarchived(&session)?;
                let sup = live.agent.subagents().ok_or_else(|| ProtocolError::Other {
                    message: "session has no children (it is a child itself)".into(),
                })?;
                // The GUI's send to a child: the user is the sender.
                sup.message(
                    &handle,
                    text.clone(),
                    Lane::Steering,
                    Some("user".to_owned()),
                )
                .map_err(|e| ProtocolError::Other { message: e })?;
                let info = sup
                    .state_info(&handle)
                    .ok_or_else(|| ProtocolError::NotFound {
                        what: format!("subagent {handle}"),
                    })?;
                Ok(CommandOutput::Subagent {
                    subagent: info_to_protocol(&info),
                })
            }
            Command::SubagentStop { handle } => {
                let session =
                    self.child_parent(&handle)
                        .ok_or_else(|| ProtocolError::NotFound {
                            what: format!("subagent {handle}"),
                        })?;
                let live = self.live_unarchived(&session)?;
                let sup = live.agent.subagents().ok_or_else(|| ProtocolError::Other {
                    message: "session has no children (it is a child itself)".into(),
                })?;
                sup.stop(&handle, StoppedBy::User)
                    .map_err(|e| ProtocolError::Other { message: e })?;
                let info = sup
                    .state_info(&handle)
                    .ok_or_else(|| ProtocolError::NotFound {
                        what: format!("subagent {handle}"),
                    })?;
                Ok(CommandOutput::Subagent {
                    subagent: info_to_protocol(&info),
                })
            }
            Command::SubagentActivity { handle } => {
                let session =
                    self.child_parent(&handle)
                        .ok_or_else(|| ProtocolError::NotFound {
                            what: format!("subagent {handle}"),
                        })?;
                let live = self.live(&session)?;
                let sup = live.agent.subagents().ok_or_else(|| ProtocolError::Other {
                    message: "session has no children (it is a child itself)".into(),
                })?;
                let activity =
                    sup.activity_window(&handle)
                        .ok_or_else(|| ProtocolError::NotFound {
                            what: format!("subagent {handle}"),
                        })?;
                Ok(CommandOutput::SubagentActivity { activity })
            }

            _ => unreachable!("dispatch routes the arm"),
        }
    }
}
