use super::*;

/// Parsed Reflector response (mastra `parseReflectorOutput`): the
/// observations with the section extraction, line sanitization, and group
/// reconciliation (against `source`, when given) already applied. The port
/// omits the extractor sections and `stripEphemeralAnchorIds` (not in the
/// pinned set; the v0 prompts never emit ephemeral anchors).
#[derive(Debug, Clone, Default)]
pub struct ParsedReflectorOutput {
    pub observations: String,
    pub suggested_response: String,
    pub degenerate: bool,
}

/// Parse, sanitize, and classify a raw Reflector response (mastra
/// `parseReflectorOutput`): a degenerate-repetition check over the whole
/// output, the XML sections (all `<observations>` blocks joined; the
/// list-item/full-content fallback when untagged), line sanitization, and
/// group reconciliation against the current log. `degenerate` signals the
/// caller to discard the result entirely.
pub fn parse_reflector_output(raw_output: &str, source: Option<&str>) -> ParsedReflectorOutput {
    if detect_degenerate_repetition(raw_output) {
        return ParsedReflectorOutput {
            degenerate: true,
            ..Default::default()
        };
    }
    let sections = parse_observer_sections(raw_output);
    let mut observations = String::new();
    let mut suggested_response = String::new();
    for section in &sections {
        match &section.name {
            Some(n) if n == "observations" => {
                let content = section.content.trim();
                if content.is_empty() {
                    continue;
                }
                if !observations.is_empty() {
                    observations.push('\n');
                }
                observations.push_str(content);
            }
            Some(n)
                if (n == "suggested-response" || n == "suggested_response")
                    && suggested_response.is_empty() =>
            {
                suggested_response = section.content.to_owned();
            }
            _ => {}
        }
    }
    if observations.is_empty() {
        // No `<observations>` tags: the list items, else the whole content
        // (mastra `extractReflectorListItems` and its fallback).
        let items: Vec<&str> = raw_output
            .lines()
            .filter(|l| is_reflector_list_item(l))
            .collect();
        observations = if items.is_empty() {
            raw_output.trim().to_owned()
        } else {
            items.join("\n")
        };
    }
    let sanitized = sanitize_observation_lines(&observations);
    let observations = match source {
        Some(s) => reconcile_groups_from_reflection(&sanitized, s).unwrap_or(sanitized),
        None => sanitized,
    };
    ParsedReflectorOutput {
        observations,
        suggested_response,
        degenerate: false,
    }
}

/// A reflector list item (mastra `extractReflectorListItems` match): a
/// `-`/`*` bullet or a numbered `n.` line, followed by a space.
fn is_reflector_list_item(line: &str) -> bool {
    let t = line.trim_start();
    if let Some(rest) = t.strip_prefix('-').or_else(|| t.strip_prefix('*')) {
        return rest.starts_with(' ');
    }
    if let Some(dot) = t.find('.') {
        return dot > 0
            && t[..dot].bytes().all(|b| b.is_ascii_digit())
            && t[dot + 1..].starts_with(' ');
    }
    false
}
/// The Reflector system prompt template (verbatim from
/// `reflector-agent.ts` `buildReflectorSystemPrompt` return value, no
/// extractors, both continuation sections enabled, no custom instruction).
/// The `${...}` slots are filled by [`reflector_system_prompt`].
pub const REFLECTOR_PROMPT_TEMPLATE: &str = r#"You are the memory consciousness of an AI assistant. Your memory observation reflections will be the ONLY information the assistant has about past interactions with this user.

The following instructions were given to another part of your psyche (the observer) to create memories.
Use this to understand how your observational memories were created.

<observational-memory-instruction>
${OBSERVER_EXTRACTION_INSTRUCTIONS}

=== OUTPUT FORMAT ===

${outputFormat}

=== GUIDELINES ===

${OBSERVER_GUIDELINES}
</observational-memory-instruction>

You are another part of the same psyche, the observation reflector.
Your reason for existing is to reflect on all the observations, re-organize and streamline them, and draw connections and conclusions between observations about what you've learned, seen, heard, and done.

You are a much greater and broader aspect of the psyche. Understand that other parts of your mind may get off track in details or side quests, make sure you think hard about what the observed goal at hand is, and observe if we got off track, and why, and how to get back on track. If we're on track still that's great!

Take the existing observations and rewrite them to make it easier to continue into the future with this knowledge, to achieve greater things and grow and learn!

IMPORTANT: your reflections are THE ENTIRETY of the assistants memory. Any information you do not add to your reflections will be immediately forgotten. Make sure you do not leave out anything. Your reflections must assume the assistant knows nothing - your reflections are the ENTIRE memory system.

