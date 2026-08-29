//! Advisory repeated-call reminder for the agent loop. The run already has a
//! hard `max_identical_actions` guard (see `RunBudget` and the `recent_actions`
//! ring in run control) that stops a run once identical tool calls exceed the
//! tier limit. This module adds an advisory notice before that boundary: when
//! the same tool with the same canonical arguments repeats consecutively and
//! reaches one of the configured thresholds, the next model turn carries a
//! reminder. The reminder never vetoes or rewrites a call.

use crate::tool_input_fingerprint;
use agent_core::{Message, MessageRole, Metadata};

/// Consecutive identical tool-call counts at which an advisory reminder is
/// injected into the model context. Both sit at or below the tier hard
/// `max_identical_actions` limits (3/3/4), so the model is warned before the
/// run is stopped.
pub const REPETITION_ADVISORY_THRESHOLDS: &[usize] = &[3, 5];
/// Character cap for the canonical-argument preview inside the advisory text.
/// Detection always uses the full canonical string; only the preview is capped.
pub const REPETITION_ADVISORY_PREVIEW_MAX_CHARS: usize = 500;
/// Metadata kind of the injected advisory message.
pub const REPETITION_ADVISORY_KIND: &str = "repetition_advisory";

const REPETITION_ADVISORY_SCHEMA: &str = "cindx.repetition-advisory.v1";
const PREVIEW_TRUNCATION_MARKER: &str = "...[truncated]";

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RepetitionAdvisoryTracker {
    fingerprint: Option<String>,
    tool_name: String,
    canonical_input: String,
    count: usize,
}

impl RepetitionAdvisoryTracker {
    /// Observe one tool action on the loop's observation path. A different
    /// tool or canonical argument resets the consecutive streak.
    pub fn observe(&mut self, tool_name: &str, input_json: &str) {
        let canonical_input = canonical_tool_input(input_json);
        let fingerprint = tool_input_fingerprint(tool_name, &canonical_input);
        if self.fingerprint.as_deref() == Some(fingerprint.as_str()) {
            self.count = self.count.saturating_add(1);
        } else {
            self.fingerprint = Some(fingerprint);
            self.tool_name = tool_name.to_string();
            self.canonical_input = canonical_input;
            self.count = 1;
        }
    }

    /// The current consecutive identical-call streak length.
    pub fn streak(&self) -> usize {
        self.count
    }

    /// The tool name of the current identical-call streak.
    pub fn tool_name(&self) -> &str {
        &self.tool_name
    }

    /// The canonical argument string of the current identical-call streak.
    pub fn canonical_input(&self) -> &str {
        &self.canonical_input
    }

    /// True exactly when the current streak reaches an advisory threshold.
    pub fn advisory_due(&self) -> bool {
        REPETITION_ADVISORY_THRESHOLDS.contains(&self.count)
    }

    /// The advisory context message when one is due. Advisory only: it never
    /// vetoes, rewrites, or delays the repeated call.
    pub fn advisory_message(&self) -> Option<Message> {
        if !self.advisory_due() {
            return None;
        }
        Some(advisory_message_from(
            repetition_advisory_content(&self.tool_name, self.count, &self.canonical_input),
            self.count,
            &self.tool_name,
        ))
    }
}

/// The advisory notice text for a repeated-call streak. Shared by the tracker
/// and the repetition observer so both emit byte-identical advisories.
pub(crate) fn repetition_advisory_content(
    tool_name: &str,
    count: usize,
    canonical_input: &str,
) -> String {
    let preview = bounded_argument_preview(canonical_input, REPETITION_ADVISORY_PREVIEW_MAX_CHARS);
    format!(
        "Advisory repetition notice: `{}` has now been called {} consecutive times with identical arguments: {}. This notice is advisory only; it does not block or rewrite the call. Before repeating it, change the approach (different arguments, a different check, or move on); the run's hard repeated-action limit still applies.",
        tool_name, count, preview
    )
}

/// Builds the advisory overlay message from a prepared content string.
pub(crate) fn advisory_message_from(content: String, count: usize, tool_name: &str) -> Message {
    Message {
        role: MessageRole::System,
        content,
        metadata: [
            ("internal".to_string(), "true".to_string()),
            ("kind".to_string(), REPETITION_ADVISORY_KIND.to_string()),
            (
                "repetition_advisory_schema".to_string(),
                REPETITION_ADVISORY_SCHEMA.to_string(),
            ),
            ("repetition_streak".to_string(), count.to_string()),
            ("tool_name".to_string(), tool_name.to_string()),
        ]
        .into_iter()
        .collect::<Metadata>(),
    }
}

