use super::*;

/// The seven task tools (spec §5.4): free text + ids in, core-enforced
/// transitions; diagnostics are results, never panics.
pub fn tool_call(store: &mut SessionStore, name: &str, args: &Value) -> String {
    match name {
        "task_create" => {
            let Some(title) = args.get("title").and_then(Value::as_str) else {
                return "task_create: missing \"title\"".into();
            };
            // Malformed items are rejected, not silently dropped (review
            // N6): a step without its expected output is a quality-gate
            // failure the model must see.
            let steps = match args.get("steps").and_then(|v| v.as_array()) {
                Some(v) => {
                    let mut out = Vec::new();
                    for (i, s) in v.iter().enumerate() {
                        let Some(text) = s.get("text").and_then(Value::as_str) else {
                            return format!("task_create: step {i} is missing \"text\"");
                        };
                        let Some(expected) = s.get("expected_output").and_then(Value::as_str)
                        else {
                            return format!(
                                "task_create: step {i} (\"{text}\") is missing \"expected_output\""
                            );
                        };
                        out.push(Step {
                            text: text.to_owned(),
                            expected_output: expected.to_owned(),
                            status: StepStatus::Pending,
                        });
                    }
                    out
                }
                None => Vec::new(),
            };
            let criteria = match args.get("criteria").and_then(|v| v.as_array()) {
                Some(v) => {
                    let mut out = Vec::new();
                    for (i, c) in v.iter().enumerate() {
                        let Some(text) = c.as_str() else {
                            return format!("task_create: criterion {i} is not a string");
                        };
                        out.push(Criterion {
                            text: text.to_owned(),
                            status: CriterionStatus::Pending,
                        });
                    }
                    out
                }
                None => Vec::new(),
            };
            let n = load(store)
                .map(|t| t.iter().filter(|t| t.id.starts_with("task-")).count())
                .unwrap_or(0)
                + 1;
            let id = format!("task-{n}");
            match create(store, &id, title, steps.clone(), criteria.clone()) {
                Ok(()) => format!(
                    "created {id}: {title} ({} steps, {} criteria)",
                    steps.len(),
                    criteria.len()
                ),
                Err(e) => e,
            }
        }
        "task_start" => {
            let Some(id) = task_id(args) else {
                return "task_start: missing \"task\"".into();
            };
            match start(store, id) {
                Ok(t) => report(&t),
                Err(e) => e,
            }
        }
        "task_evidence" => {
            let Some(id) = task_id(args) else {
                return "task_evidence: missing \"task\"".into();
            };
            let Some(criterion) = args.get("criterion").and_then(Value::as_str) else {
                return "task_evidence: missing \"criterion\"".into();
            };
            let Some(summary) = args.get("summary").and_then(Value::as_str) else {
                return "task_evidence: missing \"summary\"".into();
            };
            let ev = Evidence {
                criterion: criterion.to_owned(),
                summary: summary.to_owned(),
                command: args
                    .get("command")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                artifact: args
                    .get("artifact")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                passed: args.get("passed").and_then(Value::as_bool).unwrap_or(true),
                step: args.get("step").and_then(Value::as_str).map(str::to_owned),
            };
            match add_evidence(store, id, ev) {
                Ok(t) => report(&t),
                Err(e) => e,
            }
        }
        "task_block" => {
            let Some(id) = task_id(args) else {
                return "task_block: missing \"task\"".into();
            };
            let Some(reason) = args.get("reason").and_then(Value::as_str) else {
                return "task_block: missing \"reason\"".into();
            };
            match block(
                store,
                id,
                reason,
                args.get("needs").and_then(Value::as_str).map(str::to_owned),
            ) {
                Ok(t) => report(&t),
                Err(e) => e,
            }
        }
        "task_finish" => {
            let Some(id) = task_id(args) else {
                return "task_finish: missing \"task\"".into();
            };
            match finish(
                store,
                id,
                args.get("force").and_then(Value::as_bool).unwrap_or(false),
                args.get("reason")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            ) {
                Ok(t) => report(&t),
                Err(e) => e,
            }
        }
        "task_cancel" => {
            let Some(id) = task_id(args) else {
                return "task_cancel: missing \"task\"".into();
            };
            match cancel(
                store,
                id,
                args.get("reason")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            ) {
                Ok(t) => report(&t),
                Err(e) => e,
            }
        }
        other => format!("unknown task tool {other}"),
    }
}

fn task_id(args: &Value) -> Option<&str> {
    args.get("task").and_then(Value::as_str)
}

fn report(t: &Task) -> String {
    let worker = t
        .worker
        .as_ref()
        .map(|w| format!(" → worker {} ({})", w.session, w.status));
    format!(
        "task {} {:?} [{}]{}",
        t.id,
        t.title,
        t.status,
        worker.as_deref().unwrap_or("")
    )
}
