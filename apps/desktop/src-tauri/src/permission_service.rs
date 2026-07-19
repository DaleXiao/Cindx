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
    Ok(store
        .list_permission_audits_for_session(task_id, session_id, None, 0)?
        .into_iter()
        .any(|audit| {
            audit.resolution.as_ref().is_some_and(|resolution| {
                resolution.decision == PermissionDecision::AllowForSession
            })
        }))
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
