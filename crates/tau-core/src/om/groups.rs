/// Provenance pointer from a compressed observation to the exact raw entries
/// it was derived from — the recall bookkeeping (spec §4; mastra
/// `observation-groups.ts`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservationGroup {
    /// 16-hex id.
    pub id: String,
    /// "startEntryId:endEntryId", or comma-joined segments for a merged span.
    pub range: String,
    /// "reflection" for reflected (re-wrapped) sections; absent for observer
    /// appends.
    pub kind: Option<String>,
    pub content: String,
}

/// Deterministic 16-hex group id (mastra uses `randomBytes(8)`; this module
/// is pure, so the id is a content hash — stable across reruns).
pub fn generate_group_id(source: &str) -> String {
    format!("{:016x}", xxhash_rust::xxh3::xxh3_64(source.as_bytes()))
}

/// Wrap an observation in its group tag (mastra `wrapInObservationGroup`).
pub fn wrap_in_observation_group(
    observations: &str,
    range: &str,
    id: &str,
    kind: Option<&str>,
) -> String {
    let kind_attr = kind.map(|k| format!(" kind=\"{k}\"")).unwrap_or_default();
    format!(
        "<observation-group id=\"{id}\" range=\"{range}\"{kind_attr}>\n{}\n</observation-group>",
        observations.trim()
    )
}

/// Extract all observation groups from a log (mastra `parseObservationGroups`).
pub fn parse_observation_groups(observations: &str) -> Vec<ObservationGroup> {
    let mut groups = Vec::new();
    let open_tag = "<observation-group ";
    let mut rest = observations;
    while let Some(start) = rest.find(open_tag) {
        let after_tag = &rest[start + open_tag.len()..];
        let Some(attr_end) = after_tag.find('>') else {
            break;
        };
        let attrs = &after_tag[..attr_end];
        let Some(close) = rest[start..].find("</observation-group>") else {
            break;
        };
        let content = &rest[start + open_tag.len() + attr_end + 1..start + close];
        let mut id: Option<&str> = None;
        let mut range: Option<&str> = None;
        let mut kind: Option<&str> = None;
        for attr in attrs.split_whitespace() {
            let Some((key, value)) = attr.split_once('=') else {
                continue;
            };
            let value = value.trim_matches('"');
            match key {
                "id" => id = Some(value),
                "range" => range = Some(value),
                "kind" => kind = Some(value),
                _ => {}
            }
        }
        if let (Some(id), Some(range)) = (id, range) {
            groups.push(ObservationGroup {
                id: id.to_owned(),
                range: range.to_owned(),
                kind: kind.map(str::to_owned),
                content: content.trim().to_owned(),
            });
        }
        rest = &rest[start + close + "</observation-group>".len()..];
    }
    groups
}

/// Remove the group tags, keeping the content (mastra `stripObservationGroups`).
pub fn strip_observation_groups(observations: &str) -> String {
    let mut out = observations.to_owned();
    let open_tag = "<observation-group ";
    while let Some(start) = out.find(open_tag) {
        let after_tag = &out[start + open_tag.len()..];
        let Some(attr_end) = after_tag.find('>') else {
            break;
        };
        let Some(close) = out[start..].find("</observation-group>") else {
            break;
        };
        let content = out[start + open_tag.len() + attr_end + 1..start + close]
            .trim()
            .to_owned();
        out = format!(
            "{}{}{}",
            &out[..start],
            content,
            &out[start + close + "</observation-group>".len()..]
        );
    }
    out.replace("\n\n\n", "\n\n").trim().to_owned()
}

/// Merge the ranges of several groups into one span (mastra
/// `combineObservationGroupRanges`): the span from the first segment's start
/// to the last segment's end (`first.start:last.end`).
pub fn combine_group_ranges(groups: &[ObservationGroup]) -> String {
    let mut segments: Vec<&str> = Vec::new();
    for group in groups {
        for segment in group.range.split(',') {
            let segment = segment.trim();
            if !segment.is_empty() {
                segments.push(segment);
            }
        }
    }
    match (segments.first(), segments.last()) {
        (Some(first), Some(last)) => format!(
            "{}:{}",
            first.split(':').next().map(str::trim).unwrap_or_default(),
            last.split(':')
                .next_back()
                .map(str::trim)
                .unwrap_or_default()
        ),
        _ => String::new(),
    }
}

