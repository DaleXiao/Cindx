use super::*;
use crate::{
    collaboration_learning_physical_run_sha256, CollaborationLearningOfflineEntryV1,
    CollaborationLearningOfflineGenesisV1, CollaborationLearningOfflineReplayV1,
};

fn offline_genesis(
    holdout: &CollaborationLearningComparisonBindingV1,
) -> CollaborationLearningOfflineGenesisV1 {
    let frozen_config = config(1);
    CollaborationLearningOfflineGenesisV1::new(
        candidate(std::slice::from_ref(holdout), &frozen_config),
        baseline(),
        frozen_config,
        vec![holdout.clone()],
    )
    .unwrap()
}

#[test]
fn agent_collaboration_learning_offline_adapter_contract_round_trips_and_reconstructs_aggregate() {
    println!("{}", crate::COLLABORATION_LEARNING_OFFLINE_GENESIS_SCHEMA);
    let holdout = comparison(
        CollaborationLearningSplitV1::Holdout,
        2,
        CollaborationLearningArmOrderV1::WorkflowFirst,
    );
    let genesis = offline_genesis(&holdout);
    let decoded_genesis =
        CollaborationLearningOfflineGenesisV1::from_json(&genesis.to_json().unwrap()).unwrap();
    assert_eq!(decoded_genesis.digest(), genesis.digest());

    let train_pair = candidate_pair(
        comparison(
            CollaborationLearningSplitV1::Train,
            2,
            CollaborationLearningArmOrderV1::DirectFirst,
        ),
        false,
        AgentOutcomeTerminalStatusV1::Completed,
        true,
        true,
    );
    let holdout_pair = candidate_pair(
        holdout.clone(),
        false,
        AgentOutcomeTerminalStatusV1::Completed,
        true,
        true,
    );
    let first = CollaborationLearningOfflineEntryV1::pair(
        genesis.digest().into(),
        1,
        None,
        train_pair.clone(),
    )
    .unwrap();
    let second = CollaborationLearningOfflineEntryV1::pair(
        genesis.digest().into(),
        2,
        Some(first.digest().into()),
        holdout_pair.clone(),
    )
    .unwrap();
    let decoded_first =
        CollaborationLearningOfflineEntryV1::from_json(&first.to_json().unwrap()).unwrap();
    assert_eq!(decoded_first.digest(), first.digest());

    let replay = CollaborationLearningOfflineReplayV1::reconstruct(
        &decoded_genesis,
        &[decoded_first, second.clone()],
    )
    .unwrap();
    assert_eq!(replay.entry_count(), 2);
    assert_eq!(replay.head_sha256(), Some(second.digest()));
    assert_eq!(
        replay.aggregate().status(),
        CollaborationLearningAggregateStatusV1::ReadyForReview
    );

    let mut direct = evidence(holdout, 1);
    direct.append_pair(train_pair).unwrap();
    direct.append_pair(holdout_pair).unwrap();
    let direct_aggregate = direct.aggregate().unwrap();
    assert_eq!(
        replay.aggregate().review_evidence_sha256(),
        direct_aggregate.review_evidence_sha256()
    );

    let after_ready = CollaborationLearningOfflineEntryV1::pair(
        genesis.digest().into(),
        3,
        Some(second.digest().into()),
        candidate_pair(
            comparison(
                CollaborationLearningSplitV1::Train,
                3,
                CollaborationLearningArmOrderV1::WorkflowFirst,
            ),
            false,
            AgentOutcomeTerminalStatusV1::Completed,
            true,
            true,
        ),
    )
    .unwrap();
    assert!(CollaborationLearningOfflineReplayV1::reconstruct(
        &genesis,
        &[first, second, after_ready]
    )
    .is_err());
}

