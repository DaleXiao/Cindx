use super::*;

#[test]
fn runtime_status_exposes_expected_modes() {
    let status = runtime_status_for_root(workspace_root());

    assert_eq!(status.app_version, env!("CARGO_PKG_VERSION"));
    assert!(!status.provider_ready);
    assert!(status
        .orchestration_modes
        .contains(&"plan_execute_review".to_string()));
    assert!(status.registered_tools.contains(&"shell.run".to_string()));
}

#[test]
fn runtime_status_exposes_authoritative_agent_run_budgets() {
    let status = runtime_status_for_root(workspace_root());

    for (effort, exposed) in [
        ("fast", status.agent_run_budgets.fast),
        ("default", status.agent_run_budgets.default),
        ("high", status.agent_run_budgets.high),
        ("xhigh", status.agent_run_budgets.xhigh),
    ] {
        let authoritative = RunBudget::for_effort(effort);
        assert_eq!(
            exposed.max_duration_ms,
            u64::try_from(authoritative.max_duration.as_millis()).unwrap()
        );
        assert_eq!(exposed.max_model_calls, authoritative.max_model_calls);
        assert_eq!(exposed.max_tool_calls, authoritative.max_tool_calls);
    }
}
