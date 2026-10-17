# Prototype: Concierge (dialogue-first)

Single-file prototype: `app/prototype-concierge.html`.

## The idea

The third paradigm, after **board v2** (space: fleet columns) and **pulse** (time: one event
stream). Concierge inverts both: the main view is a *conversation with the harness*.

You don't navigate a tree of sessions or scan a feed. You talk to a chief of staff that
watches the whole fleet, and it talks back — reporting state as **live inline cards** in the
thread, and acting on plain-language instructions.

```
┌──────────────────────────────────────────────────────────────────────┐
│  τ concierge                                              (⌘K)  ⌘K  │
├──────────────────────────────────────────────┬───────────────────────┤
│  one conversation · the whole fleet          │  FLEET · 8            │
│                                              │  TAU                  │
│   you:  what's going on?                     │   CSS architecture…   │
│   τ:    [fleet grid of live session cards]   │   Refactor store…     │
│   τ:    [Input remap pass — running]  (card) │   Wayfinder map…  ◉   │
│   you:  open the wayfinder session           │  BLOODY-CHAIRS        │
│   τ:    [Wayfinder map #12 — needs] (card)   │   …                   │
│                                              │  PI                   │
│                                              │   …                   │
├──────────────────────────────────────────────┴───────────────────────┤
│  ask about the fleet, steer a session, spawn new work…      [send]  │
└──────────────────────────────────────────────────────────────────────┘
        [ session drawer slides in over the conversation when you go deep ]
```

## What's different from board v2 and pulse

| | board v2 | pulse | **concierge** |
|---|---|---|---|
| primary axis | space (columns by state) | time (event stream) | **dialogue** |
| home screen | fleet board | live feed | **a conversation** |
| how you act | click a card, switch view | click a feed row | **say what you want** |
| fleet state | cards in columns | rows in a stream | **inline cards + side rail** |
| attention | bell + sheet | pinned needs section | **card turns red in-thread + rail** |
| going deep | view switch | docked drawer | **docked drawer** |

The through-line concierge tests: **can natural language be the primary control surface**,
with the visual fleet as a *report* the harness produces, rather than the thing you navigate?

## The parts

- **One conversation** — your messages (right, blue) and the concierge's (left, τ avatar).
  The concierge's "messages" are mostly **live cards**, not prose.
- **Live inline cards** — a session rendered in-thread: name, workspace chip, state, model,
  tokens, cost, latest activity, and `open transcript` / `stop`. Cards re-render from live
  state, so a card in the middle of the thread reflects *now*, not when it was posted.
- **Fleet grid** — a "what's going on?" answer is a 2-column grid of the running/needs
  sessions, inline in the thread.
- **Done notices** — compact green rows when a session completes, with token/cost delta.
- **Fleet rail** (right, persistent) — every session grouped by workspace, state dot +
  activity, needs-you tinted red. The always-on "where is everything" glance.
- **Session drawer** — slide-in over the conversation (not a view switch): transcript,
  steering/follow-up queue, composer with a lane selector. The conversation keeps flowing
  behind it.
- **Plain-language routing** — the composer parses intent (`open …`, `what's going on`,
  `status`, …) and acts. A stand-in for a real routing model.
- **Command palette** (⌘K) — ask / open-session / view groups, as the power path.

## The one bug worth remembering

A scripted demo running on a 100 ms tick **clobbered user input**: it kept advancing state
and firing steps even while you typed — it only skipped the re-render. The fix is to **freeze
the whole demo while the user is focused in any input**, not just skip the render. (Same
family as the pulse "reversed typing" bug — a background re-render is never friend with a
focused input.)

## Honest read

Concierge is the boldest of the three and the least proven. Natural-language-as-control is
compelling for *directing* work ("have the researcher also check Zed's setup"), but it's a
weaker primary surface for *scanning* state — the fleet rail and inline cards exist precisely
because you still need the at-a-glance view that a conversation thread doesn't give you.

The likely synthesis: **the conversation is the home / command surface; the fleet rail is the
persistent awareness layer; the drawer is the deep-work surface.** That's a coherent single
mental model — *talk to your harness, glance at the fleet, dive when needed* — and it's
genuinely different from both the board and the feed.
