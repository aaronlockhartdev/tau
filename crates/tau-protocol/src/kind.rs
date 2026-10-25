//! The session entry's kind tag (ADR-0034). The wire spellings are the
//! on-disk values of ADR-0005 and must never change: they are what every
//! existing `.tau/sessions/*.jsonl` line already says.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// What kind of record a session line is. Each kind's payload shape is owned
/// by one type in [`crate::payload`] (C9); this enum is the discriminator.
///
/// `Other` is the forward-compat arm (ADR-0008): a line written by a newer
/// core still loads in an older one, keeps its name, and renders as its name
/// instead of failing the load. The cost is that a misspelled kind is not a
/// load error; the wire goldens in `tests/wire_goldens.rs` pin the known
/// spellings so a writer typo surfaces as a test failure, not a silent arm.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum EntryKind {
    User,
    Assistant,
    Tool,
    /// The legacy notice channel. Read-only from here on: new facts are
    /// [`EntryKind::Diagnostic`] (ADR-0034).
    System,
    Om,
    Subagent,
    SpawnSnapshot,
    Diagnostic,
    /// A kind this build does not know, preserved verbatim for round-trip.
    Other(String),
}

impl EntryKind {
    /// The wire spelling.
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::User => "user",
            Self::Assistant => "assistant",
            Self::Tool => "tool",
            Self::System => "system",
            Self::Om => "om",
            Self::Subagent => "subagent",
            Self::SpawnSnapshot => "spawn-snapshot",
            Self::Diagnostic => "diagnostic",
            Self::Other(kind) => kind,
        }
    }

    /// The kind a wire spelling denotes; unknown spellings keep their name.
    #[must_use]
    pub fn parse(value: &str) -> Self {
        match value {
            "user" => Self::User,
            "assistant" => Self::Assistant,
            "tool" => Self::Tool,
            "system" => Self::System,
            "om" => Self::Om,
            "subagent" => Self::Subagent,
            "spawn-snapshot" => Self::SpawnSnapshot,
            "diagnostic" => Self::Diagnostic,
            other => Self::Other(other.to_owned()),
        }
    }

    /// Raw entries carry the conversation and may enter the model's context;
    /// record entries (`om`, `spawn-snapshot`, `system`, `diagnostic`,
    /// `subagent`) are the harness's own bookkeeping and never do (spec §3).
    /// Exhaustive so a new kind cannot drift into the prompt by omission.
    #[must_use]
    pub fn is_raw(&self) -> bool {
        matches!(self, Self::User | Self::Assistant | Self::Tool)
    }
}

impl Serialize for EntryKind {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for EntryKind {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Self::parse(&String::deserialize(deserializer)?))
    }
}

impl std::fmt::Display for EntryKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::EntryKind;
    use serde_json::json;

    #[test]
    fn known_kinds_round_trip_through_their_wire_spellings() {
        let cases = [
            (EntryKind::User, "user"),
            (EntryKind::Assistant, "assistant"),
            (EntryKind::Tool, "tool"),
            (EntryKind::System, "system"),
            (EntryKind::Om, "om"),
            (EntryKind::Subagent, "subagent"),
            (EntryKind::SpawnSnapshot, "spawn-snapshot"),
            (EntryKind::Diagnostic, "diagnostic"),
        ];
        for (kind, spelling) in cases {
            assert_eq!(kind.as_str(), spelling);
            let encoded = serde_json::to_string(&kind).expect("a kind serializes");
            assert_eq!(encoded, format!("\"{spelling}\""));
            let parsed: EntryKind = serde_json::from_str(&encoded).expect("a known spelling");
            assert_eq!(parsed, kind);
        }
    }

    #[test]
    fn an_unknown_kind_keeps_its_name() {
        let kind: EntryKind = serde_json::from_str("\"quantum\"").expect("a string kind");
        assert_eq!(kind, EntryKind::Other("quantum".into()));
        assert_eq!(json!(kind), json!("quantum"));
    }

    #[test]
    fn only_raw_kinds_reach_the_prompt() {
        for kind in [EntryKind::User, EntryKind::Assistant, EntryKind::Tool] {
            assert!(kind.is_raw(), "{kind} is raw");
        }
        for kind in [
            EntryKind::System,
            EntryKind::Om,
            EntryKind::Subagent,
            EntryKind::SpawnSnapshot,
            EntryKind::Diagnostic,
            EntryKind::Other("quantum".into()),
        ] {
            assert!(!kind.is_raw(), "{kind} is a record");
        }
    }
}
