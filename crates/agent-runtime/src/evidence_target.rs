use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

const MAX_EVIDENCE_TARGET_ANCHORS: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum EvidenceTargetAnchor {
    Workspace(String),
    ExternalUrl(String),
    ExternalSubject(String),
}

/// Extracts only targets that can be matched without interpreting the task.
/// The caller supplies the already-selected effective objective.
pub fn evidence_target_anchors(objective: &str) -> BTreeSet<EvidenceTargetAnchor> {
    let instruction = instruction_prefix(objective);
    let mut anchors = BTreeSet::new();

    for token in tokens(instruction) {
        if let Some(url) = canonical_url(token) {
            anchors.insert(EvidenceTargetAnchor::ExternalUrl(url));
        } else if let Some(path) = workspace_target(token) {
            anchors.insert(EvidenceTargetAnchor::Workspace(path));
        }
        if anchors.len() == MAX_EVIDENCE_TARGET_ANCHORS {
            return anchors;
        }
    }

    let has_url = anchors
        .iter()
        .any(|anchor| matches!(anchor, EvidenceTargetAnchor::ExternalUrl(_)));
    if !has_url && external_subjects_are_applicable(instruction) {
        for subject in external_subjects(instruction) {
            anchors.insert(EvidenceTargetAnchor::ExternalSubject(subject));
            if anchors.len() == MAX_EVIDENCE_TARGET_ANCHORS {
                break;
            }
        }
    }
    anchors
}

/// Empty anchors intentionally match so uncertain extraction preserves the
/// previous evidence behavior instead of rejecting every input.
pub fn evidence_input_matches_anchors(
    input: &str,
    anchors: &BTreeSet<EvidenceTargetAnchor>,
) -> bool {
    if anchors.is_empty() {
        return true;
    }

    let input_paths = tokens(input)
        .filter_map(workspace_target)
        .collect::<BTreeSet<_>>();
    let input_urls = tokens(input)
        .filter_map(canonical_url)
        .collect::<BTreeSet<_>>();
    let input_subjects = subject_words(input)
        .map(str::to_ascii_lowercase)
        .collect::<BTreeSet<_>>();
    let mut required_subjects = Vec::new();

    for anchor in anchors {
        match anchor {
            EvidenceTargetAnchor::Workspace(target) => {
                if input_paths
                    .iter()
                    .any(|candidate| workspace_targets_match(target, candidate))
                {
                    return true;
                }
            }
            EvidenceTargetAnchor::ExternalUrl(target) => {
                if input_urls
                    .iter()
                    .any(|candidate| urls_match(target, candidate))
                {
                    return true;
                }
            }
            EvidenceTargetAnchor::ExternalSubject(subject) => {
                required_subjects.push(subject.as_str());
            }
        }
    }

    let matches = required_subjects
        .iter()
        .filter(|subject| input_subjects.contains(**subject))
        .count();
    let threshold = required_subjects.len().min(2);
    threshold > 0 && matches >= threshold
}

pub fn evidence_target_witness(
    input: &str,
    anchors: &BTreeSet<EvidenceTargetAnchor>,
    tool_name: &str,
    input_fingerprint: &str,
    contract_epoch: u64,
) -> Option<String> {
    (!anchors.is_empty() && evidence_input_matches_anchors(input, anchors)).then(|| {
        evidence_target_witness_digest(anchors, tool_name, input_fingerprint, contract_epoch)
    })
}

pub(crate) fn evidence_target_witness_matches(
    receipt: &str,
    anchors: &BTreeSet<EvidenceTargetAnchor>,
    tool_name: &str,
    contract_epoch: u64,
) -> bool {
    if anchors.is_empty() {
        return true;
    }
    let Ok(receipt) = serde_json::from_str::<serde_json::Value>(receipt) else {
        return false;
    };
    let Some(input_fingerprint) = receipt
        .get("permission_input_fingerprint")
        .and_then(serde_json::Value::as_str)
    else {
        return false;
    };
    let Some(witness) = receipt
        .get("evidence_target_witness")
        .and_then(serde_json::Value::as_str)
    else {
        return false;
    };
    witness == evidence_target_witness_digest(anchors, tool_name, input_fingerprint, contract_epoch)
}

