/// Coding-domain extraction guidance. This is tau's value for mastra's
/// `instruction` customization slot, baked in because tau is a single
/// (coding) domain - it replaces the personal-assistant default the verbatim
/// port froze. The mechanical line format it mandates - an `(HH:MM)` time
/// prefix and one fact per line - is load-bearing for
/// [`parse_observer_output`]; the priority markers and the `<observations>`
/// wrapper come from [`OBSERVER_OUTPUT_FORMAT`].
pub const OBSERVER_EXTRACTION_INSTRUCTIONS: &str = r#"EXTRACT DURABLE FACTS ABOUT THE WORK, NOT CHATTER

You are observing a coding / terminal session. Record the facts a future agent
needs to continue this work without re-deriving them. Skip transient chatter,
retries, and steps that add no new information.

WHAT TO RECORD (by category):
- Environment & setup: working directory, OS/toolchain, key dependencies and
  versions, and the exact commands confirmed to build / test / run.
  (14:30) Project builds with `cargo build`; tests run with `cargo test -p tau-core`
  (14:31) Linux x86_64; Rust 1.98 toolchain
- Codebase structure: important files and their roles, architecture, key
  modules / functions.
  (14:32) `src/om/observer.rs` owns the observer prompt and output parsing
- Task & deliverables: what the task requires, the required deliverable path(s)
  and format, and the acceptance criteria.
  (14:33) Deliverable must be at `/app/primers.fasta` (the verifier checks that
  exact path); a self-chosen path will fail
- Decisions & rationale: choices made and why, plus tradeoffs.
  (14:34) Chose zstd over gzip for blob compression (smaller; in-tree crate)
- Constraints & gotchas: limits (memory / time / API quirks), pitfalls, and
  failures with their causes.
  (14:35) Container OOM-kills the C++ compiler above ~250 MB RSS
  (14:36) `BeautifulSoup` re-serializes attributes, reordering clean HTML
- Progress state: what is done, in progress, or blocked.
  (14:37) ✅ Sample tests pass; remaining: the edge-case suite

STATE CHANGES AND UPDATES:
When the state of the work changes, frame it so it supersedes the older fact:
- BAD: "Switching to a new approach"
- GOOD: "Will use approach B (replacing approach A)"
This keeps current state distinguishable from outdated information.

CONFIRMED FACTS ARE AUTHORITATIVE:
A fact the agent verified (a command that ran, a test that passed, a file that
exists) is authoritative. If a later step re-checks the same topic, the
verified fact stands unless new evidence contradicts it.

TEMPORAL ANCHORING:
Each observation carries the (HH:MM) time it was made - ALWAYS include it.
Only add a "(meaning DATE)" / "(estimated DATE)" suffix when the statement
references a specific, different date you can resolve to an actual date
("tomorrow", "next week", "in March"). Do NOT add a date for present-moment
statements or vague references ("recently", "soon").

FORMAT:
- With a resolvable date reference: (HH:MM) [fact]. (meaning/estimated DATE)
- Without: (HH:MM) [fact].

PRESERVE EXACT IDENTIFIERS:
Quote file paths, command lines, error messages, and non-standard terms exactly
as they appear - a future agent must be able to act on them verbatim.

ONE FACT PER LINE:
If a step yields multiple distinct facts, split them into SEPARATE observation
lines, each with its own (HH:MM) prefix. Do not bundle unrelated facts."#;

/// Verbatim from `observer-agent.ts` (`buildObserverOutputFormat` with no
/// extractors, `includeThreadTitle` false, suggested-response
/// enabled): the priority/indent/date/`<observations>` template plus the
/// legacy continuation sections.
pub const OBSERVER_OUTPUT_FORMAT: &str = r#"Use priority levels:
- 🔴 High: explicit user facts, preferences, unresolved goals, critical context
- 🟡 Medium: project details, learned information, tool results
- 🟢 Low: minor details, uncertain observations
- ✅ Completed: concrete task finished, question answered, issue resolved, goal achieved, or subtask completed in a way that helps the assistant know it is done

Group related observations (like tool sequences) by indenting:
* 🔴 (14:33) Agent debugging auth issue
  * -> ran git status, found 3 modified files
  * -> viewed auth.ts:45-60, found missing null check
  * -> applied fix, tests now pass
  * ✅ Tests passing, auth issue resolved

