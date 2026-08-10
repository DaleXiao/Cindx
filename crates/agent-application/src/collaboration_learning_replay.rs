use crate::collaboration_learning_admission::{
    CollaborationLearningAggregateStatusV1, CollaborationLearningAggregateV1,
    CollaborationLearningCandidateV1, CollaborationLearningCensorReasonV1,
    CollaborationLearningCensorReceiptV1, CollaborationLearningComparisonBindingV1,
    CollaborationLearningConfigV1, CollaborationLearningEvidenceSetV1, CollaborationLearningPairV1,
};
use crate::collaboration_learning_policy::{
    collaboration_learning_sha256, validate_collaboration_learning_sha256,
    CollaborationLearningError,
};
use serde::{Deserialize, Serialize};

type ContractResult<T> = Result<T, CollaborationLearningError>;

pub const COLLABORATION_LEARNING_OFFLINE_GENESIS_SCHEMA: &str =
    "cindx.agent-collaboration-learning-offline-genesis.v1";
pub const COLLABORATION_LEARNING_OFFLINE_ENTRY_SCHEMA: &str =
    "cindx.agent-collaboration-learning-offline-entry.v1";

const GENESIS_HASH_DOMAIN: &[u8] = b"cindx.agent-collaboration-learning-offline-genesis.v1\0";
const ENTRY_HASH_DOMAIN: &[u8] = b"cindx.agent-collaboration-learning-offline-entry.v1\0";
const MAX_GENESIS_JSON_BYTES: usize = 2 * 1024 * 1024;
const MAX_ENTRY_JSON_BYTES: usize = 512 * 1024;
const MAX_REPLAY_ENTRIES: usize = 256;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CollaborationLearningOfflineGenesisV1 {
    schema: String,
    candidate: CollaborationLearningCandidateV1,
    baseline: CollaborationLearningPairV1,
    config: CollaborationLearningConfigV1,
    frozen_holdout: Vec<CollaborationLearningComparisonBindingV1>,
    genesis_sha256: String,
}

impl CollaborationLearningOfflineGenesisV1 {
    pub fn new(
        candidate: CollaborationLearningCandidateV1,
        baseline: CollaborationLearningPairV1,
        config: CollaborationLearningConfigV1,
        frozen_holdout: Vec<CollaborationLearningComparisonBindingV1>,
    ) -> ContractResult<Self> {
        let config = config.reconstruct()?;
        let baseline = baseline.reconstruct()?;
        let candidate = candidate.reconstruct(baseline.workflow_policy()?)?;
        let mut frozen_holdout = frozen_holdout
            .into_iter()
            .map(CollaborationLearningComparisonBindingV1::reconstruct)
            .collect::<ContractResult<Vec<_>>>()?;
        frozen_holdout.sort_by(|left, right| left.digest().cmp(right.digest()));
        CollaborationLearningEvidenceSetV1::new(
            candidate.clone(),
            baseline.clone(),
            config.clone(),
            frozen_holdout.clone(),
        )?;
        let mut genesis = Self {
            schema: COLLABORATION_LEARNING_OFFLINE_GENESIS_SCHEMA.into(),
            candidate,
            baseline,
            config,
            frozen_holdout,
            genesis_sha256: String::new(),
        };
        genesis.genesis_sha256 = genesis.payload_sha256()?;
        Ok(genesis)
    }

    pub fn from_json(encoded: &str) -> ContractResult<Self> {
        if encoded.len() > MAX_GENESIS_JSON_BYTES {
            return Err(error("genesis JSON exceeds its size bound"));
        }
        let decoded: Self = serde_json::from_str(encoded)
            .map_err(|cause| error(format!("genesis JSON is invalid: {cause}")))?;
        decoded.reconstruct()
    }

    pub fn to_json(&self) -> ContractResult<String> {
        let reconstructed = self.clone().reconstruct()?;
        let encoded = serde_json::to_string(&reconstructed)
            .map_err(|cause| error(format!("genesis JSON encoding failed: {cause}")))?;
        if encoded.len() > MAX_GENESIS_JSON_BYTES {
            return Err(error("genesis JSON exceeds its size bound"));
        }
        Ok(encoded)
    }

    pub fn digest(&self) -> &str {
        &self.genesis_sha256
    }

    pub fn reconstruct_evidence_set(&self) -> ContractResult<CollaborationLearningEvidenceSetV1> {
        let reconstructed = self.clone().reconstruct()?;
        CollaborationLearningEvidenceSetV1::new(
            reconstructed.candidate,
            reconstructed.baseline,
            reconstructed.config,
            reconstructed.frozen_holdout,
        )
    }

    fn payload_sha256(&self) -> ContractResult<String> {
        collaboration_learning_sha256(
            GENESIS_HASH_DOMAIN,
            &(
                self.schema.as_str(),
                &self.candidate,
                &self.baseline,
                &self.config,
                &self.frozen_holdout,
            ),
            "offline genesis",
        )
    }

