//! OS-level shell confinement built on macOS Seatbelt (`sandbox-exec`).
//!
//! The confinement is opt-in per session: the default mode is [`SandboxMode::FullAccess`],
//! which produces the exact argv the shell tool has always used (no wrapper process).
//! Any other mode wraps the command in `/usr/bin/sandbox-exec` with a deterministic
//! SBPL profile derived only from the mode and the workspace root, so the wrapping
//! is pure and replayable in tests without executing `sandbox-exec`.

use std::path::Path;

use crate::Metadata;

/// Metadata key carrying the effective sandbox mode into a tool invocation.
pub const SANDBOX_MODE_METADATA_KEY: &str = "sandbox_mode";

/// Session-scoped confinement level for shell execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SandboxMode {
    /// Every filesystem write is denied except `/dev/null`.
    ReadOnly,
    /// Filesystem writes are only allowed inside the workspace and `/tmp`.
    WorkspaceWrite,
    /// No confinement: the shell argv is left exactly as-is.
    #[default]
    FullAccess,
}

impl SandboxMode {
    /// Parse the persisted label; unknown values yield `None`.
    pub fn parse(value: &str) -> Option<SandboxMode> {
        match value {
            "read-only" => Some(Self::ReadOnly),
            "workspace-write" => Some(Self::WorkspaceWrite),
            "full" => Some(Self::FullAccess),
            _ => None,
        }
    }

    /// The persisted label used in events and invocation metadata.
    pub fn label(self) -> &'static str {
        match self {
            Self::ReadOnly => "read-only",
            Self::WorkspaceWrite => "workspace-write",
            Self::FullAccess => "full",
        }
    }
}

/// The effective sandbox mode recorded on a tool invocation. Absent or unknown
/// values stay unconfined ([`SandboxMode::FullAccess`]), preserving the
/// historical default behavior.
pub fn sandbox_mode_from_metadata(metadata: &Metadata) -> SandboxMode {
    metadata
        .get(SANDBOX_MODE_METADATA_KEY)
        .and_then(|value| SandboxMode::parse(value))
        .unwrap_or_default()
}

/// Escape a path for embedding in an SBPL quoted string literal.
pub fn sbpl_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Deterministic SBPL profile fragments for a mode. Joining the fragments
/// produces the full profile text passed to `sandbox-exec -p`.
/// [`SandboxMode::FullAccess`] returns no fragments (no wrapper at all).
pub fn seatbelt_profile_args(mode: SandboxMode, workspace_root: &Path) -> Vec<String> {
    match mode {
        SandboxMode::FullAccess => Vec::new(),
        SandboxMode::ReadOnly => vec![
            "(version 1)".to_string(),
            "(allow default)".to_string(),
            "(deny file-write*)".to_string(),
            "(allow file-write* (literal \"/dev/null\"))".to_string(),
        ],
        SandboxMode::WorkspaceWrite => {
            let mut fragments = seatbelt_profile_args(SandboxMode::ReadOnly, workspace_root);
            fragments.push(format!(
                "(allow file-write* (subpath \"{}\") (subpath \"/tmp\"))",
                sbpl_escape(&workspace_root.display().to_string())
            ));
            fragments
        }
    }
}

