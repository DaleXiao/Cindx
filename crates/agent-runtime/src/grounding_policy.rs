use crate::run_context::{effective_agent_objective, run_context_steer_epoch};
use agent_core::Metadata;
use std::collections::BTreeSet;

const MAX_CLASSIFICATION_INSTRUCTION_CHARS: usize = 16_000;

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum PromptEvidenceScope {
    Workspace,
    External,
    Browser,
    Visual,
}

impl PromptEvidenceScope {
    pub fn requirement_id(self) -> &'static str {
        match self {
            Self::Workspace => "workspace_grounding",
            Self::External => "external_grounding",
            Self::Browser => "browser_grounding",
            Self::Visual => "visual_grounding",
        }
    }
}

pub fn prompt_evidence_scopes(run_context: &Metadata) -> BTreeSet<PromptEvidenceScope> {
    let objective = effective_agent_objective(run_context, "");
    if objective.trim().is_empty() {
        return BTreeSet::new();
    }

    let mut turns = effective_objective_turns(objective);
    if run_context_steer_epoch(run_context) > 0 {
        if let Some(latest) = run_context
            .get("prompt_objective")
            .cloned()
            .filter(|value| !value.trim().is_empty())
        {
            if turns
                .last()
                .is_none_or(|turn| !turn.trim().eq_ignore_ascii_case(latest.trim()))
            {
                turns.push(latest);
            }
        }
    }

    let mut scopes = BTreeSet::new();
    for turn in turns {
        let instruction = classification_instruction_text(&turn).to_ascii_lowercase();
        let classified = if let Some(replacement) = replacement_evidence_request(&instruction) {
            scopes.clear();
            prompt_evidence_scopes_for_instruction(&replacement)
        } else {
            prompt_evidence_scopes_for_instruction(&instruction)
        };
        scopes.extend(classified);
    }
    if run_context.get("task_class").map(String::as_str) == Some("browser") {
        scopes.insert(PromptEvidenceScope::Browser);
    }
    if run_context.get("vision_required").map(String::as_str) == Some("true") {
        scopes.insert(PromptEvidenceScope::Visual);
    }
    scopes
}

fn prompt_evidence_scopes_for_instruction(instruction: &str) -> BTreeSet<PromptEvidenceScope> {
    if instruction.trim().is_empty() {
        return BTreeSet::new();
    }
    let shared_action =
        evidence_action_requested(instruction) || retrieval_action_requested(instruction);

    evidence_clauses(instruction)
        .into_iter()
        .filter(|clause| {
            !capability_or_guidance_request(clause)
                && !(template_or_creative_request(clause)
                    && !explicit_evidence_execution_request(clause))
        })
        .filter_map(|clause| prompt_evidence_scope_for_clause(clause, shared_action))
        .collect()
}

fn classification_instruction_text(value: &str) -> String {
    let mut output = String::with_capacity(value.len().min(8_192));
    let mut remaining_chars = MAX_CLASSIFICATION_INSTRUCTION_CHARS;
    let mut remaining = value;
    let mut removed_fence = false;
    while let Some(start) = remaining.find("```") {
        append_bounded_classification_text(&mut output, &remaining[..start], &mut remaining_chars);
        let fenced = &remaining[start + 3..];
        let Some(end) = fenced.find("```") else {
            removed_fence = true;
            remaining = "";
            break;
        };
        append_bounded_classification_text(&mut output, " pasted below ", &mut remaining_chars);
        removed_fence = true;
        remaining = &fenced[end + 3..];
        if remaining_chars == 0 {
            break;
        }
    }
    append_bounded_classification_text(&mut output, remaining, &mut remaining_chars);
    if !removed_fence {
        for marker in [
            "content below",
            "code below",
            "pasted code",
            "inline code",
            "pasted below",
            "内容如下",
            "以下代码",
            "以下文字",
            "粘贴的代码",
            "内联代码",
            "粘贴如下",
        ] {
            if let Some(index) = output.find(marker) {
                if let Some(newline) = output[index..].find('\n') {
                    output.truncate(index + newline);
                }
                break;
            }
        }
    }
    output
}

fn append_bounded_classification_text(
    output: &mut String,
    value: &str,
    remaining_chars: &mut usize,
) {
    if *remaining_chars == 0 {
        return;
    }
    for character in value.chars().take(*remaining_chars) {
        output.push(character);
        *remaining_chars -= 1;
    }
}