/// The canonical form of a tool input: parsed-and-reserialized JSON when the
/// input is JSON, otherwise the trimmed raw string. Mirrors the canonical
/// input used by `tool_input_fingerprint`.
pub(crate) fn canonical_tool_input(input_json: &str) -> String {
    serde_json::from_str::<serde_json::Value>(input_json)
        .map(|value| value.to_string())
        .unwrap_or_else(|_| input_json.trim().to_string())
}

/// Preview of the canonical argument string capped to `max_chars` characters
/// (including the truncation marker when one is needed).
pub(crate) fn bounded_argument_preview(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    let keep = max_chars.saturating_sub(PREVIEW_TRUNCATION_MARKER.chars().count());
    let mut preview: String = value.chars().take(keep).collect();
    preview.push_str(PREVIEW_TRUNCATION_MARKER);
    preview
}

#[cfg(test)]
mod tests {
    use super::*;

    fn observe_streak(tracker: &mut RepetitionAdvisoryTracker, count: usize) {
        for _ in 0..count {
            tracker.observe("file.read", r#"{"path":"README.md"}"#);
        }
    }

    #[test]
    fn threshold_triggers_the_advisory() {
        let mut tracker = RepetitionAdvisoryTracker::default();
        observe_streak(&mut tracker, 2);
        assert!(!tracker.advisory_due());
        assert!(tracker.advisory_message().is_none());

        tracker.observe("file.read", r#"{"path":"README.md"}"#);
        assert_eq!(tracker.streak(), 3);
        assert!(tracker.advisory_due());
        let advisory = tracker
            .advisory_message()
            .expect("an advisory is due at the first threshold");
        assert_eq!(
            advisory.metadata.get("kind").map(String::as_str),
            Some(REPETITION_ADVISORY_KIND)
        );
        assert_eq!(
            advisory
                .metadata
                .get("repetition_streak")
                .map(String::as_str),
            Some("3")
        );
        assert!(advisory.content.contains("file.read"));
        assert!(advisory.content.contains("advisory only"));
    }

    #[test]
    fn argument_preview_is_capped_but_detection_uses_the_full_input() {
        let mut tracker = RepetitionAdvisoryTracker::default();
        let long_path = "x".repeat(2_000);
        for _ in 0..3 {
            tracker.observe("file.read", &format!(r#"{{"path":"{long_path}"}}"#));
        }
        assert_eq!(
            tracker.streak(),
            3,
            "the full canonical string drives detection"
        );

        let advisory = tracker.advisory_message().expect("advisory due");
        let preview = bounded_argument_preview(
            &tracker.canonical_input,
            REPETITION_ADVISORY_PREVIEW_MAX_CHARS,
        );
        assert!(preview.chars().count() <= REPETITION_ADVISORY_PREVIEW_MAX_CHARS);
        assert!(preview.ends_with("...[truncated]"));
        // The advisory text carries only the capped preview, never the full input.
        assert!(!advisory.content.contains(&"x".repeat(600)));
    }

    #[test]
    fn advisory_never_vetoes_the_repeated_call() {
        let mut tracker = RepetitionAdvisoryTracker::default();
        observe_streak(&mut tracker, 3);
        assert!(tracker.advisory_due());

        // The tracker has no veto surface: observing further identical
        // calls keeps counting instead of blocking.
        tracker.observe("file.read", r#"{"path":"README.md"}"#);
        assert_eq!(tracker.streak(), 4);
        assert!(
            !tracker.advisory_due(),
            "the notice fires once per threshold"
        );
        tracker.observe("file.read", r#"{"path":"README.md"}"#);
        assert_eq!(tracker.streak(), 5);
        assert!(tracker.advisory_due(), "the second threshold fires at five");
    }

    #[test]
    fn a_different_call_resets_the_streak() {
        let mut tracker = RepetitionAdvisoryTracker::default();
        observe_streak(&mut tracker, 2);

        tracker.observe("file.read", r#"{"path":"OTHER.md"}"#);
        assert_eq!(tracker.streak(), 1);
        assert!(!tracker.advisory_due());

        tracker.observe("file.list", r#"{"path":"README.md"}"#);
        assert_eq!(tracker.streak(), 1);

        // Canonical (not textual) equality drives the streak: reordered JSON
        // keys are the same canonical input.
        tracker.observe("file.list", r#"{"depth":1,"path":"README.md"}"#);
        tracker.observe("file.list", r#"{"path":"README.md","depth":1}"#);
        assert_eq!(tracker.streak(), 2, "key order does not break the streak");
        assert!(!tracker.advisory_due());
        tracker.observe("file.list", r#"{"depth":1,"path":"README.md"}"#);
        assert_eq!(tracker.streak(), 3);
        assert!(tracker.advisory_due());
    }
}