    fn reconstruct(self) -> ContractResult<Self> {
        if self.schema != COLLABORATION_LEARNING_OFFLINE_GENESIS_SCHEMA {
            return Err(error("genesis schema is invalid"));
        }
        let genesis_sha256 = self.genesis_sha256;
        let reconstructed = Self::new(
            self.candidate,
            self.baseline,
            self.config,
            self.frozen_holdout,
        )?;
        if reconstructed.genesis_sha256 != genesis_sha256 {
            return Err(error("genesis digest is invalid"));
        }
        Ok(reconstructed)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "payload",
    rename_all = "snake_case",
    deny_unknown_fields
)]
enum CollaborationLearningOfflineObservationV1 {
    Pair(Box<CollaborationLearningPairV1>),
    Censor(Box<CollaborationLearningCensorReceiptV1>),
}

impl CollaborationLearningOfflineObservationV1 {
    fn reconstruct(self) -> ContractResult<Self> {
        match self {
            Self::Pair(pair) => Ok(Self::Pair(Box::new((*pair).reconstruct()?))),
            Self::Censor(censor) => Ok(Self::Censor(Box::new((*censor).reconstruct()?))),
        }
    }

    fn binding(&self) -> &CollaborationLearningComparisonBindingV1 {
        match self {
            Self::Pair(pair) => pair.binding(),
            Self::Censor(censor) => censor.binding(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CollaborationLearningOfflineEntryV1 {
    schema: String,
    genesis_sha256: String,
    sequence: u16,
    previous_entry_sha256: Option<String>,
    binding_sha256: String,
    observation: CollaborationLearningOfflineObservationV1,
    entry_sha256: String,
}

impl CollaborationLearningOfflineEntryV1 {
    pub fn pair(
        genesis_sha256: String,
        sequence: u16,
        previous_entry_sha256: Option<String>,
        pair: CollaborationLearningPairV1,
    ) -> ContractResult<Self> {
        Self::build(
            genesis_sha256,
            sequence,
            previous_entry_sha256,
            CollaborationLearningOfflineObservationV1::Pair(Box::new(pair)),
        )
    }

    pub fn censor(
        genesis_sha256: String,
        sequence: u16,
        previous_entry_sha256: Option<String>,
        censor: CollaborationLearningCensorReceiptV1,
    ) -> ContractResult<Self> {
        Self::build(
            genesis_sha256,
            sequence,
            previous_entry_sha256,
            CollaborationLearningOfflineObservationV1::Censor(Box::new(censor)),
        )
    }

    pub fn from_json(encoded: &str) -> ContractResult<Self> {
        if encoded.len() > MAX_ENTRY_JSON_BYTES {
            return Err(error("entry JSON exceeds its size bound"));
        }
        let decoded: Self = serde_json::from_str(encoded)
            .map_err(|cause| error(format!("entry JSON is invalid: {cause}")))?;
        decoded.reconstruct()
    }

    pub fn to_json(&self) -> ContractResult<String> {
        let reconstructed = self.clone().reconstruct()?;
        let encoded = serde_json::to_string(&reconstructed)
            .map_err(|cause| error(format!("entry JSON encoding failed: {cause}")))?;
        if encoded.len() > MAX_ENTRY_JSON_BYTES {
            return Err(error("entry JSON exceeds its size bound"));
        }
        Ok(encoded)
    }

    pub fn digest(&self) -> &str {
        &self.entry_sha256
    }

    pub fn genesis_sha256(&self) -> &str {
        &self.genesis_sha256
    }

    pub fn sequence(&self) -> u16 {
        self.sequence
    }

    pub fn previous_entry_sha256(&self) -> Option<&str> {
        self.previous_entry_sha256.as_deref()
    }

    pub fn binding_sha256(&self) -> &str {
        &self.binding_sha256
    }

    pub fn physical_run_sha256(&self) -> ContractResult<Vec<String>> {
        let mut identities = match &self.observation {
            CollaborationLearningOfflineObservationV1::Pair(pair) => {
                pair.physical_run_identities()?.into_iter().collect()
            }
            CollaborationLearningOfflineObservationV1::Censor(censor) => {
                censor.physical_run_sha256().to_vec()
            }
        };
        identities.sort();
        Ok(identities)
    }

    pub fn censor_reason(&self) -> Option<CollaborationLearningCensorReasonV1> {
        match &self.observation {
            CollaborationLearningOfflineObservationV1::Pair(_) => None,
            CollaborationLearningOfflineObservationV1::Censor(censor) => Some(censor.reason()),
        }
    }

    fn build(
        genesis_sha256: String,
        sequence: u16,
        previous_entry_sha256: Option<String>,
        observation: CollaborationLearningOfflineObservationV1,
    ) -> ContractResult<Self> {
        validate_collaboration_learning_sha256(&genesis_sha256, "offline genesis")?;
        if sequence == 0 || usize::from(sequence) > MAX_REPLAY_ENTRIES {
            return Err(error("entry sequence is outside its fixed bound"));
        }
        match (&previous_entry_sha256, sequence) {
            (None, 1) => {}
            (Some(previous), 2..) => {
                validate_collaboration_learning_sha256(previous, "previous offline entry")?;
            }
            _ => return Err(error("entry predecessor does not match its sequence")),
        }
        let observation = observation.reconstruct()?;
        let binding_sha256 = observation.binding().digest().to_string();
        let mut entry = Self {
            schema: COLLABORATION_LEARNING_OFFLINE_ENTRY_SCHEMA.into(),
            genesis_sha256,
            sequence,
            previous_entry_sha256,
            binding_sha256,
            observation,
            entry_sha256: String::new(),
        };
        entry.entry_sha256 = entry.payload_sha256()?;
        Ok(entry)
    }

    fn payload_sha256(&self) -> ContractResult<String> {
        collaboration_learning_sha256(
            ENTRY_HASH_DOMAIN,
            &(
                self.schema.as_str(),
                self.genesis_sha256.as_str(),
                self.sequence,
                self.previous_entry_sha256.as_deref(),
                self.binding_sha256.as_str(),
                &self.observation,
            ),
            "offline entry",
        )
    }

    fn reconstruct(self) -> ContractResult<Self> {
        if self.schema != COLLABORATION_LEARNING_OFFLINE_ENTRY_SCHEMA {
            return Err(error("entry schema is invalid"));
        }
        let binding_sha256 = self.binding_sha256;
        let entry_sha256 = self.entry_sha256;
        let reconstructed = Self::build(
            self.genesis_sha256,
            self.sequence,
            self.previous_entry_sha256,
            self.observation,
        )?;
        if reconstructed.binding_sha256 != binding_sha256
            || reconstructed.entry_sha256 != entry_sha256
        {
            return Err(error("entry digest or binding is invalid"));
        }
        Ok(reconstructed)
    }
}

#[derive(Debug)]
pub struct CollaborationLearningOfflineReplayV1 {
    evidence_set: CollaborationLearningEvidenceSetV1,
    aggregate: CollaborationLearningAggregateV1,
    head_sha256: Option<String>,
    entry_count: u16,
}

impl CollaborationLearningOfflineReplayV1 {
    pub fn reconstruct(
        genesis: &CollaborationLearningOfflineGenesisV1,
        entries: &[CollaborationLearningOfflineEntryV1],
    ) -> ContractResult<Self> {
        if entries.len() > MAX_REPLAY_ENTRIES {
            return Err(error("replay exceeds its fixed entry bound"));
        }
        let genesis = genesis.clone().reconstruct()?;
        let mut evidence_set = genesis.reconstruct_evidence_set()?;
        let mut head_sha256: Option<String> = None;
        let mut aggregate = None;
        for (index, entry) in entries.iter().enumerate() {
            let entry = entry.clone().reconstruct()?;
            let sequence = u16::try_from(index + 1)
                .map_err(|_| error("replay sequence exceeds its fixed bound"))?;
            if entry.genesis_sha256 != genesis.genesis_sha256
                || entry.sequence != sequence
                || entry.previous_entry_sha256.as_deref() != head_sha256.as_deref()
            {
                return Err(error("replay chain is not contiguous from genesis"));
            }
            let entry_sha256 = entry.entry_sha256.clone();
            match entry.observation {
                CollaborationLearningOfflineObservationV1::Pair(pair) => {
                    evidence_set.append_pair(*pair)?;
                }
                CollaborationLearningOfflineObservationV1::Censor(censor) => {
                    evidence_set.append_censor(*censor)?;
                }
            }
            head_sha256 = Some(entry_sha256);
            let prefix_aggregate = evidence_set.aggregate()?;
            if index + 1 < entries.len()
                && prefix_aggregate.status() != CollaborationLearningAggregateStatusV1::Collecting
            {
                return Err(error("replay continued after a terminal aggregate"));
            }
            aggregate = Some(prefix_aggregate);
        }
        let aggregate = match aggregate {
            Some(aggregate) => aggregate,
            None => evidence_set.aggregate()?,
        };
        Ok(Self {
            evidence_set,
            aggregate,
            head_sha256,
            entry_count: u16::try_from(entries.len())
                .map_err(|_| error("replay entry count exceeds its fixed bound"))?,
        })
    }

    pub fn evidence_set(&self) -> &CollaborationLearningEvidenceSetV1 {
        &self.evidence_set
    }

    pub fn into_evidence_set(self) -> CollaborationLearningEvidenceSetV1 {
        self.evidence_set
    }

    pub fn aggregate(&self) -> &CollaborationLearningAggregateV1 {
        &self.aggregate
    }

    pub fn head_sha256(&self) -> Option<&str> {
        self.head_sha256.as_deref()
    }

    pub fn entry_count(&self) -> u16 {
        self.entry_count
    }
}

fn error(message: impl Into<String>) -> CollaborationLearningError {
    CollaborationLearningError::new(format!(
        "collaboration learning offline adapter {}",
        message.into()
    ))
}