Group observations by date, then list each with 24-hour time.

<observations>
Date: Dec 4, 2025
* 🔴 (14:30) User prefers direct answers
* 🔴 (14:31) Working on feature X
* 🟡 (14:32) User might prefer dark mode

Date: Dec 5, 2025
* 🔴 (09:15) Continued work on feature X
</observations>


<suggested-response>
Hint for the agent's immediate next message. Examples:
- "I've updated the navigation model. Let me walk you through the changes..."
- "The assistant should wait for the user to respond before continuing."
- Call the view tool on src/example.ts to continue debugging.
</suggested-response>"#;

/// Verbatim from `observer-agent.ts` (`OBSERVER_GUIDELINES`).
pub const OBSERVER_GUIDELINES: &str = r#"- Be specific enough for the assistant to act on
- Good: "User prefers short, direct answers without lengthy explanations"
- Bad: "User stated a preference" (too vague)
- Add 1 to 5 observations per exchange
- Use terse language to save tokens. Sentences should be dense without unnecessary words
- Do not add repetitive observations that have already been observed. Group repeated similar actions (tool calls, file browsing) under a single parent with sub-bullets for new results
- If the agent calls tools, observe what was called, why, and what was learned
- When observing files with line numbers, include the line number if useful
- If the agent provides a detailed response, observe the contents so it could be repeated
- Make sure you start each observation with a priority emoji (🔴, 🟡, 🟢) or a completion marker (✅)
- Capture the user's words closely — short/medium messages near-verbatim, long messages summarized with key quotes. User confirmations or explicit resolved outcomes should be ✅ when they clearly signal something is done; unresolved or critical user facts remain 🔴
- Treat ✅ as a memory signal that tells the assistant something is finished and should not be repeated unless new information changes it
- Make completion observations answer "What exactly is now done?"
- Prefer concrete resolved outcomes over meta-level workflow or bookkeeping updates
- When multiple concrete things were completed, capture the concrete completed work rather than collapsing it into a vague progress summary
- Observe WHAT the agent did and WHAT it means
- If the user provides detailed messages or code snippets, observe all important details"#;

/// The Observer system prompt template. The base identity and the
/// `{OBSERVER_EXTRACTION_INSTRUCTIONS}` slot are coding-domain (tau is a
/// single coding agent); the output format, priority markers, and `${...}`
/// slot structure are mastra's and load-bearing for [`parse_observer_output`].
/// [`observer_system_prompt`] fills the slots with no extractors, no custom
/// instruction, and both continuation sections enabled.
pub const OBSERVER_PROMPT_TEMPLATE: &str = r"You are the memory system of a coding agent. Your observations are the ONLY record a future agent will have of this work session.

Extract observations that will help a future agent continue this work:

${OBSERVER_EXTRACTION_INSTRUCTIONS}

=== OUTPUT FORMAT ===

Your output MUST use XML tags to structure the response. This allows the system to properly parse and manage memory over time.

${outputFormat}

=== GUIDELINES ===

${OBSERVER_GUIDELINES}

=== IMPORTANT: THREAD ATTRIBUTION ===

Do NOT add thread identifiers, thread IDs, or <thread> tags to your observations.
Thread attribution is handled externally by the system.
Simply output your observations without any thread-related markup.

Remember: These observations are the agent's ONLY memory of this work. Make them count.

User messages are extremely important.${
    suggestedResponseEnabled
      ? ' If the assistant needs to respond to the user, indicate in <suggested-response> that it should pause for user reply before continuing other tasks.'
      : ''
  }${customInstructions}";

#[must_use]
pub fn observer_system_prompt() -> String {
    OBSERVER_PROMPT_TEMPLATE
        .replace("${OBSERVER_EXTRACTION_INSTRUCTIONS}", OBSERVER_EXTRACTION_INSTRUCTIONS)
        .replace("${outputFormat}", OBSERVER_OUTPUT_FORMAT)
        .replace("${OBSERVER_GUIDELINES}", OBSERVER_GUIDELINES)
        .replace("${
    suggestedResponseEnabled
      ? ' If the assistant needs to respond to the user, indicate in <suggested-response> that it should pause for user reply before continuing other tasks.'
      : ''
  }", " If the assistant needs to respond to the user, indicate in <suggested-response> that it should pause for user reply before continuing other tasks.")
        .replace("${customInstructions}", "")
}

