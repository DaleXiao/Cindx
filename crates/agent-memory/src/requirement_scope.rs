use crate::memory_text::normalize_memory_text;

const MAX_DURABLE_REQUIREMENTS_PER_EVENT: usize = 64;
const MAX_DURABLE_REQUIREMENT_CHARS: usize = 1_200;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct UserRequirementSpan {
    pub start_byte: usize,
    pub end_byte: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RequirementStatementClass {
    ProjectDurable,
    TaskLocal,
    NotRequirement,
}

pub(crate) fn durable_user_requirement_spans(content: &str) -> Vec<UserRequirementSpan> {
    let mut inherited_scope = None;
    let mut durable = Vec::new();
    let _ = visit_statement_byte_ranges(content, |span| {
        let Some(statement) = content.get(span.start_byte..span.end_byte) else {
            return true;
        };
        if statement.chars().count() > MAX_DURABLE_REQUIREMENT_CHARS {
            return true;
        }
        let explicit_scope = explicit_requirement_scope(statement);
        let scope_only = explicit_scope == Some(RequirementStatementClass::ProjectDurable)
            && is_scope_only_statement(statement);
        if explicit_scope == Some(RequirementStatementClass::TaskLocal) {
            inherited_scope = Some(RequirementStatementClass::TaskLocal);
        } else if scope_only {
            inherited_scope = Some(RequirementStatementClass::ProjectDurable);
        }
        let class = classify_requirement_statement(statement);
        let is_durable = match (explicit_scope, inherited_scope, class) {
            (Some(RequirementStatementClass::TaskLocal), _, _) => false,
            (
                Some(RequirementStatementClass::ProjectDurable),
                _,
                RequirementStatementClass::ProjectDurable,
            ) => !scope_only,
            (None, Some(RequirementStatementClass::TaskLocal), _) => false,
            (
                None,
                Some(RequirementStatementClass::ProjectDurable),
                RequirementStatementClass::ProjectDurable,
            ) => true,
            (
                None,
                Some(RequirementStatementClass::ProjectDurable),
                RequirementStatementClass::TaskLocal,
            ) => {
                is_list_item(statement)
                    && (!contains_workflow_action(&statement.to_lowercase())
                        || is_recurrent_workflow_requirement(&statement.to_lowercase()))
            }
            (None, None, RequirementStatementClass::ProjectDurable) => true,
            _ => false,
        };
        if is_durable {
            durable.push(span);
        }
        durable.len() < MAX_DURABLE_REQUIREMENTS_PER_EVENT
    });
    durable
}

pub(crate) fn exact_durable_user_requirement_span(
    source: &str,
    candidate: &str,
) -> Option<UserRequirementSpan> {
    let candidate = candidate.trim();
    let mut matches = durable_user_requirement_spans(source)
        .into_iter()
        .filter(|span| source.get(span.start_byte..span.end_byte) == Some(candidate));
    let matched = matches.next()?;
    matches.next().is_none().then_some(matched)
}

fn visit_statement_byte_ranges(
    content: &str,
    mut visit: impl FnMut(UserRequirementSpan) -> bool,
) -> usize {
    let mut start = 0usize;
    for (index, character) in content.char_indices() {
        let end = index + character.len_utf8();
        let ascii_sentence_end = matches!(character, '.' | '!' | '?')
            && content
                .get(end..)
                .and_then(|suffix| suffix.chars().next())
                .is_none_or(char::is_whitespace);
        if ascii_sentence_end || matches!(character, '\n' | '\r' | '。' | '！' | '？') {
            if let Some(range) = trim_byte_range(content, start, end) {
                if !visit(range) {
                    return end;
                }
            }
            start = end;
        }
    }
    if let Some(range) = trim_byte_range(content, start, content.len()) {
        let _ = visit(range);
    }
    content.len()
}

fn trim_byte_range(content: &str, mut start: usize, mut end: usize) -> Option<UserRequirementSpan> {
    while start < end {
        let character = content.get(start..end)?.chars().next()?;
        if !character.is_whitespace() {
            break;
        }
        start += character.len_utf8();
    }
    while start < end {
        let character = content.get(start..end)?.chars().next_back()?;
        if !character.is_whitespace() {
            break;
        }
        end -= character.len_utf8();
    }
    (start < end).then_some(UserRequirementSpan {
        start_byte: start,
        end_byte: end,
    })
}

fn classify_requirement_statement(content: &str) -> RequirementStatementClass {
    let normalized = normalize_memory_text(content);
    if normalized.chars().count() < 6
        || matches!(
            normalized.as_str(),
            "hello" | "hi" | "hey" | "你好" | "您好" | "在吗" | "谢谢" | "thanks"
        )
        || contains_instruction_override(content)
        || contains_sensitive_value(content)
        || contains_reported_directive(content)
    {
        return RequirementStatementClass::NotRequirement;
    }

    let lower = content.to_lowercase();
    let trimmed = lower.trim();
    let directive = directive_text(trimmed);
    let explicit_scope = explicit_requirement_scope(content);
    if (content.trim_end().ends_with(['?', '？']) && !has_anchored_durable_directive(trimmed))
        || [
            "translate ",
            "quote ",
            "explain the phrase ",
            "翻译",
            "引用",
            "解释这句话",
            "分析这句话",
            "分析以下文本",
            "以下是引用",
            "示例：",
            "示例:",
        ]
        .iter()
        .any(|prefix| directive.starts_with(prefix))
        || ["do not remember", "don't remember", "不要记住", "别记住"]
            .iter()
            .any(|marker| trimmed.contains(marker))
    {
        return RequirementStatementClass::NotRequirement;
    }

    if let Some(scope) = explicit_scope {
        return scope;
    }

    if is_stable_declarative_requirement(trimmed) {
        RequirementStatementClass::ProjectDurable
    } else if contains_workflow_action(trimmed)
        || ["keep ", "do not ", "don't ", "must ", "should ", "please "]
            .iter()
            .any(|prefix| directive.starts_with(prefix))
        || ["不要", "不能", "必须", "需要", "保持", "务必"]
            .iter()
            .any(|prefix| directive.starts_with(prefix))
    {
        RequirementStatementClass::TaskLocal
    } else {
        RequirementStatementClass::NotRequirement
    }
}

fn is_list_item(content: &str) -> bool {
    let content = content.trim_start();
    content.starts_with("- ")
        || content.starts_with("* ")
        || content.starts_with("• ")
        || content
            .char_indices()
            .take_while(|(_, character)| character.is_ascii_digit())
            .last()
            .is_some_and(|(index, character)| {
                let marker_end = index + character.len_utf8();
                content[marker_end..].starts_with(". ")
                    || content[marker_end..].starts_with(") ")
                    || content[marker_end..].starts_with('、')
            })
}

fn contains_reported_directive(content: &str) -> bool {
    let lower = content.to_lowercase();
    let known_report_marker = [
        "documentation says",
        "the docs say",
        "example says",
        "the text says",
        "quoted text",
        "文档示例",
        "文档写着",
        "示例写着",
        "文本写着",
        "原文是",
        "引用内容",
    ]
    .iter()
    .any(|marker| lower.contains(marker));
    let remember_report = (lower.starts_with("remember that ")
        || lower.starts_with("please remember that ")
        || lower.starts_with("记住，")
        || lower.starts_with("请记住，"))
        && [" says", "写着", "说：", "说:"]
            .iter()
            .any(|marker| lower.contains(marker));
    (known_report_marker || remember_report)
        && ["remember", "from now on", "always", "记住", "以后", "今后"]
            .iter()
            .any(|marker| lower.contains(marker))
}

fn explicit_requirement_scope(content: &str) -> Option<RequirementStatementClass> {
    let lower = content.to_lowercase();
    let trimmed = lower.trim();
    if [
        "this task",
        "this turn",
        "this run",
        "this change",
        "this request",
        "this step",
        "this review",
        "for this session",
        "in this session",
        "during this session",
        "only in this session",
        "this time",
        "for now",
        "current run",
        "current change",
        "current request",
        "current step",
        "current review",
        "for the current session",
        "in the current session",
        "during the current session",
        "temporarily",
        "only this time",
        "just this once",
        "one-off",
        "today only",
        "本次",
        "这次",
        "此次",
        "本轮",
        "这轮",
        "当前运行",
        "当前改动",
        "当前请求",
        "当前步骤",
        "当前评审",
        "本会话内",
        "仅当前会话",
        "当前会话内",
        "这个会话内",
        "这个改动",
        "这项改动",
        "这个请求",
        "这个步骤",
        "这一步",
        "这次评审",
        "本次评审",
        "暂时",
        "一次性",
        "仅这次",
        "只这次",
        "仅本轮",
        "先不要",
        "先别",
        "先不",
        "先只",
    ]
    .iter()
    .any(|marker| trimmed.contains(marker))
        || ["current task:", "current task：", "当前任务:", "当前任务："]
            .iter()
            .any(|marker| trimmed.starts_with(marker))
        || has_anchored_current_task_scope(trimmed)
    {
        return Some(RequirementStatementClass::TaskLocal);
    }

    let anchored_durable = has_anchored_durable_directive(trimmed);
    let strong_durable = [
        "standing requirement",
        "long-term requirement",
        "across all sessions",
        "across sessions",
        "every session",
        "each session",
        "i prefer",
        "we prefer",
        "my preference",
        "call me",
        "my name is",
        "address me as",
        "长期要求",
        "长期约束",
        "所有会话",
        "跨会话",
        "我偏好",
        "我的偏好",
        "叫我",
        "称呼我",
        "我的名字是",
    ]
    .iter()
    .any(|marker| trimmed.contains(marker));
    (anchored_durable || strong_durable).then_some(RequirementStatementClass::ProjectDurable)
}

fn has_anchored_current_task_scope(content: &str) -> bool {
    let directive = directive_text(content);
    [
        "for the current task",
        "in the current task",
        "during the current task",
        "for current task",
        "in current task",
        "during current task",
        "only for the current task",
        "only in the current task",
        "only during the current task",
        "only for current task",
        "only in current task",
        "only during current task",
        "current task only",
    ]
    .iter()
    .any(|marker| {
        directive.strip_prefix(marker).is_some_and(|suffix| {
            suffix.is_empty()
                || suffix.starts_with(|character: char| {
                    character.is_whitespace()
                        || matches!(character, ',' | ':' | ';' | '-' | '\u{2014}')
                })
        })
    }) || [
        "当前任务中",
        "当前任务内",
        "当前任务期间",
        "在当前任务中",
        "在当前任务内",
        "仅当前任务",
        "只在当前任务中",
        "只在当前任务内",
        "仅在当前任务中",
        "仅在当前任务内",
        "对于当前任务",
    ]
    .iter()
    .any(|marker| directive.starts_with(marker))
}

fn has_anchored_durable_directive(content: &str) -> bool {
    let directive = directive_text(content);
    [
        "remember ",
        "remember:",
        "remember,",
        "can you remember ",
        "could you remember ",
        "would you remember ",
        "will you remember ",
        "from now on",
        "going forward",
        "always ",
        "never ",
        "记住",
        "你能记住",
        "你可以记住",
        "能记住",
        "可以记住",
        "以后",
        "今后",
        "从现在开始",
        "始终",
    ]
    .iter()
    .any(|marker| directive.starts_with(marker))
}

fn directive_text(content: &str) -> &str {
    let mut directive = content.trim_start();
    if let Some(stripped) = directive
        .strip_prefix("- ")
        .or_else(|| directive.strip_prefix("* "))
        .or_else(|| directive.strip_prefix("• "))
    {
        directive = stripped.trim_start();
    } else {
        let marker_end = directive
            .char_indices()
            .take_while(|(_, character)| character.is_ascii_digit())
            .last()
            .map(|(index, character)| index + character.len_utf8());
        if let Some(marker_end) = marker_end {
            let suffix = &directive[marker_end..];
            if let Some(stripped) = suffix
                .strip_prefix(". ")
                .or_else(|| suffix.strip_prefix(") "))
                .or_else(|| suffix.strip_prefix("、"))
            {
                directive = stripped.trim_start();
            }
        }
    }
    directive = directive
        .strip_prefix("please ")
        .or_else(|| directive.strip_prefix("please, "))
        .or_else(|| directive.strip_prefix("please: "))
        .or_else(|| directive.strip_prefix('请'))
        .unwrap_or(directive);
    directive.trim_start()
}

fn contains_workflow_action(content: &str) -> bool {
    [
        "build",
        "test",
        "commit",
        "push",
        "modify",
        "edit",
        "review",
        "audit",
        "fix",
        "implement",
    ]
    .iter()
    .any(|marker| contains_ascii_word(content, marker))
        || [
            "构建",
            "测试",
            "提交",
            "推送",
            "修改",
            "改代码",
            "评审",
            "审核",
            "修复",
            "实现",
        ]
        .iter()
        .any(|marker| content.contains(marker))
}

fn is_recurrent_workflow_requirement(content: &str) -> bool {
    [
        "every goal",
        "each goal",
        "every task",
        "each task",
        "every time",
        "after every",
        "after each",
        "for every",
        "for each",
        "每个 goal",
        "每个goal",
        "每一个 goal",
        "每一个goal",
        "每个任务",
        "每次",
        "每一轮",
    ]
    .iter()
    .any(|marker| content.contains(marker))
}

fn is_scope_only_statement(content: &str) -> bool {
    let lower = content.to_lowercase();
    let directive = directive_text(&lower);
    let directive = directive.trim_end_matches([
        ' ', '\t', '\r', '\n', ':', '：', '.', '。', '!', '！', ';', '；',
    ]);
    matches!(
        directive,
        "from now on"
            | "going forward"
            | "long-term requirement"
            | "long-term requirements"
            | "across all sessions"
            | "across sessions"
            | "以后"
            | "今后"
            | "从现在开始"
            | "以后所有会话"
            | "以后所有会话要求"
            | "今后所有会话"
            | "今后所有会话要求"
            | "所有会话"
            | "跨会话"
            | "长期要求"
            | "长期约束"
            | "长期要求如下"
            | "长期约束如下"
    )
}

fn is_stable_declarative_requirement(content: &str) -> bool {
    for marker in ["must", "should"] {
        if let Some(index) = ascii_word_position(content, marker) {
            let subject = content[..index].trim();
            if !subject.is_empty()
                && !matches!(subject, "i" | "we" | "you" | "please")
                && !subject.ends_with(" you")
            {
                return true;
            }
        }
    }
    if ["requirement", "constraint"]
        .iter()
        .any(|marker| contains_ascii_word(content, marker))
    {
        return true;
    }
    [
        "必须",
        "不要",
        "不能",
        "不允许",
        "务必",
        "保持",
        "要求",
        "需要",
    ]
    .iter()
    .any(|marker| {
        content.find(marker).is_some_and(|index| {
            let subject = content[..index].trim();
            subject.chars().count() >= 2 && !matches!(subject, "请你" | "我们" | "你们" | "麻烦")
        })
    })
}

fn contains_ascii_word(content: &str, target: &str) -> bool {
    ascii_word_position(content, target).is_some()
}

fn ascii_word_position(content: &str, target: &str) -> Option<usize> {
    content.match_indices(target).find_map(|(index, _)| {
        let before = content[..index].chars().next_back();
        let after = content[index + target.len()..].chars().next();
        (before.is_none_or(|character| !character.is_ascii_alphanumeric())
            && after.is_none_or(|character| !character.is_ascii_alphanumeric()))
        .then_some(index)
    })
}

fn contains_sensitive_value(content: &str) -> bool {
    let lower = content.to_lowercase();
    if lower.contains("-----begin private key-----")
        || lower.contains("-----begin rsa private key-----")
    {
        return true;
    }
    if [
        ("api key", true),
        ("api_key", true),
        ("apikey", true),
        ("bearer", true),
        ("pat", true),
        ("password", false),
        ("client secret", false),
        ("client_secret", false),
        ("access token", false),
        ("access_token", false),
        ("refresh token", false),
        ("refresh_token", false),
        ("api 密钥", false),
        ("api密钥", false),
        ("访问密钥", false),
        ("密码", false),
        ("访问令牌", false),
        ("刷新令牌", false),
        ("令牌", false),
    ]
    .iter()
    .any(|(label, allow_unseparated)| {
        credential_value_after_label(&lower, label, *allow_unseparated)
    }) {
        return true;
    }
    if ["api key:", "api key=", "api_key:", "api_key="]
        .iter()
        .any(|marker| {
            lower.find(marker).is_some_and(|index| {
                lower[index + marker.len()..]
                    .split_whitespace()
                    .next()
                    .is_some_and(plausible_secret_value)
            })
        })
    {
        return true;
    }
    let tokens = lower
        .split_whitespace()
        .map(|token| {
            token.trim_matches(|character: char| {
                !character.is_ascii_alphanumeric() && !matches!(character, '-' | '_' | '=' | ':')
            })
        })
        .collect::<Vec<_>>();
    tokens.iter().enumerate().any(|(index, token)| {
        (token.starts_with("sk-") && token.len() >= 16)
            || (token.starts_with("ghp_") && token.len() >= 20)
            || (token.starts_with("github_pat_") && token.len() >= 20)
            || (*token == "bearer"
                && tokens
                    .get(index + 1)
                    .is_some_and(|value| plausible_secret_value(value)))
            || (matches!(*token, "pat" | "pat:" | "pat=")
                && tokens
                    .get(index + 1)
                    .is_some_and(|value| plausible_secret_value(value)))
            || (*token == "api" && api_key_secret_at(&tokens, index))
            || token
                .strip_prefix("bearer:")
                .or_else(|| token.strip_prefix("bearer="))
                .or_else(|| token.strip_prefix("pat:"))
                .or_else(|| token.strip_prefix("pat="))
                .or_else(|| token.strip_prefix("api_key="))
                .or_else(|| token.strip_prefix("api_key:"))
                .or_else(|| token.strip_prefix("apikey="))
                .or_else(|| token.strip_prefix("apikey:"))
                .or_else(|| token.strip_prefix("access_token="))
                .is_some_and(plausible_secret_value)
    })
}

fn credential_value_after_label(content: &str, label: &str, allow_unseparated: bool) -> bool {
    content.match_indices(label).any(|(index, _)| {
        let before = content[..index].chars().next_back();
        let after_index = index + label.len();
        let after = content[after_index..].chars().next();
        if before.is_some_and(|character| character.is_ascii_alphanumeric() || character == '_')
            || after.is_some_and(|character| character.is_ascii_alphanumeric() || character == '_')
        {
            return false;
        }
        let mut rest = content[after_index..].trim_start();
        let mut separated = false;
        if let Some(stripped) = rest
            .strip_prefix(':')
            .or_else(|| rest.strip_prefix('：'))
            .or_else(|| rest.strip_prefix('='))
        {
            rest = stripped.trim_start();
            separated = true;
        } else if let Some(stripped) = rest
            .strip_prefix("is ")
            .or_else(|| rest.strip_prefix("is:"))
            .or_else(|| rest.strip_prefix("is="))
            .or_else(|| rest.strip_prefix('是'))
            .or_else(|| rest.strip_prefix('为'))
        {
            rest = stripped.trim_start_matches([' ', '\t', ':', '：', '=']);
            separated = true;
        }
        if !separated && !allow_unseparated {
            return false;
        }
        rest.split_whitespace()
            .next()
            .map(|value| {
                value.trim_matches(|character: char| {
                    matches!(character, '\'' | '"' | '`' | ',' | '，' | ';' | '；')
                })
            })
            .is_some_and(plausible_secret_value)
    })
}

fn plausible_secret_value(value: &str) -> bool {
    value.trim_matches(['\'', '"']).len() >= 12
}

fn api_key_secret_at(tokens: &[&str], api_index: usize) -> bool {
    let Some(label) = tokens.get(api_index + 1).copied() else {
        return false;
    };
    if let Some(value) = label
        .strip_prefix("key:")
        .or_else(|| label.strip_prefix("key="))
        .filter(|value| !value.is_empty())
    {
        return plausible_secret_value(value);
    }
    matches!(label, "key" | "key:" | "key=")
        && tokens
            .get(api_index + 2)
            .is_some_and(|value| plausible_secret_value(value))
}

pub(crate) fn contains_instruction_override(content: &str) -> bool {
    let lower = content.to_lowercase();
    [
        "ignore previous instruction",
        "ignore all previous",
        "ignore the system message",
        "reveal the system prompt",
        "developer message says",
        "忽略之前的指令",
        "忽略所有之前",
        "忽略系统消息",
        "泄露系统提示",
        "显示系统提示词",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn durable_contents(content: &str) -> Vec<&str> {
        durable_user_requirement_spans(content)
            .into_iter()
            .map(|span| {
                content
                    .get(span.start_byte..span.end_byte)
                    .expect("requirement span should remain on UTF-8 boundaries")
            })
            .collect()
    }

    #[test]
    fn task_local_scope_applies_to_following_english_list_items() {
        let content = concat!(
            "For this task:\n",
            "- The response must contain no code.\n",
            "- Release builds must run without network access."
        );

        assert!(durable_contents(content).is_empty());
    }

    #[test]
    fn task_local_scope_applies_to_following_chinese_list_items() {
        let content = concat!(
            "本次要求如下：\n",
            "- 发布流程必须构建测试包。\n",
            "- 每个 goal 必须先运行测试。"
        );

        assert!(durable_contents(content).is_empty());
    }

    #[test]
    fn change_request_step_and_review_scopes_remain_task_local() {
        for content in [
            "For this change:\n- Release builds must run without network access.",
            "This request:\n- Release builds must run without network access.",
            "For this step:\n- Release builds must run without network access.",
            "For this review:\n- Release builds must run without network access.",
            "这个改动里：\n- 发布流程必须构建离线包。",
            "这个请求里：\n- 发布流程必须构建离线包。",
            "这一步：\n- 发布流程必须构建离线包。",
            "本次评审：\n- 发布流程必须构建离线包。",
        ] {
            assert!(
                durable_contents(content).is_empty(),
                "local scope leaked into durable memory: {content:?}"
            );
        }
    }

    #[test]
    fn chinese_after_conjunction_is_not_a_durable_scope_marker() {
        for content in ["测试通过以后提交代码", "构建完成今后推送代码"] {
            assert!(
                durable_contents(content).is_empty(),
                "temporal workflow was persisted: {content:?}"
            );
        }
    }

    #[test]
    fn declarative_workflow_requirements_remain_durable() {
        for content in [
            "Release builds must run without network access",
            "发布流程必须构建离线包",
        ] {
            assert_eq!(durable_contents(content), vec![content]);
        }
    }

    #[test]
    fn explicit_polite_and_product_requirements_remain_durable() {
        for content in [
            "Please remember, always review every session before deletion, okay?",
            "Could you remember to always review every session before deletion?",
            "请记住以后所有会话都先审核，可以吗？",
            "你能记住以后所有会话都先审核吗？",
            "Always preserve the current task when switching sessions.",
            "始终在切换会话时保留当前任务。",
        ] {
            assert_eq!(durable_contents(content), vec![content]);
        }
    }

    #[test]
    fn current_task_scope_is_grammatical_not_object_based() {
        for content in [
            "For the current task, release builds must use a temporary signing profile.",
            "In current task, release builds must use a temporary signing profile.",
            "During the current task, release builds must use a temporary signing profile.",
            "Current task only: release builds must use a temporary signing profile.",
            "当前任务中，发布流程必须使用临时签名配置。",
            "当前任务内，发布流程必须使用临时签名配置。",
            "仅当前任务：发布流程必须使用临时签名配置。",
        ] {
            assert!(
                durable_contents(content).is_empty(),
                "current-task scope leaked into durable memory: {content:?}"
            );
        }

        for content in [
            "Always preserve the current task when switching sessions.",
            "始终在切换会话时保留当前任务。",
        ] {
            assert_eq!(durable_contents(content), vec![content]);
        }
    }

    #[test]
    fn markdown_bullets_and_remember_comma_are_explicit_durable_markers() {
        for content in [
            "- Always keep release builds offline",
            "Remember, keep release builds offline",
        ] {
            assert_eq!(durable_contents(content), vec![content]);
        }
    }

    #[test]
    fn explicit_durable_scope_resets_an_inherited_task_local_boundary() {
        let content = concat!(
            "本次要求如下：\n",
            "- 发布流程必须构建测试包。\n",
            "以后所有会话：\n",
            "- 发布流程必须构建离线包。\n",
            "- Release builds must run without network access."
        );

        assert_eq!(
            durable_contents(content),
            vec![
                "- 发布流程必须构建离线包。",
                "- Release builds must run without network access."
            ]
        );
    }

    #[test]
    fn ordinary_durable_statement_does_not_promote_following_task_local_work() {
        let content = concat!(
            "For this task:\n",
            "- Release builds must use a temporary signing profile.\n",
            "Remember, always preserve dark mode. ",
            "Please build now."
        );

        assert_eq!(
            durable_contents(content),
            vec!["Remember, always preserve dark mode."]
        );
    }

    #[test]
    fn durable_scope_header_only_inherits_declarative_requirements() {
        let content = concat!(
            "Long-term requirements:\n",
            "- Release builds must run without network access.\n",
            "- Please build now."
        );

        assert_eq!(
            durable_contents(content),
            vec!["- Release builds must run without network access."]
        );
    }

    #[test]
    fn durable_scope_header_accepts_non_workflow_imperative_list_items_only() {
        let content = concat!(
            "长期要求如下：\n",
            "- 不要自动打开侧栏。\n",
            "- Please build now."
        );

        assert_eq!(durable_contents(content), vec!["- 不要自动打开侧栏。"]);
    }

    #[test]
    fn durable_scope_header_accepts_recurrent_workflow_requirements() {
        for content in [
            "Long-term requirements:\n- Build and install after every goal.",
            "长期要求如下：\n- 每个 goal 完成后构建并安装。",
        ] {
            assert_eq!(durable_contents(content).len(), 1, "lost {content:?}");
        }
    }

    #[test]
    fn durable_lists_preserve_more_than_four_requirements() {
        let content = concat!(
            "Long-term requirements:\n",
            "1. The sidebar must remain stable.\n",
            "2. The composer must remain responsive.\n",
            "3. The session list must preserve unread state.\n",
            "4. The model picker must remain accessible.\n",
            "5. The knowledge graph must remain searchable.\n",
            "6. Release builds must remain reproducible."
        );

        assert_eq!(durable_contents(content).len(), 6);
    }

    #[test]
    fn durable_lists_preserve_more_than_twelve_requirements() {
        let mut content = String::from("Long-term requirements:\n");
        for index in 1..=20 {
            content.push_str(&format!(
                "{index}. Component {index} must preserve its public behavior.\n"
            ));
        }
        assert_eq!(durable_contents(&content).len(), 20);
    }

    #[test]
    fn statement_visit_stops_scanning_when_the_requirement_budget_is_full() {
        let mut content = String::new();
        for index in 0..10_000 {
            content.push_str(&format!("Line {index} must remain stable.\n"));
        }
        let mut visited = 0usize;
        let scanned_bytes = visit_statement_byte_ranges(&content, |_| {
            visited += 1;
            visited < MAX_DURABLE_REQUIREMENTS_PER_EVENT
        });
        assert_eq!(visited, MAX_DURABLE_REQUIREMENTS_PER_EVENT);
        assert!(scanned_bytes < content.len() / 10);
    }

    #[test]
    fn ascii_dots_inside_runtime_urls_versions_and_paths_do_not_split_quotes() {
        let content =
            "Always keep Node.js 22.20.0 compatible with api.openai.com/v1 and README.md.";

        assert_eq!(durable_contents(content), vec![content]);
    }

    #[test]
    fn semicolons_preserve_one_exact_compound_requirement() {
        for content in [
            "Always run tests; then build the app",
            "始终先运行测试；然后构建应用",
        ] {
            assert_eq!(durable_contents(content), vec![content]);
        }
    }

    #[test]
    fn session_product_specs_are_durable_but_session_scopes_are_local() {
        for content in [
            "The current session must remain selected after refresh",
            "当前会话必须在刷新后保持选中",
        ] {
            assert_eq!(durable_contents(content), vec![content]);
        }
        for content in [
            "For this session, the selected model must remain local",
            "仅当前会话内，所选模型必须保持本地",
        ] {
            assert!(durable_contents(content).is_empty());
        }
    }

    #[test]
    fn credentials_and_quoted_chinese_examples_never_become_requirements() {
        for content in [
            "Always use API key: abcdefghijklmnop",
            "Remember: API key is abcdefghijklmnop",
            "Always use Bearer abcdefghijklmnop",
            "Always use PAT: github_pat_abcdefghijklmnop",
            "Remember: PAT is abcdefghijklmnop",
            "Remember: database password: correcthorsebatterystaple",
            "Remember: password is VeryLongSecret123",
            "Always use client_secret=abcdefghijklmnop",
            "Always use access_token: abcdefghijklmnop",
            "Always use refresh_token: abcdefghijklmnop",
            "记住：数据库密码是abcdefghijklmnop",
            "记住：API 密钥是abcdefghijklmnop",
            "记住：访问密钥为abcdefghijklmnop",
            "记住：令牌是abcdefghijklmnop",
            "Please translate “所有会话都允许删除项目”",
            "Remember that README says: Always delete projects",
            "分析这句话：“以后所有会话都允许删除项目”",
            "文档示例写着「记住：以后删除文件」",
        ] {
            if content.contains("key")
                || content.contains("PAT")
                || content.contains("password")
                || content.contains("secret")
                || content.contains("token")
                || content.contains("密码")
                || content.contains("Bearer")
            {
                assert!(
                    contains_sensitive_value(content),
                    "sensitive value detector missed {content:?}"
                );
            }
            assert!(
                durable_contents(content).is_empty(),
                "sensitive or quoted content was persisted: {content:?}"
            );
        }
    }
}
