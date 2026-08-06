use super::{
    is_sha256_hex, PostconditionTargetWitness, MAX_POSTCONDITION_TARGETS,
    POSTCONDITION_TARGET_DIGEST_DOMAIN,
};
use crate::task_contract::fingerprint;
use std::collections::BTreeSet;

impl PostconditionTargetWitness {
    pub(crate) fn scope_digest_for(lineage_scope: &str) -> Option<String> {
        (!lineage_scope.trim().is_empty()).then(|| target_scope_digest(lineage_scope))
    }

    pub(crate) fn capture(input_json: &str, lineage_scope: &str) -> Option<Self> {
        if lineage_scope.trim().is_empty() {
            return None;
        }
        let targets = crate::task_contract::structured_targets(input_json);
        if targets.is_empty() || targets.len() > MAX_POSTCONDITION_TARGETS {
            return None;
        }
        let scope_digest = target_scope_digest(lineage_scope);
        let exact_digests = targets
            .iter()
            .map(|target| target_digest(&scope_digest, target))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let witness = Self {
            scope_digest,
            exact_digests,
        };
        witness.contract_is_valid().then_some(witness)
    }

    pub(super) fn covered_by(&self, verifier: &Self) -> Vec<String> {
        if !self.contract_is_valid()
            || !verifier.contract_is_valid()
            || self.scope_digest != verifier.scope_digest
        {
            return Vec::new();
        }
        self.exact_digests
            .iter()
            .filter(|digest| verifier.exact_digests.binary_search(digest).is_ok())
            .cloned()
            .collect()
    }

    pub(super) fn fully_covered_by(&self, covered: &[String]) -> bool {
        self.contract_is_valid()
            && covered.windows(2).all(|pair| pair[0] < pair[1])
            && self
                .exact_digests
                .iter()
                .all(|digest| covered.binary_search(digest).is_ok())
    }

    pub(super) fn same_scope(&self, lineage_scope: &str) -> bool {
        !lineage_scope.trim().is_empty() && self.scope_digest == target_scope_digest(lineage_scope)
    }

    pub(crate) fn same_scope_digest(&self, scope_digest: &str) -> bool {
        is_sha256_hex(scope_digest) && self.scope_digest == scope_digest
    }

    pub(crate) fn contract_is_valid(&self) -> bool {
        is_sha256_hex(&self.scope_digest)
            && !self.exact_digests.is_empty()
            && self.exact_digests.len() <= MAX_POSTCONDITION_TARGETS
            && self.exact_digests.windows(2).all(|pair| pair[0] < pair[1])
            && self
                .exact_digests
                .iter()
                .all(|digest| is_sha256_hex(digest))
    }
}

fn target_scope_digest(lineage_scope: &str) -> String {
    fingerprint(&format!(
        "{POSTCONDITION_TARGET_DIGEST_DOMAIN}|scope|{lineage_scope}"
    ))
}

fn target_digest(scope_digest: &str, target: &str) -> String {
    fingerprint(&format!(
        "{POSTCONDITION_TARGET_DIGEST_DOMAIN}|{scope_digest}|{target}"
    ))
}
