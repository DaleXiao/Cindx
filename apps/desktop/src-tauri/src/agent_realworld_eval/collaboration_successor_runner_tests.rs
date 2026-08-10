use super::*;
use std::cell::Cell;

#[test]
fn agent_collaboration_successor_execution_contract_requires_explicit_authorize_command() {
    assert!(require_authorize_arguments([
        OsString::from("authorize"),
        OsString::from(AUTHORIZE_FLAG),
        OsString::from(PROTOCOL_ID),
    ])
    .is_ok());
    assert!(require_authorize_arguments([OsString::from("authorize")]).is_err());
    assert!(require_authorize_arguments([
        OsString::from("authorize"),
        OsString::from(AUTHORIZE_FLAG),
        OsString::from("wrong-protocol"),
    ])
    .is_err());
}

#[test]
fn agent_collaboration_successor_execution_contract_runs_exact_three_cells_to_ready() {
    let mut observed = Vec::new();
    let result = run_fixed_three_cell_controller(|ordinal| {
        observed.push(ordinal);
        Ok(if ordinal == 3 {
            CellStageResult::Ready("a".repeat(64))
        } else {
            CellStageResult::Continue
        })
    })
    .unwrap();
    assert_eq!(observed, vec![1, 2, 3]);
    assert_eq!(result, ControllerTerminal::Ready("a".repeat(64)));
}

#[test]
fn agent_collaboration_successor_execution_contract_baseline_failure_stops_without_retry() {
    let mut observed = Vec::new();
    let error = run_fixed_three_cell_controller(|ordinal| {
        observed.push(ordinal);
        Err("baseline_non_positive".to_string())
    })
    .unwrap_err();
    assert_eq!(error, "baseline_non_positive");
    assert_eq!(observed, vec![1]);
}

#[test]
fn agent_collaboration_successor_execution_contract_candidate_failure_never_opens_holdout() {
    let mut observed = Vec::new();
    let error = run_fixed_three_cell_controller(|ordinal| {
        observed.push(ordinal);
        if ordinal == 2 {
            Err("candidate_non_positive".to_string())
        } else {
            Ok(CellStageResult::Continue)
        }
    })
    .unwrap_err();
    assert_eq!(error, "candidate_non_positive");
    assert_eq!(observed, vec![1, 2]);
}

#[test]
fn agent_collaboration_successor_execution_contract_holdout_can_only_freeze_or_finish() {
    let mut observed = Vec::new();
    let result = run_fixed_three_cell_controller(|ordinal| {
        observed.push(ordinal);
        Ok(if ordinal == 3 {
            CellStageResult::Frozen("no_uplift".into())
        } else {
            CellStageResult::Continue
        })
    })
    .unwrap();
    assert_eq!(observed, vec![1, 2, 3]);
    assert_eq!(result, ControllerTerminal::Frozen("no_uplift".into()));
    assert!(run_fixed_three_cell_controller(|_| Ok(CellStageResult::Ready("bad".into()))).is_err());
}

#[test]
fn agent_collaboration_successor_execution_contract_binary_drift_blocks_callback() {
    let called = Cell::new(0_u8);
    let expected = sha256_hex(b"authorized-runner");
    let result = run_after_exact_runner_binding(&expected, b"drifted-runner", || {
        called.set(called.get() + 1);
        Ok(())
    });
    assert!(result.is_err());
    assert_eq!(called.get(), 0);

    run_after_exact_runner_binding(&expected, b"authorized-runner", || {
        called.set(called.get() + 1);
        Ok(())
    })
    .unwrap();
    assert_eq!(called.get(), 1);
}

#[test]
fn agent_collaboration_successor_execution_contract_reads_only_regular_binary_bytes() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("runner");
    fs::write(&path, b"runner-bytes").unwrap();
    assert_eq!(read_exact_runner_binary(&path).unwrap(), b"runner-bytes");
    assert!(read_exact_runner_binary(temp.path()).is_err());

    #[cfg(unix)]
    {
        let link = temp.path().join("runner-link");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        assert!(read_exact_runner_binary(&link).is_err());
    }
}