fn evidence_target_witness_digest(
    anchors: &BTreeSet<EvidenceTargetAnchor>,
    tool_name: &str,
    input_fingerprint: &str,
    contract_epoch: u64,
) -> String {
    let mut digest = Sha256::new();
    digest.update(b"cindx.evidence-target-witness.v1\n");
    digest.update(contract_epoch.to_string().as_bytes());
    digest.update(b"\n");
    digest.update(tool_name.as_bytes());
    digest.update(b"\n");
    digest.update(input_fingerprint.as_bytes());
    for anchor in anchors {
        digest.update(b"\n");
        match anchor {
            EvidenceTargetAnchor::Workspace(_) => digest.update(b"workspace:"),
            EvidenceTargetAnchor::ExternalUrl(_) => digest.update(b"url:"),
            EvidenceTargetAnchor::ExternalSubject(_) => digest.update(b"subject:"),
        }
        let value = match anchor {
            EvidenceTargetAnchor::Workspace(value)
            | EvidenceTargetAnchor::ExternalUrl(value)
            | EvidenceTargetAnchor::ExternalSubject(value) => value,
        };
        digest.update(value.as_bytes());
    }
    format!("{:x}", digest.finalize())
}

fn instruction_prefix(objective: &str) -> &str {
    // ASCII markers preserve byte offsets into the original UTF-8 objective.
    let lowercase = objective.to_ascii_lowercase();
    [
        "```",
        "material below",
        "content below",
        "text below",
        "pasted below",
        "以下材料",
        "材料如下",
        "以下内容",
        "内容如下",
        "粘贴如下",
    ]
    .iter()
    .filter_map(|marker| lowercase.find(marker))
    .min()
    .map(|boundary| &objective[..boundary])
    .unwrap_or(objective)
}

fn external_subjects_are_applicable(instruction: &str) -> bool {
    let lowercase = instruction.to_lowercase();
    if [
        "do not search",
        "don't search",
        "without web",
        "offline only",
        "self-contained",
        "self contained",
        "不要搜索",
        "不要联网",
        "无需联网",
        "仅使用给定",
    ]
    .iter()
    .any(|marker| lowercase.contains(marker))
    {
        return false;
    }
    [
        "search",
        "research",
        "look up",
        "lookup",
        "query",
        "browse",
        "web",
        "internet",
        "online",
        "latest",
        "documentation",
        "docs",
        "搜索",
        "查询",
        "联网",
        "网络",
        "最新",
    ]
    .iter()
    .any(|marker| lowercase.contains(marker))
}

fn external_subjects(instruction: &str) -> BTreeSet<String> {
    let mut subjects = BTreeSet::new();
    for token in tokens(instruction) {
        if canonical_url(token).is_some() || workspace_target(token).is_some() {
            continue;
        }
        for word in subject_words(token) {
            let word = word.to_ascii_lowercase();
            if significant_subject(&word) {
                subjects.insert(word);
                if subjects.len() == MAX_EVIDENCE_TARGET_ANCHORS {
                    return subjects;
                }
            }
        }
    }
    subjects
}

fn significant_subject(word: &str) -> bool {
    word.len() >= 4
        && word.len() <= 64
        && !word.bytes().all(|byte| byte.is_ascii_digit())
        && !matches!(
            word,
            "about"
                | "audit"
                | "browse"
                | "check"
                | "compare"
                | "content"
                | "current"
                | "documentation"
                | "behavior"
                | "explain"
                | "find"
                | "from"
                | "internet"
                | "latest"
                | "lookup"
                | "material"
                | "model"
                | "official"
                | "online"
                | "package"
                | "please"
                | "query"
                | "release"
                | "report"
                | "result"
                | "research"
                | "review"
                | "search"
                | "source"
                | "sources"
                | "summarize"
                | "that"
                | "this"
                | "using"
                | "verify"
                | "version"
                | "website"
                | "with"
        )
}

