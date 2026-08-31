//! Shell command permission classification.
//!
//! Owns the decision of which risk level and approval eligibility a shell or
//! managed-process command carries: destructive patterns, piped code
//! execution, unrecognized executables, and positional script-file
//! executions. The shared shell tokenizer and session-reuse analysis stay in
//! `shell.rs`; this module consumes them to produce the classification.

use agent_core::PermissionRisk;

use crate::shell::{
    executable_basename, executables_for_segments, first_executable,
    shell_command_can_reuse_session_permission, shell_command_segments,
};

/// The permission classification of a shell command.
///
/// `risk` and `reason` keep the historical two-level model (Execute or
/// Destructive). The remaining flags tighten what Execute means without
/// changing it: a command that runs an unrecognized executable or an
/// interpreter with a positional script file stays Execute risk but must
/// still prompt under the session/all approval policies, can never derive a
/// prefix grant, and (for unrecognized executables) is approved one-shot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ShellCommandClassification {
    pub(crate) risk: PermissionRisk,
    pub(crate) reason: Option<&'static str>,
    /// False when the session/all approval policy must still prompt for this
    /// command instead of auto-approving it.
    pub(crate) auto_grant_eligible: bool,
    /// False when a session prefix grant would be coarser than the exact
    /// command (script-file executions and unrecognized executables).
    pub(crate) prefix_grant_eligible: bool,
    /// False when approval is one-shot: destructive commands and commands
    /// running an unrecognized executable.
    pub(crate) session_reusable: bool,
    /// The command performs network egress (curl/ssh/...): confidentiality
    /// and exfiltration are outside the destructive/non-destructive axis, so
    /// these never auto-grant even though they are Execute risk.
    pub(crate) network_egress: bool,
    /// The command reads a well-known sensitive location (keys, credentials,
    /// env secrets): never auto-grant for the same reason.
    pub(crate) sensitive_read: bool,
}

impl ShellCommandClassification {
    pub(crate) fn destructive(reason: &'static str) -> Self {
        Self {
            risk: PermissionRisk::Destructive,
            reason: Some(reason),
            auto_grant_eligible: false,
            prefix_grant_eligible: false,
            session_reusable: false,
            network_egress: false,
            sensitive_read: false,
        }
    }
}

/// Executables whose behavior is understood well enough to keep the
/// historical approval flow (session/all auto-grant, prefix grants, session
/// reuse). Anything outside this set still runs at Execute risk but prompts
/// on every invocation, is approved one-shot, and never derives a prefix
/// grant: an unrecognized binary is untrusted input, not a known tool.
const KNOWN_SHELL_EXECUTABLES: &[&str] = &[
    // shell builtins and basic commands
    ":",
    "echo",
    "printf",
    "cd",
    "pwd",
    "true",
    "false",
    "test",
    "read",
    "export",
    "set",
    "unset",
    "shift",
    "exit",
    "return",
    "wait",
    "break",
    "continue",
    "trap",
    "umask",
    "type",
    "let",
    "declare",
    "local",
    "readonly",
    "getopts",
    "jobs",
    "bg",
    "fg",
    "disown",
    "suspend",
    "times",
    "ulimit",
    // filesystem and text utilities
    "ls",
    "cat",
    "head",
    "tail",
    "less",
    "more",
    "grep",
    "egrep",
    "fgrep",
    "rg",
    "find",
    "fd",
    "file",
    "stat",
    "du",
    "df",
    "wc",
    "sort",
    "uniq",
    "cut",
    "paste",
    "tr",
    "column",
    "comm",
    "cmp",
    "diff",
    "sed",
    "awk",
    "md5",
    "md5sum",
    "shasum",
    "sha1sum",
    "sha256sum",
    "basename",
    "dirname",
    "realpath",
    "readlink",
    "touch",
    "mkdir",
    "cp",
    "mv",
    "ln",
    "chmod",
    "date",
    "uname",
    "hostname",
    "whoami",
    "id",
    "uptime",
    "env",
    "printenv",
    "sleep",
    "yes",
    "seq",
    "tee",
    "xxd",
    "od",
    "strings",
    "watch",
    "pbcopy",
    "pbpaste",
    "open",
    "which",
    "whereis",
    // archives
    "tar",
    "zip",
    "unzip",
    "gzip",
    "gunzip",
    "xz",
    "zcat",
    "bzip2",
    "bunzip",
    // data and system inspection
    "jq",
    "yq",
    "sqlite3",
    "lsof",
    "ps",
    "pgrep",
    "top",
    "htop",
    "netstat",
    "ifconfig",
    "ping",
    "dig",
    "nslookup",
    "host",
    "traceroute",
    "caffeinate",
    "sw_vers",
    "system_profiler",
    "sips",
    // network clients
    "curl",
    "wget",
    "ssh",
    "scp",
    "sftp",
    "rsync",
    // version control and toolchains
    "git",
    "cargo",
    "rustc",
    "rustup",
    "rustfmt",
    "node",
    "npm",
    "npx",
    "pnpm",
    "yarn",
    "bun",
    "deno",
    "python",
    "python3",
    "pip",
    "pip3",
    "uv",
    "make",
    "cmake",
    "ninja",
    "go",
    "docker",
    "kubectl",
    "pod",
    "brew",
    "gh",
    "protoc",
    "java",
    "javac",
    "gradle",
    "mvn",
    // Apple platform tools
    "xcrun",
    "xcodebuild",
    "swift",
    "swiftc",
    "codesign",
    "plutil",
    "defaults",
    "lipo",
    "otool",
    "security",
    // interpreters running positional script files stay Execute-risk, but the
    // script-file detection below removes their auto-grant eligibility.
    "sh",
    "bash",
    "zsh",
    "dash",
    "ksh",
    // command wrappers that are not skipped by first_executable_token
    "timeout",
    "nice",
    "ionice",
];

