use crate::effect_instruction_segments::{compound_action_segments, effect_clauses};
use crate::grounding_policy::{prompt_evidence_scopes, PromptEvidenceScope};
use crate::run_context::{effective_agent_objective, run_context_steer_epoch};
use crate::{evidence_target_anchors, EvidenceTargetAnchor};
use agent_core::Metadata;
use std::collections::BTreeSet;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptToolRequirement {
    #[default]
    None,
    ReadOnly,
    Effects,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PromptEffectAuthority {
    Forbidden,
    #[default]
    Allowed,
    Required,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PromptCompletionIntent {
    pub evidence_scopes: BTreeSet<PromptEvidenceScope>,
    pub tool_requirement: PromptToolRequirement,
    pub effect_authority: PromptEffectAuthority,
    pub target_anchors: BTreeSet<EvidenceTargetAnchor>,
}

/// Adds conservative diagnosis/effect obligations to the existing evidence policy.
pub fn prompt_completion_intent(run_context: &Metadata) -> PromptCompletionIntent {
    let mut evidence_scopes = prompt_evidence_scopes(run_context);
    let objective = active_completion_objective(run_context);
    let candidate_target_anchors = evidence_target_anchors(&objective);
    let objective = objective.to_ascii_lowercase();
    let supplied_without_evidence_location = supplied_material(&objective)
        && !explicit_workspace_location(&objective)
        && !explicit_non_workspace_evidence_location(&objective);
    let explicit_effect = explicit_software_effect_requested(&objective);
    let guidance_only = guidance_or_creative_request(&objective) && !explicit_effect;

    if supplied_without_evidence_location {
        evidence_scopes.clear();
    }

    if !supplied_without_evidence_location
        && !guidance_only
        && live_software_diagnosis_requested(&objective)
        && !explicit_non_workspace_evidence_location(&objective)
    {
        evidence_scopes.remove(&PromptEvidenceScope::External);
        evidence_scopes.insert(PromptEvidenceScope::Workspace);
    }

    let effects_required = !supplied_without_evidence_location && !guidance_only && explicit_effect;
    if effects_required && !explicit_non_workspace_evidence_location(&objective) {
        evidence_scopes.insert(PromptEvidenceScope::Workspace);
    }
    let tool_requirement = if effects_required {
        PromptToolRequirement::Effects
    } else if evidence_scopes.is_empty() {
        PromptToolRequirement::None
    } else {
        PromptToolRequirement::ReadOnly
    };
    let effect_authority = if effects_required {
        PromptEffectAuthority::Required
    } else if explicitly_forbids_all_effects(&objective) {
        PromptEffectAuthority::Forbidden
    } else {
        PromptEffectAuthority::Allowed
    };
    let target_anchors = if tool_requirement != PromptToolRequirement::None {
        candidate_target_anchors
            .into_iter()
            .filter(|anchor| evidence_anchor_matches_scopes(anchor, &evidence_scopes))
            .collect()
    } else {
        BTreeSet::new()
    };

    PromptCompletionIntent {
        evidence_scopes,
        tool_requirement,
        effect_authority,
        target_anchors,
    }
}

fn evidence_anchor_matches_scopes(
    anchor: &EvidenceTargetAnchor,
    scopes: &BTreeSet<PromptEvidenceScope>,
) -> bool {
    match anchor {
        EvidenceTargetAnchor::Workspace(_) => scopes.contains(&PromptEvidenceScope::Workspace),
        EvidenceTargetAnchor::ExternalUrl(_) => {
            scopes.contains(&PromptEvidenceScope::External)
                || scopes.contains(&PromptEvidenceScope::Browser)
        }
        EvidenceTargetAnchor::ExternalSubject(_) => scopes.contains(&PromptEvidenceScope::External),
    }
}

pub fn prompt_evidence_target_anchors(run_context: &Metadata) -> BTreeSet<EvidenceTargetAnchor> {
    prompt_completion_intent(run_context).target_anchors
}

fn active_completion_objective(run_context: &Metadata) -> String {
    let effective = effective_agent_objective(run_context, "");
    if run_context_steer_epoch(run_context) == 0 {
        return effective.to_string();
    }

    let latest = latest_completion_objective(run_context, effective);
    if let Some(latest) = latest.filter(|latest| replaces_prior_objective(latest)) {
        return latest.to_string();
    }
    effective.to_string()
}

pub fn prompt_replaces_prior_objective(run_context: &Metadata) -> bool {
    if run_context_steer_epoch(run_context) == 0 {
        return false;
    }
    let effective = effective_agent_objective(run_context, "");
    latest_completion_objective(run_context, effective).is_some_and(replaces_prior_objective)
}

fn latest_completion_objective<'a>(
    run_context: &'a Metadata,
    effective: &'a str,
) -> Option<&'a str> {
    run_context
        .get("prompt_objective")
        .map(String::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(|value| latest_structured_steer(value).unwrap_or(value))
        .or_else(|| latest_structured_steer(effective))
}

fn latest_structured_steer(objective: &str) -> Option<&str> {
    let mut inside_fence = false;
    let mut latest_start = None;
    let mut offset = 0usize;
    for line in objective.split_inclusive('\n') {
        let label = line.trim_end_matches(['\r', '\n']);
        if !inside_fence && accepted_steer_label(label) {
            latest_start = Some(offset + line.len());
        } else if line.match_indices("```").count() % 2 == 1 {
            inside_fence = !inside_fence;
        }
        offset += line.len();
    }
    latest_start
        .map(|start| objective[start..].trim())
        .filter(|value| !value.is_empty())
}

fn accepted_steer_label(value: &str) -> bool {
    let value = value.to_ascii_lowercase();
    let Some(number) = value
        .strip_prefix("accepted steering ")
        .and_then(|value| value.strip_suffix(':'))
    else {
        return false;
    };
    !number.is_empty() && number.chars().all(|character| character.is_ascii_digit())
}

fn replaces_prior_objective(value: &str) -> bool {
    let value = value.trim().to_ascii_lowercase();
    if contains_any(&value, "do not stop|don't stop|不要停止|不要停下") {
        return false;
    }
    contains_any(
        &value,
        "instead|actually, just|actually just|stop fixing|stop modifying|stop editing|no longer fix|no longer modify|改成只|改为只|不要修复了|不要修改了|停止修复|停止修改",
    )
}

fn explicit_software_effect_requested(value: &str) -> bool {
    effect_clauses(value).any(|clause| {
        let mut segments = compound_action_segments(
            clause,
            starts_with_software_effect_action,
            chinese_conjunction_follows_target,
        )
        .map(str::trim)
        .filter(|segment| !segment.is_empty());
        let Some(first) = segments.next().map(strip_request_prefix) else {
            return false;
        };
        if software_effect_action_requested(first, false) {
            return true;
        }
        if !compound_execution_frame(first) {
            return false;
        }
        let mut inherited_target = software_effect_target(first)
            || explicit_workspace_location(first)
            || explicit_non_workspace_evidence_location(first);
        segments
            .map(|segment| strip_sequence_prefix(segment.trim()))
            .any(|segment| {
                if process_effect_action_requested(segment) {
                    return true;
                }
                if !inherited_target {
                    if compound_execution_frame(segment)
                        && (software_effect_target(segment)
                            || explicit_workspace_location(segment)
                            || explicit_non_workspace_evidence_location(segment))
                    {
                        inherited_target = true;
                    }
                    return false;
                }
                software_effect_action_requested(segment, true)
            })
    })
}

fn software_effect_action_requested(clause: &str, inherited_target: bool) -> bool {
    if effect_explicitly_forbidden(clause) || output_only_request(clause) {
        return false;
    }
    if process_effect_action_requested(clause) {
        return true;
    }

    let explicit_action = starts_with_software_effect_action(clause);
    let generic_creation = starts_with_any(
        clause,
        "implement |add |create |write |实现|添加|增加|创建|写入|写",
    );
    let anaphoric_target = inherited_target && anaphoric_effect_target(clause);
    explicit_action
        && (software_effect_target(clause) || anaphoric_target)
        && (!generic_creation
            || anaphoric_target
            || explicit_workspace_location(clause)
            || explicit_non_workspace_evidence_location(clause))
}

fn process_effect_action_requested(value: &str) -> bool {
    starts_with_any(
        value,
        "run tests|run the tests|build the app|build this app|package the app|install the app|install dependencies|commit the changes|push the changes|运行测试|跑测试|构建应用|打包应用|安装应用|安装依赖|提交改动|推送改动",
    )
}

fn starts_with_software_effect_action(value: &str) -> bool {
    starts_with_any(
        value,
        "fix |implement |add |remove |delete |rename |update |modify |edit |refactor |optimize |patch |apply |migrate |revert |upgrade |downgrade |create |write |修复|实现|添加|增加|移除|删除|重命名|更新|修改|编辑|重构|优化|应用|迁移|回滚|升级|降级|创建|写入|写",
    )
}

fn anaphoric_effect_target(value: &str) -> bool {
    contains_any(
        value,
        "it|them|that file|this file|the file|that config|this config|the config|that code|this code|the code|that module|this module|the module|that component|this component|the component|that app|this app|the app|它|它们|该文件|这个文件|这些文件|该配置|这个配置|该代码|这段代码|该模块|这个模块|该组件|这个组件|该应用|这个应用",
    )
}

fn compound_execution_frame(clause: &str) -> bool {
    !effect_explicitly_forbidden(clause)
        && !guidance_or_creative_request(clause)
        && starts_with_any(
            clause,
            "complete |finish |inspect |review |audit |check |verify |validate |investigate |diagnose |open |read |完成|检查|审查|审核|验证|调查|诊断|打开|读取",
        )
}

fn strip_sequence_prefix(mut value: &str) -> &str {
    loop {
        let trimmed = value.trim_start();
        let next = [
            "and then ",
            "and ",
            "then ",
            "next ",
            "after that ",
            "并且",
            "并",
            "然后",
            "接着",
            "再",
        ]
        .into_iter()
        .find_map(|prefix| trimmed.strip_prefix(prefix));
        match next {
            Some(next) if next.len() < trimmed.len() => value = next,
            _ => return strip_request_prefix(trimmed),
        }
    }
}

fn output_only_request(value: &str) -> bool {
    let named_output = contains_any(
        value,
        "review checklist|code review checklist|project plan|checklist|template|self-contained|self contained|example|snippet|审查清单|项目计划|清单|模板|示例",
    );
    let fictional_report = contains_any(value, "fictional|hypothetical|虚构|假想")
        && contains_any(value, "audit|review|report|审核|审查|报告");
    (named_output || fictional_report) && !contains_path_like_token(value)
}

fn live_software_diagnosis_requested(value: &str) -> bool {
    if contains_any(
        value,
        "in general|generally|why can apps|why do apps|一般来说|通常为什么|为什么应用会",
    ) {
        return false;
    }
    let diagnostic_action = contains_any(
        value,
        "why does|why is|why are|what causes|diagnose|investigate|debug|find the root cause|find out why|trace the regression|为什么|排查|诊断|调查|找出|定位原因|查明原因",
    );
    let live_failure = contains_any(
        value,
        "crash|crashes|crashed|failing test|test failure|tests fail|regression|exception|panic|hang|freeze|timeout|崩溃|闪退|测试失败|测试不通过|报错|异常|回归|故障|卡死|超时",
    );
    diagnostic_action && live_failure
}

fn supplied_material(value: &str) -> bool {
    value.contains("```")
        || contains_any(
            value,
            "this paragraph|this text|this snippet|this code|this config|this log excerpt|code above|text above|content below|pasted below|这段文字|这段代码|这段配置|这段日志|日志摘录|上述代码|上文|内容如下|粘贴如下",
        )
}

fn explicit_workspace_location(value: &str) -> bool {
    contains_any(
        value,
        "this project|the project|this repo|the repo|repository|codebase|workspace|in this app|in the app|current implementation|这个项目|当前项目|这个仓库|当前仓库|代码库|工作区|这个应用|当前应用|现有实现",
    ) || contains_path_like_token(value)
}

fn explicit_non_workspace_evidence_location(value: &str) -> bool {
    contains_any(
        value,
        "search online|search the web|browse the web|on the website|in the browser|current screen|app window|上网查|联网查|网页|网站|浏览器|当前屏幕|应用窗口",
    )
}

fn software_effect_target(value: &str) -> bool {
    contains_any(
        value,
        "bug|crash|regression|test|code|implementation|project|repo|repository|workspace|app|application|package|dependency|file|config|module|component|function|endpoint|route|button|sidebar|session|settings|performance|代码|项目|仓库|工作区|应用|依赖|文件|配置|模块|组件|函数|按钮|侧边栏|会话|设置|性能|崩溃|闪退|回归|故障",
    ) || contains_path_like_token(value)
}

fn guidance_or_creative_request(value: &str) -> bool {
    contains_any(
        value,
        "how to|how do i|how would you|explain how|teach me|suggest how|give me a plan|write a plan|create a plan|write a project plan|create a project plan|write a checklist|create a checklist|write a code review checklist|create a code review checklist|write a template|translate|summarize|rewrite|draft an announcement|write a poem|如何|怎么做|怎样做|讲讲如何|解释如何|给我一个方案|给我一份计划|写一份计划|创建项目计划|写一份清单|创建清单|写一份项目审查清单|创建项目审查清单|写一个模板|翻译|总结|改写|起草公告|写一首诗",
    )
}

fn effect_explicitly_forbidden(value: &str) -> bool {
    contains_any(
        value,
        "do not change|do not modify|do not edit|do not fix|don't change|don't modify|without changing|without modifying|no code changes|read only|read-only|不要改|不要修改|不要修复|先不要改|别改|不改代码|无需修改|只读",
    )
}

fn explicitly_forbids_all_effects(value: &str) -> bool {
    contains_any(
        value,
        "do not change anything|do not modify anything|do not edit anything|do not fix anything|don't change anything|don't modify anything|without changing anything|without modifying anything|no code changes|read only|read-only|不要改任何|不要修改任何|不要修复任何|先不要改代码|不改代码|只读",
    )
}

fn chinese_conjunction_follows_target(value: &str) -> bool {
    let value = value.trim_end();
    [
        ".rs", ".ts", ".tsx", ".js", ".jsx", ".json", ".toml", ".yaml", ".yml", ".md", ".swift",
        ".py",
    ]
    .into_iter()
    .any(|extension| value.ends_with(extension))
        || [
            "配置",
            "文件",
            "代码",
            "模块",
            "组件",
            "应用",
            "项目",
            "仓库",
            "按钮",
            "侧边栏",
            "会话",
            "设置",
        ]
        .into_iter()
        .any(|target| value.ends_with(target))
}

fn strip_request_prefix(mut value: &str) -> &str {
    loop {
        let trimmed = value.trim_start();
        let next = [
            "initial request:",
            "please ",
            "can you ",
            "could you ",
            "would you ",
            "i want you to ",
            "i need you to ",
            "请",
            "帮我",
            "麻烦",
            "你先",
            "先",
            "继续",
        ]
        .into_iter()
        .find_map(|prefix| trimmed.strip_prefix(prefix));
        match next {
            Some(next) if next.len() < trimmed.len() => value = next,
            _ => return trimmed,
        }
    }
}

fn contains_path_like_token(value: &str) -> bool {
    value.split_whitespace().any(|token| {
        let token = token.trim_matches(|character: char| {
            matches!(
                character,
                ',' | '，' | '.' | '。' | ':' | '：' | ';' | '；' | '(' | ')' | '`'
            )
        });
        token.contains('/')
            || [
                ".rs", ".ts", ".tsx", ".js", ".jsx", ".json", ".toml", ".yaml", ".yml", ".md",
                ".swift", ".py",
            ]
            .iter()
            .any(|extension| token.ends_with(extension))
    })
}

fn starts_with_any(value: &str, prefixes: &str) -> bool {
    prefixes.split('|').any(|prefix| value.starts_with(prefix))
}

fn contains_any(value: &str, signals: &str) -> bool {
    signals
        .split('|')
        .any(|signal| contains_signal(value, signal))
}

fn contains_signal(value: &str, signal: &str) -> bool {
    if signal
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || character == '_')
    {
        value
            .split(|character: char| !character.is_ascii_alphanumeric() && character != '_')
            .any(|token| token == signal)
    } else {
        value.contains(signal)
    }
}

#[cfg(test)]
#[path = "completion_intent_tests.rs"]
mod tests;