/// The argv used to run a shell command under the session's sandbox mode.
/// [`SandboxMode::FullAccess`] returns the exact historical argv
/// (`/bin/zsh -fc <command>`); every other mode wraps the command in
/// `/usr/bin/sandbox-exec -p <profile> -- /bin/zsh -c <command>`.
pub fn confined_argv(mode: SandboxMode, workspace_root: &Path, command: &str) -> Vec<String> {
    let profile = seatbelt_profile_args(mode, workspace_root);
    if profile.is_empty() {
        return vec![
            "/bin/zsh".to_string(),
            "-fc".to_string(),
            command.to_string(),
        ];
    }
    vec![
        "/usr/bin/sandbox-exec".to_string(),
        "-p".to_string(),
        profile.join(""),
        "--".to_string(),
        "/bin/zsh".to_string(),
        "-c".to_string(),
        command.to_string(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace() -> std::path::PathBuf {
        std::path::PathBuf::from("/Users/dev/project")
    }

    #[test]
    fn full_access_profile_is_empty_and_confined_argv_is_unwrapped() {
        assert_eq!(
            seatbelt_profile_args(SandboxMode::FullAccess, &workspace()),
            Vec::<String>::new()
        );
        assert_eq!(
            confined_argv(SandboxMode::FullAccess, &workspace(), "cargo test"),
            vec!["/bin/zsh", "-fc", "cargo test"]
        );
    }

    #[test]
    fn read_only_profile_denies_all_file_writes_except_dev_null() {
        let fragments = seatbelt_profile_args(SandboxMode::ReadOnly, &workspace());
        assert_eq!(
            fragments.join(""),
            "(version 1)(allow default)(deny file-write*)\
             (allow file-write* (literal \"/dev/null\"))"
        );
        // Determinism: the same inputs always produce the same profile.
        assert_eq!(
            fragments,
            seatbelt_profile_args(SandboxMode::ReadOnly, &workspace())
        );
    }

    #[test]
    fn workspace_write_profile_allows_workspace_and_tmp_writes() {
        let fragments = seatbelt_profile_args(SandboxMode::WorkspaceWrite, &workspace());
        assert_eq!(
            fragments.join(""),
            "(version 1)(allow default)(deny file-write*)\
             (allow file-write* (literal \"/dev/null\"))\
             (allow file-write* (subpath \"/Users/dev/project\") (subpath \"/tmp\"))"
        );
    }

    #[test]
    fn workspace_paths_are_sbpl_escaped() {
        let hostile = std::path::PathBuf::from("/Users/dev/\"quoted\"\\dir");
        let profile = seatbelt_profile_args(SandboxMode::WorkspaceWrite, &hostile).join("");
        assert!(profile.contains("(subpath \"/Users/dev/\\\"quoted\\\"\\\\dir\")"));
        assert!(!profile.contains("\"quoted\"\\d"));
        assert_eq!(sbpl_escape("a\"b\\c"), "a\\\"b\\\\c");
    }

    #[test]
    fn confined_argv_wraps_confined_modes_with_the_mode_profile() {
        for mode in [SandboxMode::ReadOnly, SandboxMode::WorkspaceWrite] {
            let argv = confined_argv(mode, &workspace(), "echo hi");
            let expected: Vec<String> = vec![
                "/usr/bin/sandbox-exec".to_string(),
                "-p".to_string(),
                seatbelt_profile_args(mode, &workspace()).join(""),
                "--".to_string(),
                "/bin/zsh".to_string(),
                "-c".to_string(),
                "echo hi".to_string(),
            ];
            assert_eq!(argv, expected);
        }
    }

    #[test]
    fn sandbox_mode_labels_round_trip_and_reject_unknown_values() {
        for mode in [
            SandboxMode::ReadOnly,
            SandboxMode::WorkspaceWrite,
            SandboxMode::FullAccess,
        ] {
            assert_eq!(SandboxMode::parse(mode.label()), Some(mode));
        }
        assert_eq!(SandboxMode::parse(""), None);
        assert_eq!(SandboxMode::parse("nope"), None);
        assert_eq!(SandboxMode::default(), SandboxMode::FullAccess);
    }

    #[test]
    fn invocation_metadata_without_a_valid_mode_stays_unconfined() {
        assert_eq!(
            sandbox_mode_from_metadata(&Metadata::new()),
            SandboxMode::FullAccess
        );
        let mut metadata = Metadata::new();
        metadata.insert(SANDBOX_MODE_METADATA_KEY.to_string(), "bogus".to_string());
        assert_eq!(
            sandbox_mode_from_metadata(&metadata),
            SandboxMode::FullAccess
        );
        metadata.insert(
            SANDBOX_MODE_METADATA_KEY.to_string(),
            "workspace-write".to_string(),
        );
        assert_eq!(
            sandbox_mode_from_metadata(&metadata),
            SandboxMode::WorkspaceWrite
        );
    }
}