fn tokens(value: &str) -> impl Iterator<Item = &str> {
    value
        .split(|character: char| {
            character.is_whitespace()
                || matches!(
                    character,
                    '"' | '\'' | '`' | '<' | '>' | '(' | ')' | '[' | ']' | '{' | '}' | ',' | ';'
                )
        })
        .map(|token| {
            token.trim_matches(|character: char| {
                matches!(
                    character,
                    ':' | '!' | '?' | '*' | '|' | '=' | '\u{3002}' | '\u{ff0c}'
                )
            })
        })
        .filter(|token| !token.is_empty())
}

fn workspace_target(token: &str) -> Option<String> {
    let token = strip_line_reference(token).trim_end_matches('.');
    if token.is_empty() || token.starts_with("http://") || token.starts_with("https://") {
        return None;
    }
    let normalized = token.replace('\\', "/");
    let normalized = normalized.trim_end_matches('/');
    let filename = normalized.rsplit('/').next()?;
    let lowercase_filename = filename.to_ascii_lowercase();
    let explicit_filename = matches!(
        lowercase_filename.as_str(),
        "readme" | "license" | "makefile" | "dockerfile"
    ) || lowercase_filename.starts_with('.')
        || lowercase_filename
            .rsplit_once('.')
            .is_some_and(|(stem, extension)| {
                !stem.is_empty()
                    && matches!(
                        extension,
                        "css"
                            | "go"
                            | "html"
                            | "js"
                            | "json"
                            | "jsx"
                            | "lock"
                            | "md"
                            | "py"
                            | "rs"
                            | "sh"
                            | "sql"
                            | "swift"
                            | "toml"
                            | "ts"
                            | "tsx"
                            | "txt"
                            | "yaml"
                            | "yml"
                    )
            });
    let explicit_path = normalized.starts_with('/')
        || normalized.starts_with("./")
        || normalized.starts_with("../")
        || normalized.starts_with("~/")
        || ["apps/", "crates/", "docs/", "src/", "tests/", ".github/"]
            .iter()
            .any(|prefix| normalized.starts_with(prefix));
    (explicit_filename || explicit_path).then(|| normalized.to_ascii_lowercase())
}

fn strip_line_reference(value: &str) -> &str {
    if let Some((path, line)) = value.rsplit_once(':') {
        if !path.is_empty() && line.bytes().all(|byte| byte.is_ascii_digit()) {
            return path;
        }
    }
    let lowercase = value.to_ascii_lowercase();
    if let Some(index) = lowercase.rfind("#l") {
        if value[index + 2..].bytes().all(|byte| byte.is_ascii_digit()) {
            return &value[..index];
        }
    }
    value
}

fn workspace_targets_match(target: &str, candidate: &str) -> bool {
    if target.contains('/') {
        let relative = target.trim_start_matches("./");
        candidate == target || candidate.ends_with(&format!("/{relative}"))
    } else {
        candidate.rsplit('/').next() == Some(target)
    }
}

fn canonical_url(token: &str) -> Option<String> {
    let token = token.trim_end_matches(['.', '!', '?', ':', '/']);
    (token.starts_with("http://") || token.starts_with("https://"))
        .then(|| token.to_ascii_lowercase())
        .filter(|url| url.len() > "https://".len())
}

fn urls_match(target: &str, candidate: &str) -> bool {
    candidate == target
        || candidate
            .strip_prefix(target)
            .is_some_and(|suffix| suffix.starts_with(['/', '?', '#']))
}

fn subject_words(value: &str) -> impl Iterator<Item = &str> {
    value
        .split(|character: char| {
            !character.is_ascii_alphanumeric() && !matches!(character, '-' | '_' | '+' | '#')
        })
        .filter(|word| !word.is_empty())
}

#[cfg(test)]
#[path = "evidence_target_tests.rs"]
mod tests;
