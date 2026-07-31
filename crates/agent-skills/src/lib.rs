use agent_core::{
    Metadata, ToolExecutionConcurrency, ToolExposure, ToolInvocation, ToolOutcomeStatus,
    ToolResult, ToolRisk, ToolSource, ToolSpec,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
#[cfg(test)]
use std::io::Write;
use std::io::{Cursor, Read};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use tools::{write_private_file_atomically, Tool, ToolError};

const MAX_SKILL_INSTRUCTIONS: usize = 16_000;
const MAX_SELECTED_SKILLS: usize = 2;
const MAX_INSTALL_FILES: usize = 256;
const MAX_INSTALL_FILE_BYTES: usize = 20 * 1024 * 1024;
const MAX_INSTALL_TOTAL_BYTES: usize = 50 * 1024 * 1024;
const BUILTIN_SKILL_CREATOR_ID: &str = "global:skill-creator";
const BUILTIN_SKILL_CREATOR: &str = include_str!("../builtins/skill-creator/SKILL.md");

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SkillScope {
    Global,
    Project,
    Compatibility,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SkillRecord {
    pub id: String,
    pub name: String,
    pub description: String,
    pub folder_name: String,
    pub root: PathBuf,
    pub scope: SkillScope,
    pub enabled: bool,
    pub trusted: bool,
    pub required_tools: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SkillPreference {
    pub enabled: bool,
    pub trusted: bool,
}

#[derive(Debug, Clone)]
pub struct SkillInstallFile {
    pub path: PathBuf,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct SkillPreferences {
    #[serde(default)]
    skills: BTreeMap<String, SkillPreference>,
}

#[derive(Debug, Default)]
struct SkillCatalogState {
    skills: Vec<SkillRecord>,
    preferences: SkillPreferences,
}

#[derive(Clone)]
pub struct SkillCatalog {
    global_root: PathBuf,
    project_root: PathBuf,
    preferences_path: PathBuf,
    state: Arc<Mutex<SkillCatalogState>>,
}

impl SkillCatalog {
    pub fn load(
        global_root: impl Into<PathBuf>,
        project_root: impl Into<PathBuf>,
        preferences_path: impl Into<PathBuf>,
    ) -> Self {
        let preferences_path = preferences_path.into();
        let preferences = fs::read_to_string(&preferences_path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        let catalog = Self {
            global_root: global_root.into(),
            project_root: project_root.into(),
            preferences_path,
            state: Arc::new(Mutex::new(SkillCatalogState {
                skills: Vec::new(),
                preferences,
            })),
        };
        let _ = catalog.refresh();
        catalog
    }

    pub fn refresh(&self) -> Result<Vec<SkillRecord>, String> {
        let preferences = self
            .state
            .lock()
            .map_err(|error| format!("skill catalog lock poisoned: {error}"))?
            .preferences
            .clone();
        let mut skills = Vec::new();
        let builtin_preference = preferences.skills.get(BUILTIN_SKILL_CREATOR_ID);
        skills.push(SkillRecord {
            id: BUILTIN_SKILL_CREATOR_ID.to_string(),
            name: "Claude Code Skill Creator".to_string(),
            description: "Create and improve Claude Code compatible skills with focused instructions, resources, and validation.".to_string(),
            folder_name: "skill-creator".to_string(),
            root: PathBuf::from("__cindx_builtin__/skill-creator"),
            scope: SkillScope::Global,
            enabled: builtin_preference
                .map(|preference| preference.enabled)
                .unwrap_or(true),
            trusted: builtin_preference
                .map(|preference| preference.trusted)
                .unwrap_or(true),
            required_tools: vec![
                "file.read".to_string(),
                "file.list".to_string(),
                "file.search".to_string(),
                "file.write".to_string(),
                "shell.run".to_string(),
            ],
        });
        let roots = [
            (self.global_root.clone(), SkillScope::Global, true),
            (
                self.project_root.join(".cindx/skills"),
                SkillScope::Project,
                true,
            ),
            (
                self.project_root.join(".agents/skills"),
                SkillScope::Compatibility,
                false,
            ),
            (
                self.project_root.join(".claude/skills"),
                SkillScope::Compatibility,
                false,
            ),
        ];
        let mut seen = BTreeSet::from([BUILTIN_SKILL_CREATOR_ID.to_string()]);
        for (root, scope, trusted_default) in roots {
            for mut skill in discover_skills(&root, scope.clone())? {
                if !seen.insert(skill.id.clone()) {
                    continue;
                }
                let preference = preferences.skills.get(&skill.id);
                skill.enabled = preference
                    .map(|preference| preference.enabled)
                    .unwrap_or(trusted_default);
                skill.trusted = preference
                    .map(|preference| preference.trusted)
                    .unwrap_or(trusted_default);
                skills.push(skill);
            }
        }
        skills.sort_by(|left, right| left.name.cmp(&right.name).then(left.id.cmp(&right.id)));
        self.state
            .lock()
            .map_err(|error| format!("skill catalog lock poisoned: {error}"))?
            .skills = skills.clone();
        Ok(skills)
    }

    pub fn list(&self) -> Vec<SkillRecord> {
        self.state
            .lock()
            .map(|state| state.skills.clone())
            .unwrap_or_default()
    }

    pub fn set_preference(
        &self,
        skill_id: &str,
        preference: SkillPreference,
    ) -> Result<Vec<SkillRecord>, String> {
        {
            let mut state = self
                .state
                .lock()
                .map_err(|error| format!("skill catalog lock poisoned: {error}"))?;
            if !state.skills.iter().any(|skill| skill.id == skill_id) {
                return Err(format!("unknown skill: {skill_id}"));
            }
            state
                .preferences
                .skills
                .insert(skill_id.to_string(), preference);
            write_private_json(&self.preferences_path, &state.preferences)?;
        }
        self.refresh()
    }

    pub fn context_for_prompt(&self, prompt: &str) -> Result<Option<String>, String> {
        let selected = self.select(prompt, MAX_SELECTED_SKILLS);
        if selected.is_empty() {
            return Ok(None);
        }
        let mut sections = Vec::new();
        for skill in selected {
            sections.push(format!(
                "<skill name=\"{}\" id=\"{}\">\n{}\n</skill>",
                skill.name,
                skill.id,
                read_skill_instructions(&skill)?
            ));
        }
        Ok(Some(format!(
            "Cindx selected the following trusted skills for this request. Follow their guidance, but do not bypass tool permissions or execute scripts except through registered tools.\n\n{}",
            sections.join("\n\n")
        )))
    }

    pub fn tools(&self) -> Vec<Box<dyn Tool>> {
        vec![
            Box::new(SkillSearchTool {
                catalog: self.clone(),
            }),
            Box::new(SkillLoadTool {
                catalog: self.clone(),
            }),
        ]
    }

    fn select(&self, prompt: &str, limit: usize) -> Vec<SkillRecord> {
        let query = prompt.to_ascii_lowercase();
        let query_tokens = token_set(&query);
        let mut scored = self
            .list()
            .into_iter()
            .filter(|skill| skill.enabled && skill.trusted)
            .filter_map(|skill| {
                let name = skill.name.to_ascii_lowercase();
                let description = skill.description.to_ascii_lowercase();
                let explicit = query.contains(&name)
                    || query.contains(&skill.folder_name.to_ascii_lowercase());
                let overlap = token_set(&format!("{name} {description}"))
                    .intersection(&query_tokens)
                    .count();
                let score = usize::from(explicit) * 100 + overlap;
                (score > 0).then_some((score, skill))
            })
            .collect::<Vec<_>>();
        scored.sort_by(|left, right| right.0.cmp(&left.0).then(left.1.name.cmp(&right.1.name)));
        scored
            .into_iter()
            .take(limit)
            .map(|(_, skill)| skill)
            .collect()
    }
}

pub fn install_skill_archive(
    destination_root: &Path,
    archive_bytes: &[u8],
) -> Result<String, String> {
    if archive_bytes.len() > MAX_INSTALL_TOTAL_BYTES {
        return Err("skill package exceeds the 50 MB limit".to_string());
    }
    let mut archive = zip::ZipArchive::new(Cursor::new(archive_bytes))
        .map_err(|error| format!("invalid .skill package: {error}"))?;
    if archive.len() > MAX_INSTALL_FILES {
        return Err(format!(
            "skill package contains more than {MAX_INSTALL_FILES} entries"
        ));
    }

    let mut files = Vec::new();
    let mut total_bytes = 0usize;
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|error| format!("failed to read skill package: {error}"))?;
        if entry.is_dir() {
            continue;
        }
        if entry
            .unix_mode()
            .map(|mode| mode & 0o170000 == 0o120000)
            .unwrap_or(false)
        {
            return Err("skill packages cannot contain symbolic links".to_string());
        }
        let path = entry
            .enclosed_name()
            .ok_or_else(|| "skill package contains an unsafe path".to_string())?
            .to_path_buf();
        if path
            .components()
            .next()
            .is_some_and(|component| component.as_os_str() == "__MACOSX")
        {
            continue;
        }
        let declared_size = usize::try_from(entry.size())
            .map_err(|_| "skill package entry is too large".to_string())?;
        if declared_size > MAX_INSTALL_FILE_BYTES {
            return Err(format!("{} exceeds the 20 MB file limit", path.display()));
        }
        total_bytes = total_bytes.saturating_add(declared_size);
        if total_bytes > MAX_INSTALL_TOTAL_BYTES {
            return Err("skill package expands beyond the 50 MB limit".to_string());
        }
        let mut bytes = Vec::with_capacity(declared_size);
        entry
            .read_to_end(&mut bytes)
            .map_err(|error| format!("failed to extract {}: {error}", path.display()))?;
        files.push(SkillInstallFile { path, bytes });
    }
    install_skill_files(destination_root, files)
}

pub fn install_skill_files(
    destination_root: &Path,
    files: Vec<SkillInstallFile>,
) -> Result<String, String> {
    if files.is_empty() {
        return Err("the selected skill is empty".to_string());
    }
    if files.len() > MAX_INSTALL_FILES {
        return Err(format!(
            "a skill can contain at most {MAX_INSTALL_FILES} files"
        ));
    }

    let mut normalized_files = Vec::with_capacity(files.len());
    let mut total_bytes = 0usize;
    for file in files {
        let path = normalize_install_path(&file.path)?;
        if file.bytes.len() > MAX_INSTALL_FILE_BYTES {
            return Err(format!("{} exceeds the 20 MB file limit", path.display()));
        }
        total_bytes = total_bytes.saturating_add(file.bytes.len());
        if total_bytes > MAX_INSTALL_TOTAL_BYTES {
            return Err("skill files exceed the 50 MB limit".to_string());
        }
        normalized_files.push(SkillInstallFile {
            path,
            bytes: file.bytes,
        });
    }

    let skill_files = normalized_files
        .iter()
        .filter(|file| file.path.file_name().is_some_and(|name| name == "SKILL.md"))
        .collect::<Vec<_>>();
    if skill_files.len() != 1 {
        return Err("a skill must contain exactly one SKILL.md".to_string());
    }
    let source_root = skill_files[0]
        .path
        .parent()
        .unwrap_or_else(|| Path::new(""))
        .to_path_buf();
    let skill_text = std::str::from_utf8(&skill_files[0].bytes)
        .map_err(|_| "SKILL.md must be valid UTF-8".to_string())?;
    let metadata = parse_frontmatter(skill_text);
    let name = metadata
        .get("name")
        .map(String::as_str)
        .ok_or_else(|| "SKILL.md must declare a name".to_string())?;
    validate_skill_name(name)?;
    if metadata
        .get("description")
        .is_none_or(|description| description.trim().is_empty())
    {
        return Err("SKILL.md must declare a description".to_string());
    }
    if let Some(folder_name) = source_root.file_name().and_then(|value| value.to_str()) {
        if folder_name != name {
            return Err(format!(
                "skill folder '{folder_name}' must match the SKILL.md name '{name}'"
            ));
        }
    }

    fs::create_dir_all(destination_root)
        .map_err(|error| format!("failed to create skill directory: {error}"))?;
    let target = destination_root.join(name);
    if target.exists() {
        return Err(format!("skill '{name}' is already installed"));
    }
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let staging = destination_root.join(format!(".{name}-install-{nonce}"));
    fs::create_dir(&staging)
        .map_err(|error| format!("failed to prepare skill installation: {error}"))?;

    let install_result = (|| {
        for file in normalized_files {
            let Ok(relative) = file.path.strip_prefix(&source_root) else {
                continue;
            };
            if relative.as_os_str().is_empty() {
                continue;
            }
            let path = staging.join(relative);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)
                    .map_err(|error| format!("failed to create skill directory: {error}"))?;
            }
            fs::write(&path, file.bytes)
                .map_err(|error| format!("failed to write {}: {error}", path.display()))?;
        }
        if !staging.join("SKILL.md").is_file() {
            return Err("skill installation did not produce SKILL.md".to_string());
        }
        fs::rename(&staging, &target)
            .map_err(|error| format!("failed to finish skill installation: {error}"))?;
        Ok(format!("project:{name}"))
    })();
    if install_result.is_err() {
        let _ = fs::remove_dir_all(&staging);
    }
    install_result
}

fn normalize_install_path(path: &Path) -> Result<PathBuf, String> {
    if path.as_os_str().is_empty() || path.to_string_lossy().contains('\\') {
        return Err("skill contains an invalid path".to_string());
    }
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(value) => normalized.push(value),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(format!("unsafe skill path: {}", path.display()));
            }
        }
    }
    if normalized.as_os_str().is_empty() {
        return Err("skill contains an invalid path".to_string());
    }
    Ok(normalized)
}