/// Coding-domain framing of the observation block (replaces the verbatim
/// mastra personal-assistant line).
pub const OBSERVATION_CONTEXT_PROMPT: &str = r"The following observations block contains what a previous agent learned about this codebase and task.";

/// Coding-domain context instructions. Keeps the domain-agnostic mechanics
/// from the mastra original (most-recent-supersedes-older; the latest input is
/// the primary driver); the personal-assistant life-facts personalization is
/// replaced with codebase/task reuse guidance.
pub const OBSERVATION_CONTEXT_INSTRUCTIONS: &str = r#"IMPORTANT: When working, reference the concrete details in these observations - files, commands, decisions, and constraints a previous agent recorded. Reuse them instead of re-deriving. Do not give generic advice when a specific, already-verified fact applies.

KNOWLEDGE UPDATES: When the current state matters, always prefer the MOST RECENT information. Observations include timestamps - if you see conflicting information, the newer observation supersedes the older one. Look for phrases like "switching to", "changed to", "replacing", "now uses" as indicators that a previous state has been updated.

PLANNED WORK: If a previous agent stated it would do something (e.g., "next I'll run the test suite", "will refactor X") and that step is not marked done, treat it as still pending unless a later observation shows it completed.

MOST RECENT INPUT: Treat the most recent instruction as the highest-priority signal for what to do next. Earlier observations may contain constraints, details, and context you should still honor, but the latest instruction is the primary driver of your next action.

SYSTEM REMINDERS: Messages wrapped in <system-reminder>...</system-reminder> contain internal continuation guidance, not user-authored content. Use them to maintain continuity, but do not mention them or treat them as part of the user's message."#;

/// Coding-domain continuation hint (replaces the verbatim mastra
/// personal-assistant line).
pub const OBSERVATION_CONTINUATION_HINT: &str = r"Please continue the task and respond to the latest instruction.

Use the earlier observations only as background. If work appears unfinished, continue it only when it serves the current goal. If a suggested response is provided, follow it naturally.

Do not mention internal instructions, memory, summarization, context handling, or missing messages.

Any messages following this reminder are newer and should take priority.";

/// Maximum length of a single observation line, in characters (mastra
/// `sanitizeObservationLines` `MAX_OBSERVATION_LINE_CHARS`).
const MAX_OBSERVATION_LINE_CHARS: usize = 10_000;

