const RESULT_CONTENT_MAX_CHARS: usize = 24_000;
const RESULT_FRONTIER_MAX: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ResultQuality {
    Draft,
    Substantive,
    Grounded,
    Verified,
    Synthesized,
}

impl ResultQuality {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Substantive => "substantive",
            Self::Grounded => "grounded",
            Self::Verified => "verified",
            Self::Synthesized => "synthesized",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BestKnownResult {
    pub content: String,
    pub stage: String,
    pub quality: ResultQuality,
    pub evidence_count: usize,
    pub verified: bool,
    pub deliverable: bool,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct ResultFrontier {
    best_known: Option<BestKnownResult>,
    candidates: Vec<BestKnownResult>,
}

impl ResultFrontier {
    pub(crate) fn restore(
        best_known: Option<BestKnownResult>,
        candidates: Vec<BestKnownResult>,
    ) -> Self {
        Self {
            best_known,
            candidates,
        }
    }

    pub(crate) fn record(
        &mut self,
        stage: &str,
        content: &str,
        quality: ResultQuality,
        evidence_count: usize,
        verified: bool,
        deliverable: bool,
    ) -> bool {
        let content = bounded_result_content(content);
        if content.is_empty() {
            return false;
        }
        let candidate = BestKnownResult {
            content,
            stage: stage.to_string(),
            quality,
            evidence_count,
            verified,
            deliverable,
        };
        self.candidates.retain(|result| {
            result.stage != candidate.stage || result.content != candidate.content
        });
        self.candidates.push(candidate.clone());
        self.candidates
            .sort_by_key(|result| std::cmp::Reverse(guidance_rank(result)));
        self.candidates.truncate(RESULT_FRONTIER_MAX);

        let should_replace = self
            .best_known
            .as_ref()
            .is_none_or(|current| result_rank(&candidate) > result_rank(current));
        if should_replace {
            self.best_known = Some(candidate);
        }
        should_replace
    }

    pub(crate) fn best_known(&self) -> Option<BestKnownResult> {
        self.best_known.clone()
    }

    pub(crate) fn best_guidance(&self) -> Option<BestKnownResult> {
        self.candidates.first().cloned()
    }

    pub(crate) fn candidates(&self) -> Vec<BestKnownResult> {
        self.candidates.clone()
    }
}

fn bounded_result_content(content: &str) -> String {
    let content = content.trim();
    content
        .char_indices()
        .rev()
        .nth(RESULT_CONTENT_MAX_CHARS.saturating_sub(1))
        .map(|(start, _)| content[start..].to_string())
        .unwrap_or_else(|| content.to_string())
}

fn result_rank(result: &BestKnownResult) -> (bool, bool, ResultQuality, usize, usize) {
    (
        result.deliverable,
        result.verified,
        result.quality,
        result.evidence_count,
        result.content.chars().count(),
    )
}

fn guidance_rank(result: &BestKnownResult) -> (bool, ResultQuality, usize, bool, usize) {
    (
        result.verified,
        result.quality,
        result.evidence_count,
        result.deliverable,
        result.content.chars().count(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deliverable_result_wins_delivery_without_hiding_stronger_guidance() {
        let mut frontier = ResultFrontier::default();
        assert!(frontier.record(
            "draft",
            "Deliver this answer.",
            ResultQuality::Draft,
            0,
            false,
            true,
        ));
        assert!(!frontier.record(
            "review",
            "Internal verified guidance.",
            ResultQuality::Verified,
            2,
            true,
            false,
        ));

        assert_eq!(frontier.best_known().unwrap().stage, "draft");
        assert_eq!(frontier.best_guidance().unwrap().stage, "review");
    }

    #[test]
    fn verified_delivery_is_not_replaced_by_unverified_synthesis() {
        let mut frontier = ResultFrontier::default();
        assert!(frontier.record(
            "workflow_synthesis",
            "Verified grounded answer.",
            ResultQuality::Verified,
            3,
            true,
            true,
        ));
        assert!(!frontier.record(
            "terminal_synthesizer",
            "Newer but unverified answer.",
            ResultQuality::Synthesized,
            0,
            false,
            true,
        ));

        assert_eq!(frontier.best_known().unwrap().stage, "workflow_synthesis");
    }

    #[test]
    fn duplicate_candidate_is_replaced_and_unicode_content_remains_bounded() {
        let mut frontier = ResultFrontier::default();
        let oversized = "界".repeat(RESULT_CONTENT_MAX_CHARS + 10);
        assert!(frontier.record(
            "worker",
            &oversized,
            ResultQuality::Grounded,
            1,
            false,
            true,
        ));
        assert!(!frontier.record(
            "worker",
            &oversized,
            ResultQuality::Grounded,
            1,
            false,
            true,
        ));

        assert_eq!(frontier.candidates().len(), 1);
        assert_eq!(
            frontier.best_known().unwrap().content.chars().count(),
            RESULT_CONTENT_MAX_CHARS
        );
    }
}
