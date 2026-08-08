use crate::memory_text::{is_sha256_hex, sha256_hex};
use crate::{memory_content_sha256, MemoryRecord};
use serde::{de::Error as _, Deserialize, Deserializer, Serialize};

pub const MEMORY_INFLUENCE_RECEIPT_SCHEMA: &str = "cindx.memory-influence-receipt.v1";
pub const MEMORY_EFFECT_RECEIPT_SCHEMA: &str = "cindx.memory-effect-receipt.v1";
pub const MEMORY_UTILITY_ATTRIBUTION_SCHEMA: &str = "cindx.memory-utility-attribution.v1";
pub const MAX_MEMORY_UTILITY_ATTRIBUTIONS: usize = 16;
pub const MAX_MEMORY_UTILITY_VALIDITY_MS: u64 = 90 * 86_400_000;

const MAX_RECEIPT_ID_CHARS: usize = 192;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryExperienceKey {
    pub memory_id: String,
    pub project_id: String,
    pub session_id: String,
    pub logical_run_id: String,
    pub steer_epoch: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryInfluenceKind {
    ToolObservedInRecalledRun,
    ConstraintObservedInRecalledRun,
    VerificationObservedInRecalledRun,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryEffectKind {
    MatchedImprovement,
    MatchedRegression,
    VerifiedOutcome,
    UserCorrection,
    Counterexample,
    Inconclusive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryEvidenceKind {
    MatchedEvaluationReceipt,
    TestReceipt,
    BrowserReceipt,
    RetrievalReceipt,
    ArtifactReceipt,
    WorkspaceRevision,
    UserCorrection,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryUtilityDisposition {
    Helpful,
    Harmful,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryInfluenceReceipt {
    pub schema: String,
    pub key: MemoryExperienceKey,
    pub memory_sha256: String,
    pub task_condition_sha256: String,
    pub environment_sha256: String,
    pub action_sha256: String,
    pub influence: MemoryInfluenceKind,
    pub recorded_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryEffectReceipt {
    pub schema: String,
    pub key: MemoryExperienceKey,
    pub influence_receipt_sha256: String,
    pub evidence_sha256: String,
    pub outcome_sha256: String,
    pub effect: MemoryEffectKind,
    pub evidence: MemoryEvidenceKind,
    pub recorded_at_ms: u64,
    pub valid_until_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryUtilityAttribution {
    pub schema: String,
    pub key: MemoryExperienceKey,
    pub memory_sha256: String,
    pub influence: MemoryInfluenceKind,
    pub effect: MemoryEffectKind,
    pub evidence: MemoryEvidenceKind,
    pub disposition: MemoryUtilityDisposition,
    pub influence_receipt_sha256: String,
    pub effect_receipt_sha256: String,
    pub recorded_at_ms: u64,
    pub valid_until_ms: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct MemoryUtilitySummary {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    attributions: Vec<MemoryUtilityAttribution>,
}

impl MemoryUtilitySummary {
    pub fn is_empty(&self) -> bool {
        self.attributions.is_empty()
    }

    pub fn disposition_for_at(
        &self,
        record: &MemoryRecord,
        now_ms: u64,
    ) -> MemoryUtilityDisposition {
        if !self.is_structurally_valid_for(record) {
            return MemoryUtilityDisposition::Unknown;
        }
        disposition_from(
            self.attributions
                .iter()
                .filter(|item| attribution_is_active_at(item, now_ms)),
        )
    }

    pub fn len(&self) -> usize {
        self.attributions.len()
    }

    pub fn attributions(&self) -> &[MemoryUtilityAttribution] {
        &self.attributions
    }

    pub fn is_structurally_valid_for(&self, record: &MemoryRecord) -> bool {
        self.attributions.len() <= MAX_MEMORY_UTILITY_ATTRIBUTIONS
            && !has_duplicate_experience_keys(&self.attributions)
            && self.attributions.iter().all(|item| {
                valid_attribution_shape(item)
                    && item.key.memory_id == record.id
                    && item.key.project_id == record.provenance.project_id
                    && item.memory_sha256 == memory_content_sha256(&record.content)
                    && validity_matches_authority(item.valid_until_ms, record)
            })
    }

    pub fn has_active_attribution_for_at(&self, record: &MemoryRecord, now_ms: u64) -> bool {
        self.is_structurally_valid_for(record)
            && self
                .attributions
                .iter()
                .any(|item| attribution_is_active_at(item, now_ms))
    }

    pub(crate) fn merge_bounded(&mut self, incoming: &Self) {
        let mut merged = Vec::with_capacity(self.attributions.len() + incoming.attributions.len());
        merged.extend(self.attributions.iter().cloned());
        merged.extend(incoming.attributions.iter().cloned());
        canonicalize_attributions(&mut merged);
        self.attributions = merged;
    }

    pub(crate) fn retain_valid_for(
        &mut self,
        memory_id: &str,
        project_id: &str,
        memory_sha256: &str,
        durable_user_requirement: bool,
    ) {
        self.attributions.retain(|item| {
            valid_attribution_shape(item)
                && item.key.memory_id == memory_id
                && item.key.project_id == project_id
                && item.memory_sha256 == memory_sha256
                && (item.valid_until_ms.is_none() == durable_user_requirement)
        });
        canonicalize_attributions(&mut self.attributions);
    }
}

impl<'de> Deserialize<'de> for MemoryUtilitySummary {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct WireSummary {
            #[serde(default)]
            attributions: Vec<MemoryUtilityAttribution>,
        }

        let wire = WireSummary::deserialize(deserializer)?;
        if wire.attributions.len() > MAX_MEMORY_UTILITY_ATTRIBUTIONS
            || has_duplicate_experience_keys(&wire.attributions)
            || wire
                .attributions
                .iter()
                .any(|item| !valid_attribution_shape(item))
        {
            return Err(D::Error::custom(
                "memory utility summary is malformed or exceeds its bound",
            ));
        }
        Ok(Self {
            attributions: wire.attributions,
        })
    }
}

pub fn memory_influence_receipt_sha256(
    receipt: &MemoryInfluenceReceipt,
    now_ms: u64,
) -> Option<String> {
    valid_influence_receipt(receipt, now_ms)
        .then(|| receipt_sha256(receipt))
        .flatten()
}

pub fn record_memory_utility(
    record: &mut MemoryRecord,
    influence: &MemoryInfluenceReceipt,
    effect: &MemoryEffectReceipt,
    now_ms: u64,
) -> Result<bool, &'static str> {
    if !record.utility.is_structurally_valid_for(record) {
        return Err("memory utility summary is structurally invalid");
    }
    if !valid_influence_receipt(influence, now_ms) {
        return Err("invalid memory influence receipt");
    }
    if !valid_effect_receipt(effect, now_ms) {
        return Err("invalid memory effect receipt");
    }
    if influence.key != effect.key
        || influence.key.memory_id != record.id
        || influence.key.project_id != record.provenance.project_id
        || influence.memory_sha256 != memory_content_sha256(&record.content)
        || effect.recorded_at_ms < influence.recorded_at_ms
    {
        return Err("memory attribution keys or chronology do not match");
    }
    let user_stated_requirement = is_durable_user_requirement(record);
    if (user_stated_requirement && effect.valid_until_ms.is_some())
        || (!user_stated_requirement && effect.valid_until_ms.is_none())
    {
        return Err("memory utility validity does not match memory authority");
    }
    let influence_receipt_sha256 =
        receipt_sha256(influence).ok_or("memory influence receipt could not be hashed")?;
    if effect.influence_receipt_sha256 != influence_receipt_sha256 {
        return Err("memory effect does not reference the influence receipt");
    }
    let effect_receipt_sha256 =
        receipt_sha256(effect).ok_or("memory effect receipt could not be hashed")?;
    if record
        .utility
        .attributions()
        .iter()
        .any(|item| item.effect_receipt_sha256 == effect_receipt_sha256)
    {
        return Ok(false);
    }

    let attribution = MemoryUtilityAttribution {
        schema: MEMORY_UTILITY_ATTRIBUTION_SCHEMA.to_string(),
        key: influence.key.clone(),
        memory_sha256: influence.memory_sha256.clone(),
        influence: influence.influence,
        effect: effect.effect,
        evidence: effect.evidence,
        disposition: disposition_for_effect(effect.effect),
        influence_receipt_sha256,
        effect_receipt_sha256,
        recorded_at_ms: effect.recorded_at_ms,
        valid_until_ms: effect.valid_until_ms,
    };
    let mut incoming = MemoryUtilitySummary {
        attributions: vec![attribution],
    };
    incoming.merge_bounded(&MemoryUtilitySummary::default());
    if incoming.is_empty() {
        return Err("memory attribution failed validation");
    }
    let before = record.utility.clone();
    record.utility.merge_bounded(&incoming);
    Ok(record.utility != before)
}

fn valid_influence_receipt(receipt: &MemoryInfluenceReceipt, now_ms: u64) -> bool {
    receipt.schema == MEMORY_INFLUENCE_RECEIPT_SCHEMA
        && valid_key(&receipt.key)
        && valid_timestamp(receipt.recorded_at_ms, now_ms)
        && [
            &receipt.memory_sha256,
            &receipt.task_condition_sha256,
            &receipt.environment_sha256,
            &receipt.action_sha256,
        ]
        .into_iter()
        .all(|digest| is_sha256_hex(digest))
}

fn valid_effect_receipt(receipt: &MemoryEffectReceipt, now_ms: u64) -> bool {
    receipt.schema == MEMORY_EFFECT_RECEIPT_SCHEMA
        && valid_key(&receipt.key)
        && valid_timestamp(receipt.recorded_at_ms, now_ms)
        && [
            &receipt.influence_receipt_sha256,
            &receipt.evidence_sha256,
            &receipt.outcome_sha256,
        ]
        .into_iter()
        .all(|digest| is_sha256_hex(digest))
        && evidence_matches_effect(receipt.effect, receipt.evidence)
        && valid_expiry(receipt.recorded_at_ms, receipt.valid_until_ms, now_ms)
}

fn attribution_is_active_at(attribution: &MemoryUtilityAttribution, now_ms: u64) -> bool {
    valid_attribution_shape(attribution)
        && valid_timestamp(attribution.recorded_at_ms, now_ms)
        && valid_expiry(
            attribution.recorded_at_ms,
            attribution.valid_until_ms,
            now_ms,
        )
}

fn valid_attribution_shape(attribution: &MemoryUtilityAttribution) -> bool {
    attribution.schema == MEMORY_UTILITY_ATTRIBUTION_SCHEMA
        && valid_key(&attribution.key)
        && attribution.recorded_at_ms > 0
        && is_sha256_hex(&attribution.memory_sha256)
        && is_sha256_hex(&attribution.influence_receipt_sha256)
        && is_sha256_hex(&attribution.effect_receipt_sha256)
        && evidence_matches_effect(attribution.effect, attribution.evidence)
        && attribution.disposition == disposition_for_effect(attribution.effect)
        && valid_expiry_shape(attribution.recorded_at_ms, attribution.valid_until_ms)
}

fn valid_key(key: &MemoryExperienceKey) -> bool {
    [
        &key.memory_id,
        &key.project_id,
        &key.session_id,
        &key.logical_run_id,
    ]
    .into_iter()
    .all(|value| {
        !value.trim().is_empty()
            && value.chars().count() <= MAX_RECEIPT_ID_CHARS
            && !value.chars().any(char::is_control)
    })
}

fn valid_timestamp(recorded_at_ms: u64, now_ms: u64) -> bool {
    recorded_at_ms > 0 && now_ms > 0 && recorded_at_ms <= now_ms
}

fn valid_expiry(recorded_at_ms: u64, valid_until_ms: Option<u64>, now_ms: u64) -> bool {
    valid_expiry_shape(recorded_at_ms, valid_until_ms)
        && valid_until_ms.is_none_or(|valid_until_ms| now_ms < valid_until_ms)
}

fn valid_expiry_shape(recorded_at_ms: u64, valid_until_ms: Option<u64>) -> bool {
    valid_until_ms.is_none_or(|valid_until_ms| {
        valid_until_ms > recorded_at_ms
            && valid_until_ms.saturating_sub(recorded_at_ms) <= MAX_MEMORY_UTILITY_VALIDITY_MS
    })
}

fn is_durable_user_requirement(record: &MemoryRecord) -> bool {
    record.kind == crate::MemoryKind::Requirement && record.trust == crate::MemoryTrust::UserStated
}

fn validity_matches_authority(valid_until_ms: Option<u64>, record: &MemoryRecord) -> bool {
    valid_until_ms.is_none() == is_durable_user_requirement(record)
}

fn evidence_matches_effect(effect: MemoryEffectKind, evidence: MemoryEvidenceKind) -> bool {
    match effect {
        MemoryEffectKind::MatchedImprovement | MemoryEffectKind::MatchedRegression => {
            evidence == MemoryEvidenceKind::MatchedEvaluationReceipt
        }
        MemoryEffectKind::VerifiedOutcome => !matches!(
            evidence,
            MemoryEvidenceKind::UserCorrection | MemoryEvidenceKind::MatchedEvaluationReceipt
        ),
        MemoryEffectKind::UserCorrection => evidence == MemoryEvidenceKind::UserCorrection,
        MemoryEffectKind::Counterexample | MemoryEffectKind::Inconclusive => {
            evidence != MemoryEvidenceKind::MatchedEvaluationReceipt
        }
    }
}

fn disposition_for_effect(effect: MemoryEffectKind) -> MemoryUtilityDisposition {
    match effect {
        MemoryEffectKind::MatchedImprovement => MemoryUtilityDisposition::Helpful,
        MemoryEffectKind::MatchedRegression => MemoryUtilityDisposition::Harmful,
        MemoryEffectKind::VerifiedOutcome
        | MemoryEffectKind::UserCorrection
        | MemoryEffectKind::Counterexample
        | MemoryEffectKind::Inconclusive => MemoryUtilityDisposition::Unknown,
    }
}

fn canonicalize_attributions(attributions: &mut Vec<MemoryUtilityAttribution>) {
    attributions.retain(valid_attribution_shape);
    attributions.sort_by(|left, right| {
        right
            .recorded_at_ms
            .cmp(&left.recorded_at_ms)
            .then_with(|| right.effect_receipt_sha256.cmp(&left.effect_receipt_sha256))
    });
    let mut seen = Vec::<MemoryExperienceKey>::with_capacity(attributions.len());
    attributions.retain(|item| {
        if seen.contains(&item.key) {
            false
        } else {
            seen.push(item.key.clone());
            true
        }
    });
    attributions.truncate(MAX_MEMORY_UTILITY_ATTRIBUTIONS);
}

fn has_duplicate_experience_keys(attributions: &[MemoryUtilityAttribution]) -> bool {
    attributions.iter().enumerate().any(|(index, item)| {
        attributions[index + 1..]
            .iter()
            .any(|other| other.key == item.key)
    })
}

fn disposition_from<'a>(
    attributions: impl Iterator<Item = &'a MemoryUtilityAttribution>,
) -> MemoryUtilityDisposition {
    let mut helpful = false;
    for item in attributions {
        match item.disposition {
            MemoryUtilityDisposition::Harmful => return MemoryUtilityDisposition::Harmful,
            MemoryUtilityDisposition::Helpful => helpful = true,
            MemoryUtilityDisposition::Unknown => {}
        }
    }
    if helpful {
        MemoryUtilityDisposition::Helpful
    } else {
        MemoryUtilityDisposition::Unknown
    }
}

fn receipt_sha256<T: Serialize>(receipt: &T) -> Option<String> {
    serde_json::to_vec(receipt)
        .ok()
        .map(|encoded| sha256_hex(&encoded))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        fuse_memory_recalls_at, merge_memory_records, recall_memories_at,
        record_memory_observed_uses, MemoryKind, MemoryLedger, MemoryProvenance, MemoryTrust,
    };

    fn digest(value: &str) -> String {
        sha256_hex(value.as_bytes())
    }

    fn memory_record(id: &str) -> MemoryRecord {
        let content = "The release receipt must preserve the source revision".to_string();
        MemoryRecord {
            id: id.to_string(),
            fingerprint: id.to_string(),
            kind: MemoryKind::Evidence,
            trust: MemoryTrust::ToolVerified,
            content,
            importance: 80,
            provenance: MemoryProvenance {
                project_id: "project-a".to_string(),
                session_id: "session-a".to_string(),
                event_id: "event-a".to_string(),
                agent_run_id: Some("attempt-a".to_string()),
                sequence: 1,
                timestamp_ms: 10,
            },
            source_event_ids: vec!["event-a".to_string()],
            source_session_ids: vec!["session-a".to_string()],
            user_requirement_evidence: Vec::new(),
            created_at_ms: 10,
            updated_at_ms: 10,
            recall_count: 0,
            last_recalled_at_ms: None,
            observed_use_count: 0,
            last_observed_use_at_ms: None,
            utility: MemoryUtilitySummary::default(),
            superseded_by: None,
            superseded_at_ms: None,
        }
    }

    fn influence(record: &MemoryRecord, run: &str, recorded_at_ms: u64) -> MemoryInfluenceReceipt {
        MemoryInfluenceReceipt {
            schema: MEMORY_INFLUENCE_RECEIPT_SCHEMA.to_string(),
            key: MemoryExperienceKey {
                memory_id: record.id.clone(),
                project_id: record.provenance.project_id.clone(),
                session_id: "session-recall".to_string(),
                logical_run_id: run.to_string(),
                steer_epoch: 2,
            },
            memory_sha256: memory_content_sha256(&record.content),
            task_condition_sha256: digest("task-condition"),
            environment_sha256: digest("workspace-revision"),
            action_sha256: digest(run),
            influence: MemoryInfluenceKind::VerificationObservedInRecalledRun,
            recorded_at_ms,
        }
    }

    fn effect(
        influence: &MemoryInfluenceReceipt,
        kind: MemoryEffectKind,
        evidence: MemoryEvidenceKind,
        recorded_at_ms: u64,
    ) -> MemoryEffectReceipt {
        MemoryEffectReceipt {
            schema: MEMORY_EFFECT_RECEIPT_SCHEMA.to_string(),
            key: influence.key.clone(),
            influence_receipt_sha256: memory_influence_receipt_sha256(influence, recorded_at_ms)
                .expect("valid influence receipt"),
            evidence_sha256: digest("verification-evidence"),
            outcome_sha256: digest("verified-outcome"),
            effect: kind,
            evidence,
            recorded_at_ms,
            valid_until_ms: Some(recorded_at_ms + 1_000),
        }
    }

    #[test]
    fn lexical_overlap_observation_cannot_mint_helpful_utility() {
        assert_eq!(
            MemoryUtilityDisposition::default(),
            MemoryUtilityDisposition::Unknown
        );
        let record = memory_record("memory-a");
        let mut ledger = MemoryLedger {
            records: vec![record],
            ..MemoryLedger::new("project-a")
        };

        assert_eq!(
            record_memory_observed_uses(
                &mut ledger,
                &["memory-a".to_string()],
                "Preserved the release receipt source revision",
                20,
            ),
            vec!["memory-a".to_string()]
        );
        assert_eq!(ledger.records[0].observed_use_count, 1);
        assert_eq!(
            ledger.records[0]
                .utility
                .disposition_for_at(&ledger.records[0], 20),
            MemoryUtilityDisposition::Unknown
        );
    }

    #[test]
    fn only_newest_matched_effect_for_a_full_key_can_be_helpful() {
        let mut record = memory_record("memory-a");
        let influence = influence(&record, "logical-run-a", 20);
        let verified = effect(
            &influence,
            MemoryEffectKind::VerifiedOutcome,
            MemoryEvidenceKind::TestReceipt,
            30,
        );

        assert!(
            record_memory_utility(&mut record, &influence, &verified, 30)
                .expect("trusted attribution should apply")
        );
        assert!(
            !record_memory_utility(&mut record, &influence, &verified, 30)
                .expect("same receipt replay should be harmless")
        );
        assert_eq!(record.utility.len(), 1);
        assert_eq!(
            record.utility.disposition_for_at(&record, 30),
            MemoryUtilityDisposition::Unknown
        );

        let matched = effect(
            &influence,
            MemoryEffectKind::MatchedImprovement,
            MemoryEvidenceKind::MatchedEvaluationReceipt,
            40,
        );
        assert!(record_memory_utility(&mut record, &influence, &matched, 40)
            .expect("newer matched evaluation should replace the plain effect"));
        assert!(
            !record_memory_utility(&mut record, &influence, &matched, 40)
                .expect("matched receipt replay should be idempotent")
        );
        assert_eq!(record.utility.len(), 1);
        assert_eq!(
            record.utility.attributions()[0].recorded_at_ms,
            matched.recorded_at_ms
        );
        assert_eq!(
            record.utility.disposition_for_at(&record, 40),
            MemoryUtilityDisposition::Helpful
        );
        assert_eq!(
            record.utility.disposition_for_at(&record, 1_040),
            MemoryUtilityDisposition::Unknown
        );
        assert!(record.utility.is_structurally_valid_for(&record));
        assert!(!record.utility.has_active_attribution_for_at(&record, 1_040));

        let mut ledger = MemoryLedger::new("project-a");
        merge_memory_records(&mut ledger, [record.clone()], 32);
        merge_memory_records(&mut ledger, [record], 32);
        assert_eq!(ledger.records[0].utility.len(), 1);
    }

    #[test]
    fn plain_effects_remain_unknown_without_matched_evaluation() {
        for (index, (kind, evidence)) in [
            (
                MemoryEffectKind::VerifiedOutcome,
                MemoryEvidenceKind::TestReceipt,
            ),
            (
                MemoryEffectKind::UserCorrection,
                MemoryEvidenceKind::UserCorrection,
            ),
            (
                MemoryEffectKind::Counterexample,
                MemoryEvidenceKind::BrowserReceipt,
            ),
            (
                MemoryEffectKind::Inconclusive,
                MemoryEvidenceKind::ArtifactReceipt,
            ),
        ]
        .into_iter()
        .enumerate()
        {
            let mut record = memory_record(&format!("memory-{index}"));
            let influence = influence(&record, &format!("logical-run-{index}"), 20);
            let effect = effect(&influence, kind, evidence, 30);
            assert!(record_memory_utility(&mut record, &influence, &effect, 30)
                .expect("plain evidence should be recorded as unknown"));
            assert_eq!(
                record.utility.disposition_for_at(&record, 30),
                MemoryUtilityDisposition::Unknown
            );
        }

        let mut record = memory_record("memory-matched-mismatch");
        let mismatch_influence = influence(&record, "logical-run-matched-mismatch", 20);
        let invalid_matched = effect(
            &mismatch_influence,
            MemoryEffectKind::MatchedImprovement,
            MemoryEvidenceKind::TestReceipt,
            30,
        );
        assert!(
            record_memory_utility(&mut record, &mismatch_influence, &invalid_matched, 30,).is_err()
        );
        let invalid_plain = effect(
            &mismatch_influence,
            MemoryEffectKind::VerifiedOutcome,
            MemoryEvidenceKind::MatchedEvaluationReceipt,
            30,
        );
        assert!(
            record_memory_utility(&mut record, &mismatch_influence, &invalid_plain, 30).is_err()
        );

        let mut regression = memory_record("memory-matched-regression");
        let influence = influence(&regression, "logical-run-matched-regression", 20);
        let matched_regression = effect(
            &influence,
            MemoryEffectKind::MatchedRegression,
            MemoryEvidenceKind::MatchedEvaluationReceipt,
            30,
        );
        assert!(
            record_memory_utility(&mut regression, &influence, &matched_regression, 30,)
                .expect("matched regression should be recorded")
        );
        assert_eq!(
            regression.utility.disposition_for_at(&regression, 30),
            MemoryUtilityDisposition::Harmful
        );
    }

    #[test]
    fn matched_regression_is_quarantined_from_recall_until_its_evidence_expires() {
        let mut harmful = memory_record("memory-harmful");
        let influence = influence(&harmful, "logical-run-harmful", 20);
        let regression = effect(
            &influence,
            MemoryEffectKind::MatchedRegression,
            MemoryEvidenceKind::MatchedEvaluationReceipt,
            30,
        );
        record_memory_utility(&mut harmful, &influence, &regression, 30)
            .expect("matched regression should be recorded");
        let harmful_id = harmful.id.clone();
        let mut ledger = MemoryLedger::new("project-a");
        merge_memory_records(&mut ledger, [harmful], 8);

        assert!(recall_memories_at(
            &ledger,
            "release receipt preserve source revision",
            Some("session-b"),
            4,
            40,
        )
        .is_empty());
        assert!(fuse_memory_recalls_at(
            &ledger,
            Vec::new(),
            &std::collections::BTreeMap::from([(harmful_id.clone(), 0.95)]),
            Some("session-b"),
            4,
            40,
        )
        .is_empty());

        let after_expiry = recall_memories_at(
            &ledger,
            "release receipt preserve source revision",
            Some("session-b"),
            4,
            1_040,
        );
        assert_eq!(
            after_expiry
                .into_iter()
                .map(|recall| recall.record.id)
                .collect::<Vec<_>>(),
            vec![harmful_id]
        );
    }

    #[test]
    fn harmful_memory_is_evicted_before_unknown_memory_under_pressure() {
        let mut harmful = memory_record("memory-harmful");
        let influence = influence(&harmful, "logical-run-harmful", 20);
        let regression = effect(
            &influence,
            MemoryEffectKind::MatchedRegression,
            MemoryEvidenceKind::MatchedEvaluationReceipt,
            30,
        );
        record_memory_utility(&mut harmful, &influence, &regression, 30)
            .expect("matched regression should be recorded");
        let unknown = memory_record("memory-unknown");

        let mut ledger = MemoryLedger::new("project-a");
        let stats = merge_memory_records(&mut ledger, [harmful, unknown], 1);

        assert_eq!(stats.evicted, 1);
        assert_eq!(ledger.records.len(), 1);
        assert_eq!(ledger.records[0].id, "memory-unknown");
    }

    #[test]
    fn malformed_future_and_oversize_receipts_fail_closed() {
        let mut record = memory_record("memory-a");
        let mut future = influence(&record, "logical-run-a", 31);
        assert_eq!(memory_influence_receipt_sha256(&future, 30), None);
        let future_effect = MemoryEffectReceipt {
            schema: MEMORY_EFFECT_RECEIPT_SCHEMA.to_string(),
            key: future.key.clone(),
            influence_receipt_sha256: digest("influence"),
            evidence_sha256: digest("evidence"),
            outcome_sha256: digest("outcome"),
            effect: MemoryEffectKind::VerifiedOutcome,
            evidence: MemoryEvidenceKind::TestReceipt,
            recorded_at_ms: 32,
            valid_until_ms: Some(1_032),
        };
        assert!(record_memory_utility(&mut record, &future, &future_effect, 30).is_err());

        let current = influence(&record, "logical-run-expired", 20);
        let mut expired = effect(
            &current,
            MemoryEffectKind::VerifiedOutcome,
            MemoryEvidenceKind::BrowserReceipt,
            21,
        );
        expired.valid_until_ms = Some(30);
        assert!(record_memory_utility(&mut record, &current, &expired, 30).is_err());

        let mut unbounded = effect(
            &current,
            MemoryEffectKind::Inconclusive,
            MemoryEvidenceKind::WorkspaceRevision,
            21,
        );
        unbounded.valid_until_ms = None;
        assert!(record_memory_utility(&mut record, &current, &unbounded, 21).is_err());

        future.recorded_at_ms = 20;
        future.key.logical_run_id = "x".repeat(MAX_RECEIPT_ID_CHARS + 1);
        assert_eq!(memory_influence_receipt_sha256(&future, 30), None);

        let unknown_field = serde_json::json!({
            "schema": MEMORY_INFLUENCE_RECEIPT_SCHEMA,
            "key": {
                "memory_id": "memory-a",
                "project_id": "project-a",
                "session_id": "session-recall",
                "logical_run_id": "logical-run-a",
                "steer_epoch": 0
            },
            "memory_sha256": digest("memory"),
            "task_condition_sha256": digest("task"),
            "environment_sha256": digest("environment"),
            "action_sha256": digest("action"),
            "influence": "verification_observed_in_recalled_run",
            "recorded_at_ms": 20,
            "raw_prompt": "must never be accepted"
        });
        assert!(serde_json::from_value::<MemoryInfluenceReceipt>(unknown_field).is_err());
        let removed_plan_variant = serde_json::json!({
            "schema": MEMORY_INFLUENCE_RECEIPT_SCHEMA,
            "key": {
                "memory_id": "memory-a",
                "project_id": "project-a",
                "session_id": "session-recall",
                "logical_run_id": "logical-run-a",
                "steer_epoch": 0
            },
            "memory_sha256": digest("memory"),
            "task_condition_sha256": digest("task"),
            "environment_sha256": digest("environment"),
            "action_sha256": digest("action"),
            "influence": "plan_changed",
            "recorded_at_ms": 20
        });
        assert!(serde_json::from_value::<MemoryInfluenceReceipt>(removed_plan_variant).is_err());

        let attribution = MemoryUtilityAttribution {
            schema: MEMORY_UTILITY_ATTRIBUTION_SCHEMA.to_string(),
            key: MemoryExperienceKey {
                memory_id: "memory-a".to_string(),
                project_id: "project-a".to_string(),
                session_id: "session-recall".to_string(),
                logical_run_id: "logical-run-a".to_string(),
                steer_epoch: 0,
            },
            memory_sha256: memory_content_sha256(&record.content),
            influence: MemoryInfluenceKind::VerificationObservedInRecalledRun,
            effect: MemoryEffectKind::MatchedImprovement,
            evidence: MemoryEvidenceKind::MatchedEvaluationReceipt,
            disposition: MemoryUtilityDisposition::Helpful,
            influence_receipt_sha256: digest("influence"),
            effect_receipt_sha256: digest("effect"),
            recorded_at_ms: 20,
            valid_until_ms: Some(1_020),
        };
        let mut forged_unbounded = attribution.clone();
        forged_unbounded.valid_until_ms = None;
        let forged_summary = MemoryUtilitySummary {
            attributions: vec![forged_unbounded],
        };
        assert!(!forged_summary.is_structurally_valid_for(&record));
        assert_eq!(
            forged_summary.disposition_for_at(&record, 30),
            MemoryUtilityDisposition::Unknown
        );

        let mut forged_future = attribution.clone();
        forged_future.recorded_at_ms = 31;
        forged_future.valid_until_ms = Some(1_031);
        let future_summary = MemoryUtilitySummary {
            attributions: vec![forged_future],
        };
        assert!(future_summary.is_structurally_valid_for(&record));
        assert!(!future_summary.has_active_attribution_for_at(&record, 30));
        assert_eq!(
            future_summary.disposition_for_at(&record, 30),
            MemoryUtilityDisposition::Unknown
        );

        let oversized = serde_json::json!({
            "attributions": vec![attribution; MAX_MEMORY_UTILITY_ATTRIBUTIONS + 1]
        });
        assert!(serde_json::from_value::<MemoryUtilitySummary>(oversized).is_err());
    }

    #[test]
    fn user_stated_requirement_utility_never_gets_an_automatic_ttl() {
        let mut record = memory_record("memory-requirement");
        record.kind = MemoryKind::Requirement;
        record.trust = MemoryTrust::UserStated;
        let influence = influence(&record, "logical-run-requirement", 20);
        let mut effect = effect(
            &influence,
            MemoryEffectKind::MatchedImprovement,
            MemoryEvidenceKind::MatchedEvaluationReceipt,
            30,
        );

        assert!(record_memory_utility(&mut record, &influence, &effect, 30).is_err());
        effect.valid_until_ms = None;
        effect.influence_receipt_sha256 =
            memory_influence_receipt_sha256(&influence, 30).expect("valid influence receipt");
        assert!(record_memory_utility(&mut record, &influence, &effect, 30)
            .expect("durable user authority may have non-expiring utility"));
        assert_eq!(
            record.utility.disposition_for_at(&record, u64::MAX),
            MemoryUtilityDisposition::Helpful
        );
    }

    #[test]
    fn unknown_utility_never_boosts_recall() {
        let baseline = memory_record("baseline");
        let mut unknown = memory_record("unknown");
        let influence = influence(&unknown, "logical-run-a", 20);
        let effect = effect(
            &influence,
            MemoryEffectKind::Inconclusive,
            MemoryEvidenceKind::ArtifactReceipt,
            30,
        );
        record_memory_utility(&mut unknown, &influence, &effect, 30)
            .expect("inconclusive evidence should remain explicitly unknown");
        let ledger = MemoryLedger {
            records: vec![baseline, unknown],
            ..MemoryLedger::new("project-a")
        };

        let recalls = recall_memories_at(
            &ledger,
            "release receipt preserve source revision",
            Some("session-b"),
            4,
            30,
        );
        let baseline_score = recalls
            .iter()
            .find(|recall| recall.record.id == "baseline")
            .expect("baseline recall")
            .score;
        let unknown_score = recalls
            .iter()
            .find(|recall| recall.record.id == "unknown")
            .expect("unknown recall")
            .score;
        assert!((baseline_score - unknown_score).abs() < f64::EPSILON);
        assert_eq!(
            ledger.records[1]
                .utility
                .disposition_for_at(&ledger.records[1], 30),
            MemoryUtilityDisposition::Unknown
        );
    }

    #[test]
    fn utility_history_is_bounded_and_keeps_newest_unique_receipts() {
        let mut record = memory_record("memory-a");
        for index in 0..(MAX_MEMORY_UTILITY_ATTRIBUTIONS + 4) {
            let recorded_at_ms = 20 + index as u64;
            let influence = influence(&record, &format!("logical-run-{index}"), recorded_at_ms);
            let effect = effect(
                &influence,
                MemoryEffectKind::Inconclusive,
                MemoryEvidenceKind::WorkspaceRevision,
                recorded_at_ms + 1,
            );
            assert!(record_memory_utility(&mut record, &influence, &effect, 100)
                .expect("bounded attribution should apply"));
        }

        assert_eq!(record.utility.len(), MAX_MEMORY_UTILITY_ATTRIBUTIONS);
        assert_eq!(
            record
                .utility
                .attributions()
                .first()
                .map(|item| item.key.logical_run_id.as_str()),
            Some("logical-run-19")
        );
        assert!(record.utility.is_structurally_valid_for(&record));
        assert!(record.utility.has_active_attribution_for_at(&record, 100));
        assert!(!record.utility.has_active_attribution_for_at(&record, 10));
    }

    #[test]
    fn session_is_part_of_the_full_attribution_key() {
        let mut record = memory_record("memory-a");
        let first = influence(&record, "logical-run-a", 20);
        let first_effect = effect(
            &first,
            MemoryEffectKind::Inconclusive,
            MemoryEvidenceKind::ArtifactReceipt,
            30,
        );
        assert!(
            record_memory_utility(&mut record, &first, &first_effect, 30)
                .expect("first session attribution")
        );

        let mut second = influence(&record, "logical-run-a", 20);
        second.key.session_id = "session-other".to_string();
        let second_effect = effect(
            &second,
            MemoryEffectKind::Inconclusive,
            MemoryEvidenceKind::ArtifactReceipt,
            31,
        );
        assert!(
            record_memory_utility(&mut record, &second, &second_effect, 31)
                .expect("different session is a distinct attribution key")
        );

        assert_eq!(record.utility.len(), 2);
        assert!(record.utility.is_structurally_valid_for(&record));
    }

    #[test]
    fn exact_content_change_cannot_inherit_utility_from_a_fingerprint_collision() {
        let mut original = memory_record("memory-a");
        original.fingerprint = "shared-normalized-fingerprint".to_string();
        let influence = influence(&original, "logical-run-a", 20);
        let effect = effect(
            &influence,
            MemoryEffectKind::MatchedImprovement,
            MemoryEvidenceKind::MatchedEvaluationReceipt,
            30,
        );
        record_memory_utility(&mut original, &influence, &effect, 30)
            .expect("matched utility should attach to the exact original content");

        let mut replacement = memory_record("memory-b");
        replacement.fingerprint = original.fingerprint.clone();
        replacement.content = "The release receipt must preserve a different revision".to_string();
        replacement.provenance.sequence = 2;
        replacement.provenance.timestamp_ms = 50;
        replacement.updated_at_ms = 50;

        let mut ledger = MemoryLedger::new("project-a");
        merge_memory_records(&mut ledger, [original], 8);
        merge_memory_records(&mut ledger, [replacement.clone()], 8);

        assert_eq!(ledger.records.len(), 1);
        assert_eq!(ledger.records[0].content, replacement.content);
        assert!(ledger.records[0].utility.is_empty());
        assert_eq!(
            ledger.records[0]
                .utility
                .disposition_for_at(&ledger.records[0], 50),
            MemoryUtilityDisposition::Unknown
        );
    }

    #[test]
    fn merge_ranks_both_bounded_inputs_before_global_truncation() {
        let mut older = memory_record("memory-a");
        let mut newer = memory_record("memory-a");
        for index in 0..MAX_MEMORY_UTILITY_ATTRIBUTIONS {
            let recorded_at_ms = 20 + index as u64;
            let older_influence = influence(&older, &format!("older-{index}"), recorded_at_ms);
            let older_effect = effect(
                &older_influence,
                MemoryEffectKind::Inconclusive,
                MemoryEvidenceKind::WorkspaceRevision,
                recorded_at_ms + 1,
            );
            record_memory_utility(&mut older, &older_influence, &older_effect, 100)
                .expect("older summary attribution");

            let recorded_at_ms = 200 + index as u64;
            let newer_influence = influence(&newer, &format!("newer-{index}"), recorded_at_ms);
            let newer_effect = effect(
                &newer_influence,
                MemoryEffectKind::Inconclusive,
                MemoryEvidenceKind::WorkspaceRevision,
                recorded_at_ms + 1,
            );
            record_memory_utility(&mut newer, &newer_influence, &newer_effect, 300)
                .expect("newer summary attribution");
        }

        older.utility.merge_bounded(&newer.utility);

        assert_eq!(older.utility.len(), MAX_MEMORY_UTILITY_ATTRIBUTIONS);
        assert!(older
            .utility
            .attributions()
            .iter()
            .all(|item| item.key.logical_run_id.starts_with("newer-")));
        assert_eq!(
            older
                .utility
                .attributions()
                .first()
                .map(|item| item.key.logical_run_id.as_str()),
            Some("newer-15")
        );
        assert_eq!(
            older
                .utility
                .attributions()
                .last()
                .map(|item| item.key.logical_run_id.as_str()),
            Some("newer-0")
        );
    }
}
