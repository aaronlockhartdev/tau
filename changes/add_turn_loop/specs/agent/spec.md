# Delta: agent

Apply to `spec/agent/spec.md` at archive time, for a domain with no children. Describe the difference, not the whole file.

## ADDED Requirements

### R-agent-01  The loop commits each closed block

WHEN the provider stream closes a block THE SYSTEM SHALL commit that block.
Acceptance: @R-agent-01
Why: a closed block is a finished fact and a delta is not, so the close is where a row can exist.

### R-agent-02  A delta never commits

THE SYSTEM SHALL record no token delta as a block.
Acceptance: @R-agent-02
Why: a request streams hundreds of deltas, and each commit would land a prefix of the same answer in the index.

### R-agent-03  The context is sent whole

THE SYSTEM SHALL send the context built from the log, and never provider-side conversation state.
Acceptance: @R-agent-03
Why: a provider's stored chain expires on its retention window, and the provider then holds the context the log owns.

### R-agent-04  A tool call is answered

WHEN a tool call block commits THE SYSTEM SHALL commit a result block for it, naming the reason when the tool did not run.
Acceptance: @R-agent-04
Why: a call with no result is refused by the provider, so the session cannot continue.

### R-agent-05  A cancel commits what is complete

WHEN a run is cancelled mid-stream THE SYSTEM SHALL commit the text received so far, before the request's
reason is stamped.
Acceptance: @R-agent-05
Why: the text that arrived is what the user saw, and a cancel that drops it leaves no record of an answer.

### R-agent-06  A cancel leaves no open call

WHEN a run is cancelled THE SYSTEM SHALL commit no truncated tool call.
Acceptance: @R-agent-06
Why: a call left open poisons the next request, which the provider rejects.

### R-agent-07  A bare request is recorded

WHEN a provider request fails before any block THE SYSTEM SHALL record the request row with its reason.
Acceptance: @R-agent-07
Why: a request that spent a budget and returned nothing is a fact about the session, not an absence.

### R-agent-08  A stream gap fails the request

IF a provider stream skips a number in its sequence THEN THE SYSTEM SHALL fail the request with the stream-gap reason.
Acceptance: @R-agent-08
Why: a stream with a hole in it is an answer with a piece missing, and no reader can tell which piece.

### R-agent-09  Resume closes an open request

WHEN a session resumes and the newest request has no reason THE SYSTEM SHALL close it as interrupted before a new request starts.
Acceptance: @R-agent-09
Why: an unclosed request row reads as a stream still running, and a reader would wait on a dead one.

### R-agent-10  Steering lands at the boundary

WHEN steering input arrives during a run THE SYSTEM SHALL record it as a user input block at the next request boundary, in arrival order.
Acceptance: @R-agent-10
Why: recording at arrival places the input before the output it did not cause.

### R-agent-11  The steering queue is bounded

THE SYSTEM SHALL refuse steering input past the configured queue cap.
Acceptance: @R-agent-11
Why: an unbounded queue lets a user type faster than a session can answer, and the input waits forever.

### R-agent-12  Stop conditions come from config

THE SYSTEM SHALL stop the loop at the configured token budget, wall clock limit, or repeated tool call, and record the reason.
Acceptance: @R-agent-12
Why: a loop with no ceiling spends a budget on a session that has stopped making progress.

### R-agent-13  Injections land in named slots

WHEN the loop assembles a context THE SYSTEM SHALL place each injection in its named slot, stable slots first and volatile last.
Acceptance: @R-agent-13
Why: a byte change before the cache breakpoint rewrites the whole prefix; a tail change costs nothing.

### R-agent-14  An injection records its bytes

WHEN an injection reaches the model THE SYSTEM SHALL record a block naming its slot and the bytes sent; an
observation block is that record, not a second one.
Acceptance: @R-agent-14
Why: pointers rebuild the context only if every byte sent is in the log.

### R-agent-15  A session opens with its output

WHEN a session opens THE SYSTEM SHALL emit its session start output before any block.
Acceptance: @R-agent-15
Why: a session that writes a store into the user's tree owes the user a line saying so.