fn validate_skill_name(name: &str) -> Result<(), String> {
    let valid = !name.is_empty()
        && name.len() <= 64
        && !name.starts_with('-')
        && !name.ends_with('-')
        && !name.contains("--")
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-');
    if valid {
        Ok(())
    } else {
        Err("skill name must use 1-64 lowercase letters, numbers, or single hyphens".to_string())
    }
}

struct SkillSearchTool {
    catalog: SkillCatalog,
}

impl Tool for SkillSearchTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::new(
            "skill.search",
            "skill",
            "Search enabled and trusted Cindx skills by name or description.",
            ToolRisk::ReadOnly,
            ToolSource::BuiltIn,
            ToolExposure::Inline,
            json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Skill search query" }
                },
                "required": ["query"],
                "additionalProperties": false
            })
            .to_string(),
        )
        .with_execution_concurrency(ToolExecutionConcurrency::IndependentRead)
    }

    fn permission_request(
        &self,
        _invocation: &ToolInvocation,
    ) -> Option<agent_core::PermissionRequest> {
        None
    }

    fn execute(&self, invocation: ToolInvocation) -> Result<ToolResult, ToolError> {
        let input: serde_json::Value = serde_json::from_str(&invocation.input_json)
            .map_err(|error| ToolError::new(format!("invalid skill search input: {error}")))?;
        let query = input
            .get("query")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        let rows = self
            .catalog
            .select(query, 8)
            .into_iter()
            .map(|skill| format!("{}\t{}\t{}", skill.id, skill.name, skill.description))
            .collect::<Vec<_>>();
        Ok(ToolResult::text(
            invocation.id,
            ToolOutcomeStatus::Succeeded,
            if rows.is_empty() {
                "No matching enabled skills.".to_string()
            } else {
                rows.join("\n")
            },
            Metadata::new(),
        ))
    }
}