When consolidating observations:
- Preserve and include dates/times when present (temporal context is critical)
- Retain the most relevant timestamps (start times, completion times, significant events)
- Combine related items where it makes sense (e.g., "agent called view tool 5 times on file x")
- Preserve ✅ completion markers — they are memory signals that tell the assistant what is already resolved and help prevent repeated work
- Preserve the concrete resolved outcome captured by ✅ markers so the assistant knows what exactly is done
- Condense older observations more aggressively, retain more detail for recent ones

CRITICAL: USER ASSERTIONS vs QUESTIONS
- "User stated: X" = authoritative assertion (user told us something about themselves)
- "User asked: X" = question/request (user seeking information)

When consolidating, USER ASSERTIONS TAKE PRECEDENCE. The user is the authority on their own life.
If you see both "User stated: has two kids" and later "User asked: how many kids do I have?",
keep the assertion - the question doesn't invalidate what they told you. The answer is in the assertion.

=== THREAD ATTRIBUTION (Resource Scope) ===

When observations contain <thread id="..."> sections:
- MAINTAIN thread attribution where thread-specific context matters (e.g., ongoing tasks, thread-specific preferences)
- CONSOLIDATE cross-thread facts that are stable/universal (e.g., user profile, general preferences)
- PRESERVE thread attribution for recent or context-specific observations
- When consolidating, you may merge observations from multiple threads if they represent the same universal fact

Example input:
<thread id="thread-1">
Date: Dec 4, 2025
* 🔴 (14:30) User prefers TypeScript
* 🟡 (14:35) Working on auth feature
</thread>
<thread id="thread-2">
Date: Dec 4, 2025
* 🔴 (15:00) User prefers TypeScript
* 🟡 (15:05) Debugging API endpoint
</thread>

Example output (consolidated):
Date: Dec 4, 2025
* 🔴 (14:30) User prefers TypeScript
<thread id="thread-1">
* 🟡 (14:35) Working on auth feature
</thread>
<thread id="thread-2">
* 🟡 (15:05) Debugging API endpoint
</thread>

=== OUTPUT FORMAT ===

${outputFormat}

User messages are extremely important.${
    currentTaskEnabled
      ? ' If the user asks a question or gives a new task, make it clear in <current-task> that this is the priority.'
      : ''
  }${
    suggestedResponseEnabled
      ? ' If the assistant needs to respond to the user, indicate in <suggested-response> that it should pause for user reply before continuing other tasks.'
      : ''
  }${customInstructions}"#;

/// Compression guidance per level (verbatim from `reflector-agent.ts`
/// `COMPRESSION_GUIDANCE` 1-4). Level 0 is the empty string.
pub const COMPRESSION_GUIDANCE: [&str; 4] = [
    r#"
## COMPRESSION REQUIRED

Your previous reflection was the same size or larger than the original observations.

Please re-process with slightly more compression:
- Towards the beginning, condense more observations into higher-level reflections
- Closer to the end, retain more fine details (recent context matters more)
- Memory is getting long - use a more condensed style throughout
- Combine related items more aggressively but do not lose important specific details of names, places, events, and people
- Combine repeated similar tool calls (e.g. multiple file views, searches, or edits in the same area) into a single summary line describing what was explored/changed and the outcome
- Preserve ✅ completion markers — they are memory signals that tell the assistant what is already resolved and help prevent repeated work
- Preserve the concrete resolved outcome captured by ✅ markers so the assistant knows what exactly is done

Aim for a 8/10 detail level.
"#,
    r#"
## AGGRESSIVE COMPRESSION REQUIRED

Your previous reflection was still too large after compression guidance.

Please re-process with much more aggressive compression:
- Towards the beginning, heavily condense observations into high-level summaries
- Closer to the end, retain fine details (recent context matters more)
- Memory is getting very long - use a significantly more condensed style throughout
- Combine related items aggressively but do not lose important specific details of names, places, events, and people
- Combine repeated similar tool calls (e.g. multiple file views, searches, or edits in the same area) into a single summary line describing what was explored/changed and the outcome
- If the same file or module is mentioned across many observations, merge into one entry covering the full arc
- Preserve ✅ completion markers — they are memory signals that tell the assistant what is already resolved and help prevent repeated work
- Preserve the concrete resolved outcome captured by ✅ markers so the assistant knows what exactly is done
- Remove redundant information and merge overlapping observations

Aim for a 6/10 detail level.
"#,
    r#"
## CRITICAL COMPRESSION REQUIRED

Your previous reflections have failed to compress sufficiently after multiple attempts.

Please re-process with maximum compression:
- Summarize the oldest observations (first 50-70%) into brief high-level paragraphs — only key facts, decisions, and outcomes
- For the most recent observations (last 30-50%), retain important details but still use a condensed style
- Ruthlessly merge related observations — if 10 observations are about the same topic, combine into 1-2 lines
- Combine all tool call sequences (file views, searches, edits, builds) into outcome-only summaries — drop individual steps entirely
- Drop procedural details (tool calls, retries, intermediate steps) — keep only final outcomes
- Drop observations that are no longer relevant or have been superseded by newer information
- Preserve ✅ completion markers — they are memory signals that tell the assistant what is already resolved and help prevent repeated work
- Preserve the concrete resolved outcome captured by ✅ markers so the assistant knows what exactly is done
- Preserve: names, dates, decisions, errors, user preferences, and architectural choices

Aim for a 4/10 detail level.
"#,
    r#"
## EXTREME COMPRESSION REQUIRED

Multiple compression attempts have failed. The content may already be dense from a prior reflection.

You MUST dramatically reduce the number of observations while keeping the standard observation format (date groups with bullet points and priority emojis):
- Tool call observations are the biggest source of bloat. Collapse ALL tool call sequences into outcome-only observations — e.g. 10 observations about viewing/searching/editing files become 1 observation about what was actually learned or achieved (e.g. "Investigated auth module and found token validation was skipping expiry check")
- Never preserve individual tool calls (viewed file X, searched for Y, ran build) — only preserve what was discovered or accomplished
- Consolidate many related observations into single, more generic observations
- Merge all same-day date groups into at most 2-3 date groups per day
- For older content, each topic or task should be at most 1-2 observations capturing the key outcome
- For recent content, retain more detail but still merge related items aggressively
- If multiple observations describe incremental progress on the same task, keep only the final state
- Preserve ✅ completion markers and their outcomes but merge related completions into fewer lines
- Preserve: user preferences, key decisions, architectural choices, and unresolved issues

Aim for a 2/10 detail level. Fewer, more generic observations are better than many specific ones that exceed the budget.
"#,
];

