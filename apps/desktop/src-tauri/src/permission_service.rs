use agent_core::{PermissionDecision, PermissionRequest, PermissionRisk, TaskId};
use agent_storage::{PermissionStore, SqliteStore, StorageError};

pub(crate) fn agent_session_permission_granted(
    store: &SqliteStore,
    task_id: &TaskId,
    request: &PermissionRequest,
    session_id: Option<&str>,
) -> Result<bool, StorageError> {
    if !permission_can_allow_session(request) {
        return Ok(false);
    }
    let Some(session_id) = session_id else {
        return Ok(false);
    };
    let require_capability_key = request.action == "shell.run";
    if require_capability_key && !request.metadata.contains_key("command") {
        return Ok(false);
    }
    store.has_session_permission_capability(
        task_id,
        session_id,
        request,
        permission_requires_exact_scope(request),
        require_capability_key,
    )
}

pub(crate) fn permission_capability_matches(
    granted: &PermissionRequest,
    requested: &PermissionRequest,
) -> bool {
    granted.task_id == requested.task_id
        && granted.risk == requested.risk
        && granted.action == requested.action
        && (!permission_requires_exact_scope(requested) || granted.scope == requested.scope)
        && permission_capability_metadata_matches(granted, requested)
        && permission_can_allow_session(granted)
        && permission_can_allow_session(requested)
}

pub(crate) fn permission_can_allow_session(request: &PermissionRequest) -> bool {
    if request.risk == PermissionRisk::Destructive {
        return false;
    }
    let session_reusable = request.metadata.get("session_reusable").map(String::as_str);
    if request.action == "shell.run" {
        session_reusable == Some("true")
    } else {
        session_reusable != Some("false")
    }
}

fn permission_capability_metadata_matches(
    granted: &PermissionRequest,
    requested: &PermissionRequest,
) -> bool {
    requested.action != "shell.run"
        || requested.metadata.get("command").is_some_and(|command| {
            granted.metadata.get("command").map(String::as_str) == Some(command.as_str())
        })
}

fn permission_requires_exact_scope(request: &PermissionRequest) -> bool {
    request.action == "shell.run"
}

pub(crate) fn pending_agent_permissions_for_run(
    store: &SqliteStore,
    task_id: &TaskId,
    session_id: Option<&str>,
    agent_run_id: Option<&str>,
) -> Result<Vec<PermissionRequest>, StorageError> {
    let audits = if let Some(session_id) = session_id {
        store.list_permission_audits_for_session(task_id, session_id, agent_run_id, 0)?
    } else {
        store.list_permission_audits()?
    };
    let mut requests = audits
        .into_iter()
        .filter(|audit| audit.request.task_id == *task_id)
        .filter(|audit| audit.resolution.is_none())
        .filter(|audit| {
            agent_run_id.is_none_or(|agent_run_id| {
                audit
                    .request
                    .metadata
                    .get("agent_run_id")
                    .map(String::as_str)
                    == Some(agent_run_id)
            })
        })
        .map(|audit| audit.request)
        .collect::<Vec<_>>();
    requests.reverse();
    Ok(requests)
}

pub(crate) fn permission_risk_label(risk: &PermissionRisk) -> &'static str {
    match risk {
        PermissionRisk::Read => "read",
        PermissionRisk::Write => "write",
        PermissionRisk::Execute => "execute",
        PermissionRisk::Network => "network",
        PermissionRisk::Sensitive => "sensitive",
        PermissionRisk::Destructive => "destructive",
    }
}

pub(crate) fn permission_decision_label(decision: &PermissionDecision) -> &'static str {
    match decision {
        PermissionDecision::AllowOnce => "allow_once",
        PermissionDecision::AllowForSession => "allow_for_session",
        PermissionDecision::Deny => "deny",
    }
}

pub(crate) fn permission_decision_past_tense(decision: &PermissionDecision) -> &'static str {
    match decision {
        PermissionDecision::AllowOnce | PermissionDecision::AllowForSession => "approved",
        PermissionDecision::Deny => "denied",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_core::PermissionRequestId;

    fn request(action: &str, risk: PermissionRisk, scope: &str) -> PermissionRequest {
        PermissionRequest {
            id: PermissionRequestId(format!("{action}:{scope}")),
            task_id: TaskId("task".to_string()),
            risk,
            action: action.to_string(),
            reason: "test".to_string(),
            scope: scope.to_string(),
            metadata: Default::default(),
        }
    }

    fn shell_request(command: &str, scope: &str) -> PermissionRequest {
        let mut request = request("shell.run", PermissionRisk::Execute, scope);
        request
            .metadata
            .insert("command".to_string(), command.to_string());
        request
            .metadata
            .insert("session_reusable".to_string(), "true".to_string());
        request
    }

    #[test]
    fn shell_session_capability_is_bound_to_its_working_directory() {
        let workspace = shell_request("cargo test", "/workspace");
        let same_workspace = shell_request("cargo test", "/workspace");
        let another_workspace = shell_request("cargo test", "/other");

        assert!(permission_capability_matches(&workspace, &same_workspace));
        assert!(!permission_capability_matches(
            &workspace,
            &another_workspace
        ));
    }

    #[test]
    fn shell_session_capability_is_bound_to_the_exact_command() {
        let granted = shell_request("cargo test", "/workspace");
        let same = shell_request("cargo test", "/workspace");
        let hidden_side_effect =
            shell_request("printf '%s' \"$(touch should-not-run)\"", "/workspace");
        let missing_command = request("shell.run", PermissionRisk::Execute, "/workspace");

        assert!(permission_capability_matches(&granted, &same));
        assert!(!permission_capability_matches(
            &granted,
            &hidden_side_effect
        ));
        assert!(!permission_capability_matches(&granted, &missing_command));
    }

    #[test]
    fn non_reusable_shell_commands_never_match_a_session_capability() {
        let granted = shell_request("cargo test", "/workspace");
        let mut dynamic = shell_request("printf '%s' \"$(touch probe)\"", "/workspace");
        dynamic
            .metadata
            .insert("session_reusable".to_string(), "false".to_string());

        assert!(!permission_can_allow_session(&dynamic));
        assert!(!permission_capability_matches(&granted, &dynamic));
    }

    #[test]
    fn shell_session_reuse_requires_an_explicit_positive_marker() {
        let mut request = shell_request("cargo test", "/workspace");
        request.metadata.remove("session_reusable");
        assert!(!permission_can_allow_session(&request));

        request
            .metadata
            .insert("session_reusable".to_string(), "true".to_string());
        assert!(permission_can_allow_session(&request));
    }

}
