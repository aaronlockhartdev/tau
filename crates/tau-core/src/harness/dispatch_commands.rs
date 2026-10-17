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
                    let lane = if lane == MessageLane::Force {
                        link.stop()
                            .map_err(|e| ProtocolError::Other { message: e })?;
                        Lane::Steering
                    } else {
                        lane_to_lane(lane)
                    };
                    link.send(text.clone(), lane)
                        .map_err(|e| ProtocolError::Other { message: e })?;
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
                    subagents: sup
                        .handles()
                        .iter()
                        .filter_map(|h| sup.state_info(h).map(|i| info_to_protocol(&i)))
                        .collect(),
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
                sup.message(&handle, text.clone(), Lane::Steering)
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

            _ => unreachable!("dispatch routes the arm"),
        }
    }
}
