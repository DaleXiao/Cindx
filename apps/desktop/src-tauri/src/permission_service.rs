use agent_core::{PermissionDecision, PermissionRequest, PermissionRisk, TaskId};
use agent_storage::{PermissionStore, SqliteStore, StorageError};

pub(crate) fn agent_session_permission_granted(
    store: &SqliteStore,
    task_id: &TaskId,
    request: &PermissionRequest,
    session_id: Option<&str>,
) -> Result<bool, StorageError> {
    if request.risk == PermissionRisk::Destructive {
        return Ok(false);
    }
    let Some(session_id) = session_id else {
        return Ok(false);
    };
    store.has_session_permission_capability(
        task_id,
        session_id,
        request,
        permission_requires_exact_scope(request),
    )
}

#[cfg(test)]
fn permission_capability_matches(
    granted: &PermissionRequest,
    requested: &PermissionRequest,
) -> bool {
    granted.task_id == requested.task_id
        && granted.risk == requested.risk
        && granted.action == requested.action
        && (!permission_requires_exact_scope(requested) || granted.scope == requested.scope)
        && requested.risk != PermissionRisk::Destructive
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

    #[test]
    fn session_capability_requires_the_same_action_and_risk() {
        let read = request("file.read", PermissionRisk::Read, "README.md");
        let another_read = request("file.read", PermissionRisk::Read, "src/lib.rs");
        let write = request("file.write", PermissionRisk::Write, "README.md");
        let shell = request("shell.run", PermissionRisk::Execute, "/workspace");

        assert!(permission_capability_matches(&read, &another_read));
        assert!(!permission_capability_matches(&read, &write));
        assert!(!permission_capability_matches(&read, &shell));
    }

    #[test]
    fn shell_session_capability_is_bound_to_its_working_directory() {
        let workspace = request("shell.run", PermissionRisk::Execute, "/workspace");
        let same_workspace = request("shell.run", PermissionRisk::Execute, "/workspace");
        let another_workspace = request("shell.run", PermissionRisk::Execute, "/other");

        assert!(permission_capability_matches(&workspace, &same_workspace));
        assert!(!permission_capability_matches(
            &workspace,
            &another_workspace
        ));
    }

    #[test]
    fn destructive_capabilities_are_never_reused() {
        let destructive = request(
            "computer.key",
            PermissionRisk::Destructive,
            "shortcut:cmd+delete",
        );

        assert!(!permission_capability_matches(&destructive, &destructive));
    }
}