struct SkillLoadTool {
    catalog: SkillCatalog,
}

impl Tool for SkillLoadTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::new(
            "skill.load",
            "skill",
            "Load the instructions for one enabled and trusted Cindx skill.",
            ToolRisk::ReadOnly,
            ToolSource::BuiltIn,
            ToolExposure::Inline,
            json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Skill id, name, or folder name" }
                },
                "required": ["name"],
                "additionalProperties": false
            })
            .to_string(),
        )
        .with_execution_concurrency(ToolExecutionConcurrency::IndependentRead)
    }

    fn permission_request(
        &self,
        _invocation: &ToolInvocation,
    ) -> Option<agent_core::PermissionRequest> {
        None
    }

    fn execute(&self, invocation: ToolInvocation) -> Result<ToolResult, ToolError> {
        let input: serde_json::Value = serde_json::from_str(&invocation.input_json)
            .map_err(|error| ToolError::new(format!("invalid skill load input: {error}")))?;
        let name = input
            .get("name")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        let normalized = name.to_ascii_lowercase();
        let skill = self
            .catalog
            .list()
            .into_iter()
            .find(|skill| {
                skill.enabled
                    && skill.trusted
                    && (skill.id.to_ascii_lowercase() == normalized
                        || skill.name.to_ascii_lowercase() == normalized
                        || skill.folder_name.to_ascii_lowercase() == normalized)
            })
            .ok_or_else(|| ToolError::new("skill is missing, disabled, or untrusted"))?;
        let instructions = read_skill_instructions(&skill).map_err(ToolError::new)?;
        Ok(ToolResult::text(
            invocation.id,
            ToolOutcomeStatus::Succeeded,
            instructions,
            [("skill_id".to_string(), skill.id)].into_iter().collect(),
        ))
    }
}