/// Interpreters whose positional script-file argument marks a script-file
/// execution (see [`segment_runs_script_file`]).
const SCRIPT_FILE_INTERPRETERS: &[&str] = &[
    "sh", "bash", "zsh", "dash", "ksh", "python", "python3", "node", "ruby", "perl", "php",
];

pub(crate) fn classify_shell_permission(command: &str) -> ShellCommandClassification {
    let segments = shell_command_segments(command);

    for (segment, executable) in segments.iter().zip(executables_for_segments(&segments)) {
        let Some(executable) = executable else {
            continue;
        };
        if opaque_interpreter_execution(segment, &executable) {
            return ShellCommandClassification::destructive("opaque interpreter execution");
        }
        match executable.as_str() {
            "rm" | "rmdir" | "unlink" | "shred" | "truncate" => {
                return ShellCommandClassification::destructive("filesystem deletion");
            }
            "sudo" | "su" => {
                return ShellCommandClassification::destructive("privilege escalation");
            }
            "shutdown" | "reboot" | "halt" | "poweroff" => {
                return ShellCommandClassification::destructive("system shutdown");
            }
            "kill" | "killall" | "pkill" => {
                return ShellCommandClassification::destructive("process termination");
            }
            "dd" | "mkfs" | "newfs" => {
                return ShellCommandClassification::destructive("raw disk mutation");
            }
            "find" if segment.iter().any(|token| token == "-delete") => {
                return ShellCommandClassification::destructive("recursive filesystem deletion");
            }
            "git" if git_segment_is_destructive(segment) => {
                return ShellCommandClassification::destructive("destructive git operation");
            }
            "diskutil" if diskutil_segment_is_destructive(segment) => {
                return ShellCommandClassification::destructive("disk mutation");
            }
            "launchctl"
                if segment.iter().any(|token| {
                    matches!(token.as_str(), "bootout" | "unload" | "remove" | "kill")
                }) =>
            {
                return ShellCommandClassification::destructive("service termination");
            }
            "defaults" if segment.iter().any(|token| token == "delete") => {
                return ShellCommandClassification::destructive("preference deletion");
            }
            "xargs"
                if segment.iter().any(|token| {
                    matches!(
                        executable_basename(token).as_str(),
                        "rm" | "rmdir" | "unlink"
                    )
                }) =>
            {
                return ShellCommandClassification::destructive("filesystem deletion");
            }
            _ => {}
        }
    }

    if pipes_output_to_interpreter(command) {
        return ShellCommandClassification::destructive("piped code execution");
    }

    let executables = segments
        .iter()
        .filter_map(|segment| first_executable(segment))
        .collect::<Vec<_>>();

    let downloads_code = executables
        .iter()
        .any(|name| matches!(name.as_str(), "curl" | "wget"));
    let executes_shell = executables.iter().any(|name| {
        matches!(
            name.as_str(),
            "sh" | "bash" | "zsh" | "dash" | "ksh" | "eval"
        )
    });
    if downloads_code && executes_shell {
        return ShellCommandClassification::destructive("downloaded code execution");
    }

    let mut recognized = true;
    let mut script_file_execution = false;
    for (segment, executable) in segments.iter().zip(executables_for_segments(&segments)) {
        let Some(executable) = executable else {
            continue;
        };
        if !KNOWN_SHELL_EXECUTABLES.contains(&executable.as_str()) {
            recognized = false;
        }
        if segment_runs_script_file(segment, &executable) {
            script_file_execution = true;
        }
    }

    // Execute risk, but unrecognized executables and positional script-file
    // executions never auto-grant, never derive a prefix grant, and
    // unrecognized executables are approved one-shot (fail-closed default).
    // Network egress and sensitive reads stay Execute risk but likewise never
    // auto-grant: confidentiality and exfiltration are their own axis.
    let restricted = !recognized || script_file_execution;
    let network_egress = command_performs_network_egress(&executables);
    let sensitive_read = command_reads_sensitive_location(command);
    ShellCommandClassification {
        risk: PermissionRisk::Execute,
        reason: None,
        auto_grant_eligible: !restricted && !network_egress && !sensitive_read,
        prefix_grant_eligible: !restricted,
        session_reusable: recognized && shell_command_can_reuse_session_permission(command),
        network_egress,
        sensitive_read,
    }
}

