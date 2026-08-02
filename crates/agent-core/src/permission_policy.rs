use crate::{PermissionRequest, PermissionRisk};

pub fn permission_capability_matches(
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

pub fn permission_can_allow_session(request: &PermissionRequest) -> bool {
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

pub fn permission_requires_exact_scope(request: &PermissionRequest) -> bool {
    request.action == "shell.run"
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{PermissionRequestId, TaskId};

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

    #[test]
    fn destructive_permissions_are_never_session_reusable() {
        let mut request = request("file.delete", PermissionRisk::Destructive, "/workspace");
        request
            .metadata
            .insert("session_reusable".to_string(), "true".to_string());

        assert!(!permission_can_allow_session(&request));
        assert!(!permission_capability_matches(&request, &request));
    }
}