fn prompt_evidence_scope_for_clause(
    objective: &str,
    inherited_action: bool,
) -> Option<PromptEvidenceScope> {
    let workspace_target = contains_signal(
        objective,
        &[
            "project",
            "repo",
            "repository",
            "codebase",
            "workspace",
            "current code",
            "code",
            "implementation",
            "file",
            "directory",
            "config",
            "log",
            "项目",
            "仓库",
            "代码库",
            "工作区",
            "代码",
            "这个实现",
            "该实现",
            "现有实现",
            "文件",
            "目录",
            "配置",
            "日志",
        ],
    ) || contains_workspace_path(objective);
    let external_target = contains_signal(
        objective,
        &[
            "web",
            "internet",
            "online",
            "website",
            "url",
            "link",
            "sources",
            "references",
            "citation",
            "news",
            "current version",
            "latest version",
            "latest release",
            "网络",
            "互联网",
            "网站",
            "网页",
            "网址",
            "链接",
            "来源",
            "出处",
            "证据",
            "引用",
            "新闻",
            "当前版本",
            "最新版本",
            "最新发布",
        ],
    ) || contains_http_url(objective)
        || (contains_signal(objective, &["latest", "current", "最新", "当前"])
            && contains_signal(
                objective,
                &[
                    "version",
                    "release",
                    "model",
                    "api",
                    "documentation",
                    "docs",
                    "版本",
                    "发布",
                    "模型",
                    "接口",
                    "文档",
                ],
            ))
        || (!workspace_target && contains_signal(objective, &["documentation", "docs", "文档"]));
    let visual_target = contains_signal(
        objective,
        &[
            "screenshot",
            "screen",
            "visible ui",
            "current ui",
            "user interface",
            "app window",
            "application window",
            "截图",
            "屏幕",
            "界面",
        ],
    );
    let browser_execution = contains_signal(
        objective,
        &[
            "browser tool",
            "in the browser",
            "using the browser",
            "use browser",
            "browser.open",
            "browser.extract_text",
            "browser.capture",
            "用浏览器",
            "浏览器中",
            "浏览器里",
        ],
    ) && contains_signal(
        objective,
        &[
            "open", "inspect", "capture", "extract", "browse", "click", "打开", "检查", "查看",
            "截图", "提取", "点击",
        ],
    );
    let evidence_action = evidence_action_requested(objective) || inherited_action;
    let retrieval_action = retrieval_action_requested(objective) || inherited_action;
    let visual_observation = contains_signal(
        objective,
        &[
            "look at",
            "read",
            "identify",
            "observe",
            "what is on",
            "what's on",
            "tell me what",
            "看看",
            "查看",
            "观察",
            "读取",
            "识别",
            "有什么",
        ],
    );
    let self_contained_material = objective.contains("```")
        || contains_signal(
            objective,
            &[
                "this paragraph",
                "this text",
                "code above",
                "text above",
                "content below",
                "this file content",
                "inline code",
                "pasted code",
                "pasted below",
                "the following text",
                "the following code",
                "attached image",
                "attached screenshot",
                "this screenshot",
                "image above",
                "这段文字",
                "这段代码",
                "这段配置",
                "上面的代码",
                "上述代码",
                "上文",
                "内容如下",
                "内联代码",
                "粘贴的代码",
                "以下文字",
                "以下代码",
                "粘贴如下",
                "附图",
                "附上的截图",
                "这张截图",
                "上图",
            ],
        );
    let instruction_text = instruction_text_before_supplied_material(objective);
    let explicit_location = contains_signal(
        instruction_text,
        &[
            "project",
            "repo",
            "repository",
            "codebase",
            "workspace",
            "path",
            "directory",
            "website",
            "screen",
            "项目",
            "仓库",
            "代码库",
            "工作区",
            "路径",
            "目录",
            "网站",
            "网址",
            "屏幕",
        ],
    ) || contains_signal(
        instruction_text,
        &[
            "search online",
            "search the web",
            "browse the web",
            "look up online",
            "上网查",
            "联网查",
        ],
    );
    if (evidence_action || retrieval_action || visual_observation)
        && self_contained_material
        && !explicit_location
    {
        return None;
    }
    let explicit_external_retrieval = contains_signal(
        objective,
        &[
            "search for",
            "search the web",
            "web search",
            "search online",
            "look up",
            "browse for",
            "browse the web",
            "find sources",
            "research online",
            "retrieve sources",
            "搜索一下",
            "搜一下",
            "检索一下",
            "查一下",
            "上网查",
            "联网查",
            "查找资料",
            "找一下资料",
        ],
    );
    let source_request = contains_signal(
        objective,
        &[
            "sources",
            "references",
            "citation",
            "来源",
            "出处",
            "证据",
            "引用",
        ],
    ) && contains_signal(
        objective,
        &[
            "give", "provide", "list", "find", "cite", "给", "提供", "列出", "找", "引用",
        ],
    );
    let temporal_request = contains_signal(
        objective,
        &[
            "current version",
            "latest version",
            "latest release",
            "当前版本",
            "最新版本",
            "最新发布",
        ],
    ) && contains_signal(
        objective,
        &[
            "what",
            "which",
            "find",
            "check",
            "verify",
            "search",
            "是什么",
            "哪个",
            "查",
            "找",
            "核实",
        ],
    );
    let implicit_fact_check = !workspace_target
        && !visual_target
        && (contains_signal(
            objective,
            &["fact-check", "cross-check", "核实", "核验", "查证"],
        ) || (contains_signal(objective, &["verify", "验证"])
            && contains_signal(
                objective,
                &[
                    "claim", "fact", "whether", "true", "说法", "事实", "是否", "属实", "真假",
                ],
            )));
    let objective_start = objective.trim_start();
    let imperative_external_retrieval = ["search ", "find ", "look up "]
        .iter()
        .any(|prefix| objective_start.starts_with(prefix));

    if browser_execution {
        return Some(PromptEvidenceScope::Browser);
    }
    if visual_target && (evidence_action || visual_observation) {
        return Some(PromptEvidenceScope::Visual);
    }
    if (external_target && (evidence_action || retrieval_action))
        || source_request
        || temporal_request
        || implicit_fact_check
    {
        return Some(PromptEvidenceScope::External);
    }
    if workspace_target && (evidence_action || retrieval_action) {
        return Some(PromptEvidenceScope::Workspace);
    }
    if explicit_external_retrieval
        || (imperative_external_retrieval
            && !workspace_target
            && !visual_target
            && !self_contained_material)
    {
        return Some(PromptEvidenceScope::External);
    }
    None
}