/// Render a grouped log for the Reflector: each group becomes a
/// `## Group \`id\`` section with its `_range:` line (mastra
/// `renderObservationGroupsForReflection`).
pub fn render_groups_for_reflection(observations: &str) -> Option<String> {
    let groups = parse_observation_groups(observations);
    if groups.is_empty() {
        return None;
    }
    let mut result = String::new();
    let mut rest = observations;
    let open_tag = "<observation-group ";
    while let Some(start) = rest.find(open_tag) {
        let after_tag = &rest[start + open_tag.len()..];
        let Some(attr_end) = after_tag.find('>') else {
            break;
        };
        let Some(close) = rest[start..].find("</observation-group>") else {
            break;
        };
        let content = rest[start + open_tag.len() + attr_end + 1..start + close].trim();
        let group = groups.iter().find(|g| g.content == content);
        let rendered = match group {
            Some(g) => format!(
                "## Group `{}`\n_range: `{}`_\n\n{}",
                g.id, g.range, g.content
            ),
            None => content.to_owned(),
        };
        result.push_str(&rest[..start]);
        result.push_str(&rendered);
        rest = &rest[start + close + "</observation-group>".len()..];
    }
    result.push_str(rest);
    Some(result.replace("\n\n\n", "\n\n").trim().to_owned())
}

/// Re-derive group provenance from a Reflector's rewritten sections: each
/// `## Group` section keeps the canonical id from its heading, merges the
/// ranges of the source groups whose lines it shares (index fallback), and
/// is marked `kind="reflection"` (mastra `deriveObservationGroupProvenance`).
pub fn derive_group_provenance(
    content: &str,
    groups: &[ObservationGroup],
) -> Vec<ObservationGroup> {
    let sections = reflection_sections(content);
    if sections.is_empty() || groups.is_empty() {
        return Vec::new();
    }
    sections
        .into_iter()
        .enumerate()
        .map(|(index, (heading, body))| {
            let body_lines: std::collections::BTreeSet<&str> = body
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .collect();
            let matching: Vec<ObservationGroup> = groups
                .iter()
                .filter(|g| {
                    g.content
                        .lines()
                        .any(|l| !l.trim().is_empty() && body_lines.contains(l.trim()))
                })
                .cloned()
                .collect();
            let resolved = if !matching.is_empty() {
                matching
            } else {
                vec![groups[index.min(groups.len() - 1)].clone()]
            };
            let id = match heading.split('`').nth(1).map(str::trim) {
                Some(s) if !s.is_empty() => s.to_owned(),
                _ => format!("derived-group-{}", index + 1),
            };
            ObservationGroup {
                id,
                range: combine_group_ranges(&resolved),
                kind: Some("reflection".to_owned()),
                content: body,
            }
        })
        .collect()
}

/// The reflection section shape the Reflector is steered toward: `## Group`
/// headings with an optional `_range: \`...\`_` metadata line (mastra
/// `parseReflectionObservationGroupSections`).
fn reflection_sections(content: &str) -> Vec<(String, String)> {
    let normalized = content.trim();
    if !normalized.lines().any(|l| l.starts_with("## Group ")) {
        return Vec::new();
    }
    let mut sections = Vec::new();
    let mut current: Option<(String, String)> = None;
    for line in normalized.lines() {
        if line.starts_with("## Group ") {
            if let Some((h, b)) = current.take() {
                sections.push((h, b.trim().to_owned()));
            }
            current = Some((line.trim().to_owned(), String::new()));
        } else if let Some((_, body)) = current.as_mut() {
            if line.starts_with("_range:") {
                continue; // metadata line, stripped by the upstream parser
            }
            body.push_str(line);
            body.push('\n');
        }
    }
    if let Some((h, b)) = current {
        sections.push((h, b.trim().to_owned()));
    }
    sections
}

/// Commit a Reflector's output as the new log: re-wrap its sections as
/// `kind="reflection"` groups over the merged source ranges (mastra
/// `reconcileObservationGroupsFromReflection`).
pub fn reconcile_groups_from_reflection(
    content: &str,
    source_observations: &str,
) -> Option<String> {
    let source_groups = parse_observation_groups(source_observations);
    if source_groups.is_empty() {
        return None;
    }
    let normalized = content.trim();
    if normalized.is_empty() {
        return Some(String::new());
    }
    let derived = derive_group_provenance(normalized, &source_groups);
    if !derived.is_empty() {
        return Some(
            derived
                .iter()
                .map(|g| wrap_in_observation_group(&g.content, &g.range, &g.id, g.kind.as_deref()))
                .collect::<Vec<_>>()
                .join("\n\n"),
        );
    }
    Some(wrap_in_observation_group(
        normalized,
        &combine_group_ranges(&source_groups),
        &generate_group_id(normalized),
        Some("reflection"),
    ))
}
