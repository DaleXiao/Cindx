use agent_core::{
    permission_can_allow_session, permission_requires_exact_scope, prefix_rule_matches,
    ExecPrefixRule, PermissionDecision, PermissionRequest, PermissionRisk, TaskId,
};
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
    let require_capability_key = matches!(request.action.as_str(), "shell.run" | "process.start");
    if require_capability_key && !request.metadata.contains_key("command") {
        return Ok(false);
    }
    if store.has_session_permission_capability(
        task_id,
        session_id,
        request,
        permission_requires_exact_scope(request),
        require_capability_key,
    )? {
        return Ok(true);
    }
    // Prefix session grants: a shell grant may record a `command_prefix`
    // covering every later command whose shell tokens start with the granted
    // token sequence. Matching is fail-closed — `prefix_rule_matches` rejects
    // untokenizable and dangerous commands, so destructive commands always
    // prompt even under a matching prefix. Exact grants keep the SQL
    // capability-key path above and never consult this listing.
    if require_capability_key {
        if let Some(command) = request.metadata.get("command") {
            let grants =
                store.list_session_permission_grant_metadata(task_id, session_id, request)?;
            return Ok(grants.iter().any(|metadata| {
                metadata.get("command_prefix").is_some_and(|prefix| {
                    prefix_rule_matches(
                        &ExecPrefixRule {
                            prefix: prefix.clone(),
                        },
                        command,
                    )
                })
            }));
        }
    }
    Ok(false)
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