fn evidence_action_requested(value: &str) -> bool {
    contains_signal(
        value,
        &[
            "audit",
            "review",
            "inspect",
            "verify",
            "validate",
            "check",
            "fact-check",
            "cross-check",
            "analyze",
            "assess",
            "investigate",
            "diagnose",
            "审核",
            "审查",
            "检查",
            "核实",
            "核验",
            "验证",
            "复核",
            "查证",
            "分析",
            "评估",
            "调查",
            "诊断",
        ],
    )
}

fn retrieval_action_requested(value: &str) -> bool {
    contains_signal(
        value,
        &[
            "search",
            "find",
            "locate",
            "retrieve",
            "read",
            "open",
            "look up",
            "browse",
            "research",
            "搜索",
            "检索",
            "搜一下",
            "查一下",
            "查询",
            "查找",
            "定位",
            "读取",
            "打开",
            "查资料",
            "找资料",
        ],
    )
}

fn evidence_clauses(value: &str) -> Vec<&str> {
    let mut clauses = vec![value];
    for separator in [
        ";",
        "；",
        ",",
        "、",
        " and ",
        " then ",
        " as well as ",
        ". ",
        "。",
        "并且",
        "，并且",
        "，并",
        "，然后",
        "，同时",
    ] {
        clauses = clauses
            .into_iter()
            .flat_map(|clause| clause.split(separator))
            .collect();
    }
    clauses
        .into_iter()
        .map(str::trim)
        .filter(|clause| !clause.is_empty())
        .collect()
}

fn effective_objective_turns(objective: &str) -> Vec<String> {
    let Some(first_newline) = objective.find('\n') else {
        return vec![objective.to_string()];
    };
    if !objective[..first_newline]
        .trim_end_matches('\r')
        .eq_ignore_ascii_case("initial request:")
    {
        return vec![objective.to_string()];
    }

    let initial_start = first_newline + 1;
    let mut turn_ranges = Vec::new();
    let mut turn_start = initial_start;
    let mut expected_steer = 1usize;
    let mut inside_fence = false;
    let mut line_start = initial_start;

    for line in objective[initial_start..].split_inclusive('\n') {
        let label = line.trim_end_matches(['\r', '\n']);
        let expected_label = format!("accepted steering {expected_steer}:");
        if !inside_fence && label.eq_ignore_ascii_case(&expected_label) {
            turn_ranges.push((turn_start, line_start));
            turn_start = line_start + line.len();
            expected_steer = expected_steer.saturating_add(1);
        } else if line.match_indices("```").count() % 2 == 1 {
            inside_fence = !inside_fence;
        }
        line_start += line.len();
    }
    turn_ranges.push((turn_start, objective.len()));

    let turns = turn_ranges
        .into_iter()
        .map(|(start, end)| objective[start..end].trim().to_string())
        .filter(|turn| !turn.is_empty())
        .collect::<Vec<_>>();
    if turns.is_empty() {
        vec![objective.to_string()]
    } else {
        turns
    }
}