fn discover_skills(root: &Path, scope: SkillScope) -> Result<Vec<SkillRecord>, String> {
    let Ok(entries) = fs::read_dir(root) else {
        return Ok(Vec::new());
    };
    let mut skills = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| format!("failed to read skill directory: {error}"))?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let skill_path = path.join("SKILL.md");
        if !skill_path.is_file() {
            continue;
        }
        let folder_name = entry.file_name().to_string_lossy().to_string();
        let text = fs::read_to_string(&skill_path)
            .map_err(|error| format!("failed to read {}: {error}", skill_path.display()))?;
        let metadata = parse_frontmatter(&text);
        let name = metadata
            .get("name")
            .cloned()
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| folder_name.clone());
        let description = metadata.get("description").cloned().unwrap_or_default();
        let required_tools = metadata
            .get("allowed-tools")
            .or_else(|| metadata.get("required-tools"))
            .map(|tools| {
                tools
                    .split([',', ' '])
                    .map(str::trim)
                    .filter(|tool| !tool.is_empty())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        skills.push(SkillRecord {
            id: format!("{}:{}", scope_label(&scope), folder_name),
            name,
            description,
            folder_name,
            root: path,
            scope: scope.clone(),
            enabled: false,
            trusted: false,
            required_tools,
        });
    }
    Ok(skills)
}