/// Executables whose primary effect is moving bytes across the network boundary.
fn command_performs_network_egress(executables: &[String]) -> bool {
    executables.iter().any(|name| {
        matches!(
            name.as_str(),
            "curl" | "wget" | "ssh" | "scp" | "sftp" | "nc" | "netcat" | "rsync"
        )
    })
}

/// Well-known sensitive locations and secret-store CLIs. Matching is
/// deliberately conservative (substring over the lowercased command): false
/// positives only cost a manual approval, false negatives would auto-grant a
/// credential read.
fn command_reads_sensitive_location(command: &str) -> bool {
    const PATTERNS: &[&str] = &[
        "~/.ssh",
        "~/.aws",
        "~/.gnupg",
        "~/.netrc",
        "~/.config/gcloud",
        "/etc/shadow",
        "/etc/sudoers",
        ".env",
        "id_rsa",
        "id_ed25519",
        "credentials",
        "security find-generic-password",
        "security find-internet-password",
    ];
    let lower = command.to_lowercase();
    PATTERNS.iter().any(|pattern| lower.contains(pattern))
}

/// True when the command pipes one stage's output into an interpreter
/// (`cat x | sh`, `printf ... | python3`, ...). Quote-aware; `||` is the OR
/// operator, not a pipe. Piped code execution is always destructive because
/// the executed code is composed at runtime from other stages' output.
fn pipes_output_to_interpreter(command: &str) -> bool {
    let mut characters = command.chars().peekable();
    let mut quote = None;
    let mut escaped = false;
    while let Some(character) = characters.next() {
        if escaped {
            escaped = false;
            continue;
        }
        if character == '\\' {
            escaped = true;
            continue;
        }
        if let Some(active_quote) = quote {
            if character == active_quote {
                quote = None;
            }
            continue;
        }
        if matches!(character, '\'' | '"') {
            quote = Some(character);
            continue;
        }
        if character != '|' {
            continue;
        }
        if characters.peek().is_some_and(|next| *next == '|') {
            // `||` is the OR operator, not a pipe.
            characters.next();
            continue;
        }
        // A real pipe (including zsh's `|&`): classify the destination.
        let mut remaining = String::new();
        for next in characters.by_ref() {
            remaining.push(next);
        }
        let destination = remaining.trim_start();
        let tokens = destination.split_whitespace();
        // Skip leading wrappers (`sudo`, `env`, `nohup`, ...) and their flags.
        let mut target: Option<&str> = None;
        for token in tokens {
            let basename = token
                .rsplit('/')
                .next()
                .unwrap_or(token)
                .to_ascii_lowercase();
            if matches!(basename.as_str(), "sudo" | "env" | "nohup" | "time") {
                continue;
            }
            if token.starts_with('-') {
                continue;
            }
            target = Some(token);
            break;
        }
        let Some(target) = target else {
            return false;
        };
        let basename = target
            .rsplit('/')
            .next()
            .unwrap_or(target)
            .trim_matches(|character: char| !character.is_ascii_alphanumeric() && character != '_')
            .to_ascii_lowercase();
        if SCRIPT_FILE_INTERPRETERS.contains(&basename.as_str()) || basename == "eval" {
            return true;
        }
        return false;
    }
    false
}