fn replacement_evidence_request(value: &str) -> Option<String> {
    let value = value.trim();
    if contains_signal(value, &["do not stop", "don't stop", "不要停止"]) {
        return None;
    }
    let replacement_marker = [
        "改成只",
        "改为只",
        "换成只",
        "改成写",
        "改为写",
        "任务改成",
        "任务改为",
        "目标改成",
        "目标改为",
        "actually, just",
        "actually just",
    ]
    .into_iter()
    .find_map(|marker| value.find(marker).map(|index| (marker, index)))
    .or_else(|| {
        value.match_indices("instead").find_map(|(index, marker)| {
            let explicit_boundary = index == 0
                || value[..index]
                    .trim_end()
                    .chars()
                    .next_back()
                    .is_some_and(|character| matches!(character, ';' | '；' | ',' | '，'));
            explicit_boundary.then_some((marker, index))
        })
    });
    if let Some((marker, index)) = replacement_marker {
        let after = value[index + marker.len()..].trim_start_matches(|character: char| {
            character.is_whitespace() || matches!(character, ',' | '，' | ':' | '：')
        });
        if !after.is_empty() {
            return Some(after.to_string());
        }
        let before = value[..index].trim_end();
        if let Some((_, clause)) = before.rsplit_once([';', '；', ',', '，']) {
            return Some(clause.trim().to_string());
        }
    }

    let strong_cancellation = contains_signal(
        value,
        &[
            "stop auditing",
            "stop the audit",
            "stop reviewing",
            "stop the review",
            "stop inspecting",
            "stop verifying",
            "stop checking",
            "stop searching",
            "stop retrieving",
            "cancel the audit",
            "cancel the review",
            "cancel the search",
            "no longer audit",
            "no longer review",
            "no longer inspect",
            "no longer verify",
            "no longer check",
            "no longer search",
            "停止审核",
            "停止审查",
            "停止检查",
            "停止核实",
            "停止验证",
            "停止搜索",
            "停止检索",
            "取消审核",
            "取消审查",
            "取消检查",
            "取消核实",
            "取消搜索",
            "取消检索",
        ],
    );
    if strong_cancellation {
        let replacement = [
            ";", "；", ",", "，", ". ", "。", "? ", "？", "\n", " and ", " then ", "然后",
        ]
        .into_iter()
        .filter_map(|separator| {
            value
                .find(separator)
                .map(|index| (index, index + separator.len()))
        })
        .min_by_key(|(index, _)| *index)
        .and_then(|(_, start)| {
            let remainder = value[start..].trim();
            (!remainder.is_empty()).then(|| remainder.to_string())
        });
        if replacement.is_some() {
            return replacement;
        }
        return Some(String::new());
    }
    None
}

fn capability_or_guidance_request(value: &str) -> bool {
    let guidance = contains_signal(
        value,
        &[
            "how to",
            "how do i",
            "how should i",
            "what is",
            "explain how",
            "explain what",
            "teach me how",
            "如何",
            "怎么",
            "怎样",
            "什么是",
            "解释如何",
            "讲讲如何",
        ],
    ) && contains_signal(
        value,
        &[
            "audit",
            "review",
            "inspect",
            "verify",
            "search",
            "fact-check",
            "审核",
            "审查",
            "检查",
            "核实",
            "验证",
            "搜索",
            "检索",
            "查证",
        ],
    );
    guidance && !has_followup_evidence_command(value)
}

fn has_followup_evidence_command(value: &str) -> bool {
    value
        .split(['?', '？', '.', '。', ';', '；', '\n'])
        .skip(1)
        .any(|clause| {
            contains_signal(
                clause,
                &[
                    "audit",
                    "review",
                    "inspect",
                    "verify",
                    "validate",
                    "check",
                    "search",
                    "browse",
                    "look up",
                    "审核",
                    "审查",
                    "检查",
                    "核实",
                    "验证",
                    "搜索",
                    "检索",
                    "查证",
                    "上网查",
                ],
            )
        })
}