fn read_skill_instructions(skill: &SkillRecord) -> Result<String, String> {
    if skill.id == BUILTIN_SKILL_CREATOR_ID {
        return Ok(strip_frontmatter(BUILTIN_SKILL_CREATOR)
            .chars()
            .take(MAX_SKILL_INSTRUCTIONS)
            .collect());
    }
    let path = skill.root.join("SKILL.md");
    let text = fs::read_to_string(&path)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    let body = strip_frontmatter(&text);
    Ok(body.chars().take(MAX_SKILL_INSTRUCTIONS).collect())
}

fn parse_frontmatter(text: &str) -> BTreeMap<String, String> {
    let mut values = BTreeMap::new();
    let mut lines = text.lines();
    if lines.next().map(str::trim) != Some("---") {
        return values;
    }
    for line in lines {
        if line.trim() == "---" {
            break;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        values.insert(
            key.trim().to_ascii_lowercase(),
            value.trim().trim_matches(['"', '\'']).to_string(),
        );
    }
    values
}

fn strip_frontmatter(text: &str) -> &str {
    let Some(rest) = text.strip_prefix("---") else {
        return text;
    };
    let Some(end) = rest.find("\n---") else {
        return text;
    };
    rest[end + 4..].trim_start_matches(['\r', '\n'])
}

fn token_set(value: &str) -> BTreeSet<String> {
    value
        .split(|character: char| !character.is_alphanumeric())
        .map(str::trim)
        .filter(|token| token.chars().count() > 2)
        .map(str::to_string)
        .collect()
}

fn scope_label(scope: &SkillScope) -> &'static str {
    match scope {
        SkillScope::Global => "global",
        SkillScope::Project => "project",
        SkillScope::Compatibility => "compat",
    }
}

