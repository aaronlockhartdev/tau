# Sub-agents and task hand-off are native to the core

Pi deliberately ships without sub-agents (its philosophy: "No sub-agents" — build them with extensions or tmux). Tau diverges: sub-agents and task hand-off are first-class features of the core, not an extension. Rationale: delegation is central to the workflows tau targets, and the GUI can present sub-agent sessions cleanly — a gap pi and its sub-agent extensions have. The concrete sub-agent/task model (what a task is, hand-off protocol, session topology) is open on the map.

**Considered**: pi-style minimalism (sub-agents as an extension or out of scope). Rejected: the GUI's ability to show nested sessions is tau's differentiator, and that requires the core to own sub-agent sessions.