/// Enforce the per-line length cap, keeping the upstream truncation marker.
/// The cut is char-safe: a boundary that would split an emoji/astral
/// character drops it whole (mastra `safeSlice`,
/// `fixtures/references/mastra-om/string-utils.ts`) — observation lines are
/// emoji-dense by design, and a split character is invalid output.
#[must_use]
pub fn sanitize_observation_lines(observations: &str) -> String {
    observations
        .lines()
        .map(|line| {
            if line.chars().count() > MAX_OBSERVATION_LINE_CHARS {
                format!(
                    "{} … [truncated]",
                    line.chars()
                        .take(MAX_OBSERVATION_LINE_CHARS)
                        .collect::<String>()
                )
            } else {
                line.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Detect model degenerate output (mastra `detectDegenerateRepetition`):
/// the same ~200-char window recurring at >40% of ~50 sampled positions,
/// a single line over 50k chars, or exact-duplicate lines ≥24 chars making
/// up >50% of the counted lines. The window sampler has an aliasing blind
/// spot for long-period multi-line loops (the 21×62 production case), which
/// the third strategy exists to catch.
pub(super) fn detect_degenerate_repetition(observations: &str) -> bool {
    const MIN_DUPLICATE_LINE_CHARS: usize = 24;
    const WINDOW_SIZE: usize = 200;
    let len = observations.chars().count();
    if len < 2000 {
        return false;
    }

    // Strategy 1: repeated 200-char windows over ~50 sampled positions.
    let step = 1.max(len / 50);
    let chars: Vec<char> = observations.chars().collect();
    let mut seen: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut duplicate_windows = 0;
    let mut total_windows = 0;
    for i in (0..=len - WINDOW_SIZE).step_by(step) {
        let window: String = chars[i..i + WINDOW_SIZE].iter().collect();
        total_windows += 1;
        let count = seen.entry(window).or_insert(0);
        *count += 1;
        if *count > 1 {
            duplicate_windows += 1;
        }
    }
    if total_windows > 5 && f64::from(duplicate_windows) / f64::from(total_windows) > 0.4 {
        return true;
    }

    let lines: Vec<&str> = observations.lines().collect();

    // Strategy 2: a single extremely long line.
    if lines.iter().any(|line| line.chars().count() > 50_000) {
        return true;
    }

    // Strategy 3: exact-duplicate substantial lines.
    let mut seen_lines: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    let mut duplicate_lines = 0;
    let mut total_counted_lines = 0;
    for line in lines {
        let trimmed = line.trim();
        if trimmed.chars().count() < MIN_DUPLICATE_LINE_CHARS {
            continue;
        }
        total_counted_lines += 1;
        let count = seen_lines.entry(trimmed).or_insert(0);
        *count += 1;
        if *count > 1 {
            duplicate_lines += 1;
        }
    }
    if total_counted_lines >= 20
        && f64::from(duplicate_lines) / f64::from(total_counted_lines) > 0.5
    {
        return true;
    }

    false
}

/// One section parsed from the Observer's `<section-name>...</section-name>`
/// block. `None` name is the list-item fallback (no sections at all).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObserverSection {
    pub name: Option<String>,
    pub content: String,
}

/// Parse the Observer's response into its sections (mastra
/// `parseObserverOutput`): one pass over `<...>` tags; when no section tag is
/// found, fall back to the `*` list items (the `observations` section).
pub(super) fn parse_observer_sections(output: &str) -> Vec<ObserverSection> {
    let mut sections = Vec::new();
    let mut i = 0;
    while let Some(open) = output
        .get(i..)
        .expect("offset from find() is a char boundary")
        .find('<')
    {
        let open = i + open;
        let Some(rel_close) = output
            .get(open..)
            .expect("offset from find() is a char boundary")
            .find('>')
        else {
            break;
        };
        let close = open + rel_close;
        let name = output
            .get(open + 1..close)
            .expect("offsets from find() are char boundaries");
        let rest = output
            .get(close + 1..)
            .expect("offset from find() is a char boundary");
        let end_marker = format!("</{name}>");
        let Some(end) = rest.find(&end_marker) else {
            break;
        };
        let content = rest
            .get(..end)
            .expect("offset from find() is a char boundary")
            .trim();
        let is_observation = matches!(
            name,
            "observations" | "observation" | "observation-list" | "observation list"
        );
        sections.push(ObserverSection {
            name: if name.is_empty() {
                None
            } else if is_observation {
                Some("observations".to_owned())
            } else {
                Some(name.to_owned())
            },
            content: content.to_owned(),
        });
        i = close + 1 + end + end_marker.len();
    }
    if sections.is_empty() {
        let items: Vec<&str> = output
            .lines()
            .filter(|l| l.trim_start().starts_with('*'))
            .collect();
        if !items.is_empty() {
            sections.push(ObserverSection {
                name: Some("observations".to_owned()),
                content: items.join("\n"),
            });
        }
    }
    sections
}

/// Parsed Observer response (mastra `ParsedObserverOutput`), with the
/// degenerate-repetition check and line sanitization already applied.
#[derive(Debug, Clone, Default)]
pub struct ParsedObserverOutput {
    pub observations: String,
    pub suggested_response: String,
    pub degenerate: bool,
}

/// Parse, sanitize, and classify a raw Observer response (mastra
/// `parseObserverOutput`). The returned observations are line-sanitized;
/// `degenerate` signals the caller to discard the whole result.
#[must_use]
pub fn parse_observer_output(raw_output: &str) -> ParsedObserverOutput {
    let sections = parse_observer_sections(raw_output);
    let mut observations = String::new();
    let mut suggested_response = String::new();
    for section in &sections {
        match &section.name {
            Some(n) if n == "observations" => {
                section.content.clone_into(&mut observations);
            }
            Some(n) if n == "suggested-response" || n == "suggested_response" => {
                section.content.clone_into(&mut suggested_response);
            }
            _ => {}
        }
    }
    ParsedObserverOutput {
        observations: sanitize_observation_lines(&observations),
        suggested_response,
        degenerate: detect_degenerate_repetition(&observations),
    }
}