#[test]
fn agent_collaboration_learning_offline_adapter_contract_retains_censor_and_dedupes_physical_run() {
    let holdout = comparison(
        CollaborationLearningSplitV1::Holdout,
        3,
        CollaborationLearningArmOrderV1::WorkflowFirst,
    );
    let genesis = offline_genesis(&holdout);
    let pair = candidate_pair(
        comparison(
            CollaborationLearningSplitV1::Train,
            2,
            CollaborationLearningArmOrderV1::DirectFirst,
        ),
        false,
        AgentOutcomeTerminalStatusV1::Completed,
        true,
        true,
    );
    let physical_run_sha256 =
        collaboration_learning_physical_run_sha256(&pair.direct.outcome.lifecycle.agent_run_id)
            .unwrap();
    let other_physical_run_sha256 =
        collaboration_learning_physical_run_sha256("fixture-censored-other-arm").unwrap();
    let censor_binding = comparison(
        CollaborationLearningSplitV1::Train,
        3,
        CollaborationLearningArmOrderV1::WorkflowFirst,
    );
    assert!(CollaborationLearningCensorReceiptV1::for_physical_runs(
        censor_binding.clone(),
        digest('5'),
        Vec::new(),
        CollaborationLearningCensorReasonV1::IncompleteInstrumentation,
    )
    .is_err());
    assert!(CollaborationLearningCensorReceiptV1::for_physical_runs(
        censor_binding.clone(),
        digest('5'),
        vec![physical_run_sha256.clone(), physical_run_sha256.clone()],
        CollaborationLearningCensorReasonV1::IncompleteInstrumentation,
    )
    .is_err());
    assert!(CollaborationLearningCensorReceiptV1::for_physical_runs(
        censor_binding.clone(),
        digest('5'),
        vec![digest('1'), digest('2'), digest('3')],
        CollaborationLearningCensorReasonV1::IncompleteInstrumentation,
    )
    .is_err());
    let censor = CollaborationLearningCensorReceiptV1::for_physical_runs(
        censor_binding,
        digest('5'),
        vec![
            other_physical_run_sha256.clone(),
            physical_run_sha256.clone(),
        ],
        CollaborationLearningCensorReasonV1::IncompleteInstrumentation,
    )
    .unwrap();
    let mut expected_physical_runs = vec![physical_run_sha256, other_physical_run_sha256];
    expected_physical_runs.sort();
    assert_eq!(censor.physical_run_sha256(), expected_physical_runs);
    let censor_entry = CollaborationLearningOfflineEntryV1::censor(
        genesis.digest().into(),
        1,
        None,
        censor.clone(),
    )
    .unwrap();
    let censored = CollaborationLearningOfflineReplayV1::reconstruct(
        &genesis,
        std::slice::from_ref(&censor_entry),
    )
    .unwrap();
    assert_eq!(censored.evidence_set().censored_count(), 1);
    assert_eq!(
        censored.aggregate().freeze_reason(),
        Some(CollaborationLearningFreezeReasonV1::InvalidInstrumentation)
    );

    let pair_after_censor = CollaborationLearningOfflineEntryV1::pair(
        genesis.digest().into(),
        2,
        Some(censor_entry.digest().into()),
        pair.clone(),
    )
    .unwrap();
    assert!(CollaborationLearningOfflineReplayV1::reconstruct(
        &genesis,
        &[censor_entry, pair_after_censor]
    )
    .is_err());

    let pair_entry =
        CollaborationLearningOfflineEntryV1::pair(genesis.digest().into(), 1, None, pair).unwrap();
    let censor_after_pair = CollaborationLearningOfflineEntryV1::censor(
        genesis.digest().into(),
        2,
        Some(pair_entry.digest().into()),
        censor,
    )
    .unwrap();
    assert!(CollaborationLearningOfflineReplayV1::reconstruct(
        &genesis,
        &[pair_entry, censor_after_pair]
    )
    .is_err());
}

#[test]
fn agent_collaboration_learning_offline_adapter_contract_rejects_tamper_partial_and_broken_chain() {
    let holdout = comparison(
        CollaborationLearningSplitV1::Holdout,
        4,
        CollaborationLearningArmOrderV1::WorkflowFirst,
    );
    let genesis = offline_genesis(&holdout);
    let pair = |replicate, order| {
        candidate_pair(
            comparison(CollaborationLearningSplitV1::Train, replicate, order),
            false,
            AgentOutcomeTerminalStatusV1::Completed,
            true,
            true,
        )
    };
    let first = CollaborationLearningOfflineEntryV1::pair(
        genesis.digest().into(),
        1,
        None,
        pair(2, CollaborationLearningArmOrderV1::DirectFirst),
    )
    .unwrap();
    let second = CollaborationLearningOfflineEntryV1::pair(
        genesis.digest().into(),
        2,
        Some(first.digest().into()),
        pair(3, CollaborationLearningArmOrderV1::WorkflowFirst),
    )
    .unwrap();
    assert!(CollaborationLearningOfflineReplayV1::reconstruct(
        &genesis,
        &[second.clone(), first.clone()]
    )
    .is_err());

    let encoded = first.to_json().unwrap();
    assert!(CollaborationLearningOfflineEntryV1::from_json(&encoded[..encoded.len() - 1]).is_err());
    let mut unknown: serde_json::Value = serde_json::from_str(&encoded).unwrap();
    unknown["unexpected"] = serde_json::json!(true);
    assert!(CollaborationLearningOfflineEntryV1::from_json(&unknown.to_string()).is_err());
    let mut tampered: serde_json::Value = serde_json::from_str(&encoded).unwrap();
    tampered["entry_sha256"] = serde_json::json!(digest('f'));
    assert!(CollaborationLearningOfflineEntryV1::from_json(&tampered.to_string()).is_err());
    assert!(CollaborationLearningOfflineEntryV1::from_json(&"x".repeat(512 * 1024 + 1)).is_err());

    let mut tampered_genesis: serde_json::Value =
        serde_json::from_str(&genesis.to_json().unwrap()).unwrap();
    tampered_genesis["genesis_sha256"] = serde_json::json!(digest('f'));
    assert!(
        CollaborationLearningOfflineGenesisV1::from_json(&tampered_genesis.to_string()).is_err()
    );
}