/// True when the segment runs an interpreter over a positional script file
/// (`bash scripts/build.sh`, `python3 verify.py`). Inline code (`-c`/`-e`)
/// is already destructive upstream, and `-m module` runs are module
/// executions, not script files. Running a script file stays Execute risk
/// (it is the user's own workspace code) but the classification removes its
/// auto-grant and prefix-grant eligibility, so a freshly planted script can
/// never ride a session policy to silent execution.
fn segment_runs_script_file(segment: &[String], executable: &str) -> bool {
    if !SCRIPT_FILE_INTERPRETERS.contains(&executable) {
        return false;
    }
    let Some(index) = segment
        .iter()
        .position(|token| executable_basename(token) == executable)
    else {
        return false;
    };
    let arguments = &segment[index + 1..];
    let mut module_execution = false;
    let mut positional = false;
    let mut skip_next = false;
    for argument in arguments {
        if skip_next {
            skip_next = false;
            continue;
        }
        if argument == "-" {
            continue;
        }
        if argument == "-m" || argument.starts_with("--module") {
            module_execution = true;
            skip_next = true;
            continue;
        }
        if argument.starts_with('-') {
            continue;
        }
        positional = true;
    }
    positional && !module_execution
}
fn opaque_interpreter_execution(segment: &[String], executable: &str) -> bool {
    let executable_index = segment
        .iter()
        .position(|token| executable_basename(token) == executable);
    let nested_interpreter_index = matches!(executable, "find" | "xargs")
        .then(|| {
            segment.iter().position(|token| {
                matches!(
                    executable_basename(token).as_str(),
                    "eval"
                        | "sh"
                        | "bash"
                        | "zsh"
                        | "dash"
                        | "ksh"
                        | "python"
                        | "python3"
                        | "node"
                        | "ruby"
                        | "perl"
                        | "php"
                )
            })
        })
        .flatten();
    let Some(executable_index) = nested_interpreter_index.or(executable_index) else {
        return false;
    };
    let executable = executable_basename(&segment[executable_index]);
    let arguments = &segment[executable_index.saturating_add(1)..];
    match executable.as_str() {
        "eval" => true,
        // Inline code (`-c`/`--command`) is opaque and stays destructive. A
        // positional script file is the user's own workspace code, so running it
        // stays Execute risk; the script-file classification then removes its
        // auto-grant and prefix-grant eligibility so it always prompts.
        "sh" | "bash" | "zsh" | "dash" | "ksh" => arguments.iter().any(|argument| {
            short_option_enables(argument, 'c')
                || argument == "--command"
                || argument.starts_with("--command=")
        }),
        "python" | "python3" | "node" | "ruby" | "perl" | "php" => {
            arguments.iter().any(|argument| {
                short_option_enables(argument, 'c')
                    || short_option_enables(argument, 'e')
                    || argument == "--eval"
                    || argument.starts_with("--eval=")
            })
        }
        _ => false,
    }
}

fn short_option_enables(argument: &str, option: char) -> bool {
    argument.starts_with('-')
        && !argument.starts_with("--")
        && argument.chars().skip(1).any(|value| value == option)
}

fn git_segment_is_destructive(segment: &[String]) -> bool {
    let clean = segment.iter().position(|token| token == "clean");
    if let Some(index) = clean {
        return segment[index + 1..]
            .iter()
            .any(|token| token.starts_with('-') && token.contains('f'));
    }
    if segment.iter().any(|token| token == "reset") && segment.iter().any(|token| token == "--hard")
    {
        return true;
    }
    if segment.iter().any(|token| token == "checkout") && segment.iter().any(|token| token == "--")
    {
        return true;
    }
    segment.iter().any(|token| token == "restore")
}

fn diskutil_segment_is_destructive(segment: &[String]) -> bool {
    segment.iter().any(|token| {
        matches!(
            token.as_str(),
            "erasevolume" | "erasedisk" | "partitiondisk" | "apfs" | "deletevolume" | "secureerase"
        )
    })
}