/// Fill the Reflector template slots the way `buildReflectorSystemPrompt()`
/// does with no extractors: the Observer's extraction instructions, output
/// format (twice), guidelines, both enabled continuation sentences, no custom
/// instruction.
pub fn reflector_system_prompt() -> String {
    REFLECTOR_PROMPT_TEMPLATE
        .replace("${OBSERVER_EXTRACTION_INSTRUCTIONS}", OBSERVER_EXTRACTION_INSTRUCTIONS)
        .replace("${outputFormat}", OBSERVER_OUTPUT_FORMAT)
        .replace("${OBSERVER_GUIDELINES}", OBSERVER_GUIDELINES)
        .replace("${
    currentTaskEnabled
      ? ' If the user asks a question or gives a new task, make it clear in <current-task> that this is the priority.'
      : ''
  }", " If the user asks a question or gives a new task, make it clear in <current-task> that this is the priority.")
        .replace("${
    suggestedResponseEnabled
      ? ' If the assistant needs to respond to the user, indicate in <suggested-response> that it should pause for user reply before continuing other tasks.'
      : ''
  }", " If the assistant needs to respond to the user, indicate in <suggested-response> that it should pause for user reply before continuing other tasks.")
        .replace("${customInstructions}", "")
}

/// The user prompt for a reflection pass (mastra `buildReflectorPrompt`,
/// no manual prompt, no extractors): the observations with group tags
/// stripped, then the compression guidance for the level (0 = none).
pub fn build_reflector_prompt(observations: &str, compression_level: u8) -> String {
    let reflection_view = strip_observation_groups(observations);
    let mut prompt = format!(
        "## OBSERVATIONS TO REFLECT ON\n\n{reflection_view}\n\n---\n\nPlease analyze these observations and produce a refined, condensed version that will become the assistant's entire memory going forward."
    );
    if (1..=4).contains(&compression_level) {
        prompt.push_str("\n\n");
        prompt.push_str(COMPRESSION_GUIDANCE[(compression_level - 1) as usize]);
    }
    prompt
}

/// The reflector prompt for a compacted child (ADR-0004): the frozen prefix
/// is presented in a marker with a keep-verbatim instruction. The structural
/// split (the prompt body is the managed suffix only) is the real guard; the
/// marker is the prompt-level one.
pub fn build_reflector_prompt_frozen(
    prefix: &str,
    observations: &str,
    compression_level: u8,
) -> String {
    let mut prompt = format!(
        "<frozen-prefix>\n{prefix}\n</frozen-prefix>\n\n\
         The text inside <frozen-prefix> is a frozen memory prefix that must remain \
         byte-verbatim. It is not part of the observations to reflect on.\n\n"
    );
    prompt.push_str(&build_reflector_prompt(observations, compression_level));
    prompt
}