fn write_private_json<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create skill config directory: {error}"))?;
    }
    let text = serde_json::to_string_pretty(value)
        .map_err(|error| format!("failed to encode skill preferences: {error}"))?;
    write_private_file_atomically(path, text.as_bytes())
        .map_err(|error| format!("failed to write {}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEST_ROOT: AtomicU64 = AtomicU64::new(1);

    fn test_root() -> PathBuf {
        std::env::temp_dir().join(format!(
            "cindx-skills-test-{}-{}",
            std::process::id(),
            NEXT_TEST_ROOT.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[test]
    fn discovers_and_progressively_selects_project_skill() {
        let root = test_root();
        let skill_root = root.join("project/.cindx/skills/pdf");
        fs::create_dir_all(&skill_root).unwrap();
        fs::write(
            skill_root.join("SKILL.md"),
            "---\nname: PDF Review\ndescription: inspect PDF page layout\nallowed-tools: file.read\n---\n# PDF instructions\nRender every page.",
        )
        .unwrap();
        let catalog = SkillCatalog::load(
            root.join("global"),
            root.join("project"),
            root.join("preferences.json"),
        );
        let skills = catalog.list();
        let pdf = skills
            .iter()
            .find(|skill| skill.folder_name == "pdf")
            .expect("project skill should be discovered");
        assert!(pdf.enabled);
        assert!(pdf.trusted);
        let context = catalog
            .context_for_prompt("Please inspect this PDF layout")
            .unwrap()
            .unwrap();
        assert!(context.contains("Render every page"));
        assert!(!context.contains("allowed-tools"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn compatibility_skills_are_disabled_until_trusted() {
        let root = test_root();
        let skill_root = root.join("project/.agents/skills/external");
        fs::create_dir_all(&skill_root).unwrap();
        fs::write(skill_root.join("SKILL.md"), "# External").unwrap();
        let catalog = SkillCatalog::load(
            root.join("global"),
            root.join("project"),
            root.join("preferences.json"),
        );
        let external = catalog
            .list()
            .into_iter()
            .find(|skill| skill.folder_name == "external")
            .expect("compatibility skill should be discovered");
        assert!(!external.enabled);
        assert!(catalog.context_for_prompt("external").unwrap().is_none());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn includes_trusted_builtin_skill_creator() {
        let root = test_root();
        let catalog = SkillCatalog::load(
            root.join("global"),
            root.join("project"),
            root.join("preferences.json"),
        );
        let skill = catalog
            .list()
            .into_iter()
            .find(|skill| skill.id == BUILTIN_SKILL_CREATOR_ID)
            .expect("built-in skill creator should be present");
        assert!(skill.enabled);
        assert!(skill.trusted);
        assert!(read_skill_instructions(&skill)
            .unwrap()
            .contains("SKILL.md contract"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn skill_catalog_tools_allow_independent_read_execution() {
        let root = test_root();
        let catalog = SkillCatalog::load(
            root.join("global"),
            root.join("project"),
            root.join("preferences.json"),
        );

        let specs = catalog
            .tools()
            .into_iter()
            .map(|tool| tool.spec())
            .collect::<Vec<_>>();
        for name in ["skill.search", "skill.load"] {
            let spec = specs
                .iter()
                .find(|spec| spec.name == name)
                .expect("skill tool should be registered");
            assert_eq!(
                spec.execution_concurrency,
                ToolExecutionConcurrency::IndependentRead
            );
            assert!(spec.validate().is_ok());
        }

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn installs_skill_package_into_project_catalog() {
        let root = test_root();
        let mut archive = zip::ZipWriter::new(Cursor::new(Vec::new()));
        archive
            .start_file(
                "sample-skill/SKILL.md",
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
        archive
            .write_all(
                b"---\nname: sample-skill\ndescription: A sample skill for tests.\n---\nUse it.",
            )
            .unwrap();
        archive
            .start_file(
                "sample-skill/references/guide.md",
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
        archive.write_all(b"Reference").unwrap();
        let bytes = archive.finish().unwrap().into_inner();

        let skill_id = install_skill_archive(&root, &bytes).unwrap();

        assert_eq!(skill_id, "project:sample-skill");
        assert!(root.join("sample-skill/SKILL.md").is_file());
        assert!(root.join("sample-skill/references/guide.md").is_file());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_unsafe_skill_file_paths() {
        let root = test_root();
        let error = install_skill_files(
            &root,
            vec![SkillInstallFile {
                path: PathBuf::from("../SKILL.md"),
                bytes: b"---\nname: unsafe\ndescription: Unsafe.\n---".to_vec(),
            }],
        )
        .unwrap_err();

        assert!(error.contains("unsafe skill path"));
        assert!(!root.exists());
    }
}