fn explicit_evidence_execution_request(value: &str) -> bool {
    let value = value.trim_start();
    [
        "audit ",
        "review ",
        "inspect ",
        "verify ",
        "validate ",
        "check ",
        "search ",
        "find ",
        "审核",
        "审查",
        "检查",
        "核实",
        "验证",
        "搜索",
        "检索",
        "查找",
    ]
    .iter()
    .any(|prefix| value.starts_with(prefix))
}

fn instruction_text_before_supplied_material(value: &str) -> &str {
    let mut end = value.find("```").unwrap_or(value.len());
    for marker in [
        "content below",
        "code below",
        "pasted code",
        "inline code",
        "pasted below",
        "内容如下",
        "以下代码",
        "以下文字",
        "粘贴的代码",
        "内联代码",
        "粘贴如下",
    ] {
        if let Some(index) = value.find(marker) {
            let marker_end = index + marker.len();
            let instruction_end = value[marker_end..]
                .find('\n')
                .map(|offset| marker_end + offset)
                .unwrap_or(marker_end);
            end = end.min(instruction_end);
        }
    }
    &value[..end]
}

fn template_or_creative_request(value: &str) -> bool {
    let artifact = contains_signal(
        value,
        &[
            "checklist",
            "plan",
            "template",
            "example",
            "poem",
            "story",
            "guide",
            "清单",
            "计划",
            "模板",
            "示例",
            "诗",
            "故事",
            "指南",
        ],
    );
    let creation = contains_signal(
        value,
        &[
            "write",
            "draft",
            "create",
            "generate",
            "make",
            "写",
            "起草",
            "创建",
            "生成",
            "列一份",
            "给我一个",
            "给我一份",
        ],
    );
    let fictional = contains_signal(
        value,
        &["fictional", "hypothetical", "mock", "虚构", "假想", "模拟"],
    ) && contains_signal(
        value,
        &["audit", "review", "report", "审核", "审查", "报告"],
    );
    fictional
        || (artifact && creation)
        || contains_signal(
            value,
            &[
                "review checklist",
                "audit checklist",
                "review template",
                "audit template",
                "审核清单",
                "审查清单",
                "检查清单",
                "审核模板",
                "审查模板",
            ],
        )
}

fn contains_workspace_path(value: &str) -> bool {
    value.split_whitespace().any(|raw| {
        let token = raw.trim_matches(|character: char| {
            matches!(
                character,
                '"' | '\'' | '`' | '(' | ')' | '[' | ']' | '{' | '}' | ',' | '，' | ';' | '；'
            )
        });
        if token.starts_with("http://") || token.starts_with("https://") {
            return false;
        }
        if token.starts_with("./")
            || token.starts_with("../")
            || token.starts_with('/')
            || token.starts_with("src/")
            || token.starts_with("apps/")
            || token.starts_with("crates/")
            || token.starts_with("packages/")
            || token.starts_with("tests/")
        {
            return true;
        }
        let file_name = token
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or(token)
            .split(':')
            .next()
            .unwrap_or(token);
        if matches!(
            file_name,
            "dockerfile" | "makefile" | "readme" | "cargo.toml" | "package.json"
        ) {
            return true;
        }
        file_name.rsplit_once('.').is_some_and(|(_, extension)| {
            matches!(
                extension,
                "rs" | "ts"
                    | "tsx"
                    | "js"
                    | "jsx"
                    | "json"
                    | "toml"
                    | "yaml"
                    | "yml"
                    | "md"
                    | "py"
                    | "go"
                    | "java"
                    | "kt"
                    | "swift"
                    | "c"
                    | "cc"
                    | "cpp"
                    | "h"
                    | "hpp"
                    | "sh"
                    | "zsh"
                    | "lock"
                    | "env"
            )
        })
    })
}

fn contains_http_url(value: &str) -> bool {
    ["https://", "http://"].into_iter().any(|scheme| {
        value.match_indices(scheme).any(|(index, _)| {
            value[..index]
                .chars()
                .next_back()
                .is_none_or(|character| !character.is_ascii_alphanumeric() && character != '_')
        })
    })
}

fn contains_signal(value: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| {
        if !needle.is_ascii() {
            return value.contains(needle);
        }
        value.match_indices(needle).any(|(start, _)| {
            let end = start + needle.len();
            let left_boundary = value[..start]
                .chars()
                .next_back()
                .is_none_or(|character| !character.is_ascii_alphanumeric() && character != '_');
            let right_boundary = value[end..]
                .chars()
                .next()
                .is_none_or(|character| !character.is_ascii_alphanumeric() && character != '_');
            left_boundary && right_boundary
        })
    })
}
