import fs from "node:fs";
import path from "node:path";

const INTEGRATION_COMMAND_HANDLERS = [
  "get_sidecar_state",
  "save_sidecar_config",
  "get_web_search_config",
  "save_web_search_config",
  "get_mcp_state",
  "save_mcp_servers",
  "upsert_mcp_server",
  "update_mcp_server_policy",
  "remove_mcp_server",
  "refresh_mcp_server",
  "get_skill_state",
  "refresh_skills",
  "save_skill_preference",
  "install_skill_package",
  "install_skill_url",
];

const ROOT_GLOB_IMPORT_BUDGET = 74;
const PRODUCTION_SUPER_GLOB_MODULE_BUDGET = 56;

const LEGACY_DESKTOP_PRELUDE_GLOB_MODULES = new Set([
  "agent_loop_contract_runtime.rs",
  "agent_loop_runtime.rs",
  "agent_recovery_service.rs",
  "app_state.rs",
  "knowledge_embedding_runtime.rs",
  "knowledge_generation_runtime.rs",
  "knowledge_runtime.rs",
  "lib.rs",
  "memory_measurement_runtime.rs",
  "memory_runtime.rs",
  "memory_vector_generation_runtime.rs",
  "prompt_workflow_execution.rs",
  "session_context_service.rs",
]);

const listRustSourceFiles = (sourceDirectory) =>
  fs
    .readdirSync(sourceDirectory, { withFileTypes: true })
    .sort((left, right) => left.name.localeCompare(right.name))
    .flatMap((entry) => {
      const entryPath = path.join(sourceDirectory, entry.name);
      if (entry.isDirectory()) return listRustSourceFiles(entryPath);
      return entry.name.endsWith(".rs") ? [entryPath] : [];
    });

const uniqueSortedNames = (names) => [...new Set(names)].sort();

const capturedNames = (source, pattern) =>
  [...source.matchAll(pattern)].map((match) => match[1]);

const sameNames = (expected, actual) => {
  const expectedNames = new Set(expected);
  const actualNames = new Set(actual);
  return (
    expected.length === actual.length &&
    expected.every((name) => actualNames.has(name)) &&
    actual.every((name) => expectedNames.has(name))
  );
};

export const rustCodeWithoutCommentsAndLiterals = (source) => {
  let output = "";
  let index = 0;
  let blockCommentDepth = 0;
  let quoted = false;
  let escaped = false;
  let rawStringEnd = null;

  while (index < source.length) {
    if (blockCommentDepth > 0) {
      if (source.startsWith("/*", index)) {
        blockCommentDepth += 1;
        output += "  ";
        index += 2;
      } else if (source.startsWith("*/", index)) {
        blockCommentDepth -= 1;
        output += "  ";
        index += 2;
      } else {
        output += source[index] === "\n" ? "\n" : " ";
        index += 1;
      }
      continue;
    }

    if (rawStringEnd !== null) {
      if (source.startsWith(rawStringEnd, index)) {
        output += " ".repeat(rawStringEnd.length);
        index += rawStringEnd.length;
        rawStringEnd = null;
      } else {
        output += source[index] === "\n" ? "\n" : " ";
        index += 1;
      }
      continue;
    }

    if (quoted) {
      const character = source[index];
      output += character === "\n" ? "\n" : " ";
      index += 1;
      if (escaped) {
        escaped = false;
      } else if (character === "\\") {
        escaped = true;
      } else if (character === '"') {
        quoted = false;
      }
      continue;
    }

    if (source.startsWith("//", index)) {
      while (index < source.length && source[index] !== "\n") {
        output += " ";
        index += 1;
      }
      continue;
    }
    if (source.startsWith("/*", index)) {
      blockCommentDepth = 1;
      output += "  ";
      index += 2;
      continue;
    }

    const rawStringMatch = source
      .slice(index)
      .match(/^(?:b|c)?r(#{0,255})"/);
    if (rawStringMatch) {
      output += " ".repeat(rawStringMatch[0].length);
      index += rawStringMatch[0].length;
      rawStringEnd = `"${rawStringMatch[1]}`;
      continue;
    }
    if (source[index] === '"') {
      output += " ";
      index += 1;
      quoted = true;
      continue;
    }

    const characterLiteral = source
      .slice(index)
      .match(/^'(?:\\(?:.|x[0-9A-Fa-f]{2}|u\{[0-9A-Fa-f_]+\})|[^\\'\r\n])'/);
    if (characterLiteral) {
      output += " ".repeat(characterLiteral[0].length);
      index += characterLiteral[0].length;
      continue;
    }

    output += source[index];
    index += 1;
  }

  return output;
};

const rustUseStatements = (source) =>
  rustCodeWithoutCommentsAndLiterals(source).match(/\buse\s+[^;]*;/gs) ?? [];

const rustGlobImports = (source) =>
  rustUseStatements(source).filter((statement) => statement.includes("*"));

const rustPathOverrideAttributes = (source) =>
  rustCodeWithoutCommentsAndLiterals(source).match(
    /#\s*\[[^\]]*\bpath\s*=/gs
  ) ?? [];

const normalizedRustUseStatement = (statement) =>
  statement
    .replace(/\s+/g, " ")
    .replace(/\s*([{},;])\s*/g, "$1")
    .trim();

const tauriHandlerPaths = (source) =>
  uniqueSortedNames(
    capturedNames(
      rustCodeWithoutCommentsAndLiterals(source),
      /tauri::generate_handler!\s*\[([\s\S]*?)\]/g
    ).flatMap((block) =>
      block
        .split(",")
        .map((entry) => entry.trim())
        .filter(Boolean)
    )
  );

export const inspectDesktopIntegrationBoundary = (root) => {
  const sourceDirectory = path.join(root, "apps/desktop/src-tauri/src");
  const read = (relativePath) =>
    fs.readFileSync(path.join(root, relativePath), "utf8");
  const rustFiles = listRustSourceFiles(sourceDirectory).map((file) => ({
    entry: path.relative(sourceDirectory, file),
    source: fs.readFileSync(file, "utf8"),
  }));
  const compositionRoot = read("apps/desktop/src-tauri/src/lib.rs");
  const appBootstrap = read("apps/desktop/src-tauri/src/app_bootstrap.rs");
  const integrationCommands = read(
    "apps/desktop/src-tauri/src/integration_commands.rs"
  );
  const desktopPrelude = read("apps/desktop/src-tauri/src/desktop_prelude.rs");
  const appState = read("apps/desktop/src-tauri/src/app_state.rs");
  const runLifecycle = read("crates/agent-application/src/run_lifecycle.rs");
  const desktopCargo = read("apps/desktop/src-tauri/Cargo.toml");
  const workspaceCargo = read("Cargo.toml");
  const workspaceDefaultMembers =
    workspaceCargo.match(/default-members\s*=\s*\[([\s\S]*?)\]/)?.[1] ?? "";
  const harnessCargo = read("crates/agent-harness/Cargo.toml");
  const harnessSource = read("crates/agent-harness/src/lib.rs");

  const integrationFiles = rustFiles.filter(
    ({ entry }) =>
      entry === "integration_commands.rs" ||
      entry.startsWith(`integration_commands${path.sep}`)
  );
  const integrationGlobs = integrationFiles.flatMap(({ entry, source }) =>
    rustGlobImports(source).map((statement) => `${entry}: ${statement}`)
  );
  const integrationSourceOverrides = integrationFiles.filter(
    ({ source }) =>
      rustPathOverrideAttributes(source).length > 0 ||
      /\binclude\s*!/.test(rustCodeWithoutCommentsAndLiterals(source))
  );
  const rootIntegrationImports = rustUseStatements(compositionRoot).filter(
    (statement) => /\bintegration_commands\b/.test(statement)
  );
  const appIntegrationImports = rustUseStatements(appBootstrap)
    .filter((statement) => /\bintegration_commands\b/.test(statement))
    .map(normalizedRustUseStatement);
  const rootGlobImports = rustGlobImports(compositionRoot);
  const productionSuperGlobModules = rustFiles
    .filter(
      ({ entry }) =>
        entry !== "tests.rs" &&
        !/(?:^|[\\/])[^\\/]*_tests(?:\.rs|[\\/])/.test(entry)
    )
    .filter(({ source }) =>
      /^use\s+super\s*::\s*\*\s*;/m.test(rustCodeWithoutCommentsAndLiterals(source))
    );

  const integrationDefinitions = uniqueSortedNames(
    capturedNames(
      rustCodeWithoutCommentsAndLiterals(integrationCommands),
      /#\[tauri::command(?:\([^\]]*\))?\]\s*(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?fn\s+([A-Za-z_][A-Za-z0-9_]*)/g
    )
  );
  const registeredIntegrationHandlers = tauriHandlerPaths(appBootstrap)
    .filter((entry) => entry.startsWith("integration_commands::"))
    .map((entry) => entry.split("::").at(-1));

  const desktopPreludeUseStatements = rustUseStatements(desktopPrelude);
  const desktopPreludeMcpImports = desktopPreludeUseStatements
    .filter((statement) => /\bagent_mcp\b/.test(statement))
    .map(normalizedRustUseStatement);
  const desktopPreludeSkillImports = desktopPreludeUseStatements
    .filter((statement) => /\bagent_skills\b/.test(statement))
    .map(normalizedRustUseStatement);
  const desktopPreludeCode = rustCodeWithoutCommentsAndLiterals(desktopPrelude);
  const appStateCode = rustCodeWithoutCommentsAndLiterals(appState);
  const runLifecycleCode = rustCodeWithoutCommentsAndLiterals(runLifecycle);
  const productionDesktopCode = rustFiles
    .filter(
      ({ entry }) =>
        entry !== "tests.rs" &&
        !/(?:^|[\\/])[^\\/]*_tests(?:\.rs|[\\/])/.test(entry)
    )
    .map(({ source }) => rustCodeWithoutCommentsAndLiterals(source))
    .join("\n");
  const unexpectedPreludeGlobs = rustFiles
    .filter(({ source }) =>
      rustGlobImports(source).some((statement) =>
        /\bdesktop_prelude\b/.test(statement)
      )
    )
    .filter(({ entry }) => !LEGACY_DESKTOP_PRELUDE_GLOB_MODULES.has(entry));

  const parserContractValid =
    rustGlobImports("use crate::desktop_prelude::{self, *};").length === 1 &&
    rustGlobImports("use crate::{desktop_prelude::{*}};").length === 1 &&
    rustGlobImports('const SAMPLE: &str = "use crate::*;";').length === 0 &&
    rustPathOverrideAttributes(
      '#[cfg_attr(all(), path = "../helper.rs")] mod helper;'
    ).length === 1 &&
    rustPathOverrideAttributes('let path = "../helper.rs";').length === 0 &&
    sameNames(
      tauriHandlerPaths(
        "tauri::generate_handler![/* integration_commands::fake, */ integration_commands::real,]"
      ),
      ["integration_commands::real"]
    );

  const failures = [
    !parserContractValid && "parser_contract",
    integrationFiles.length === 0 && "missing_domain",
    integrationGlobs.length > 0 && "domain_glob",
    integrationSourceOverrides.length > 0 && "domain_source_override",
    rootIntegrationImports.length > 0 && "root_reexport",
    !sameNames(appIntegrationImports, ["use crate::integration_commands;"]) &&
      "implicit_consumer",
    rustGlobImports(desktopPrelude).length > 0 && "prelude_glob",
    !sameNames(desktopPreludeMcpImports, [
      "use agent_mcp::{McpCatalogService,McpServerConfig};",
    ]) && "prelude_mcp_tree",
    !sameNames(desktopPreludeSkillImports, [
      "use agent_skills::{SkillCatalog,SkillRecord,};",
    ]) && "prelude_skills_tree",
    (desktopPreludeCode.match(/\bagent_mcp\b/g) ?? []).length !== 1 &&
      "prelude_mcp_alias",
    (desktopPreludeCode.match(/\bagent_skills\b/g) ?? []).length !== 1 &&
      "prelude_skills_alias",
    /\b(?:McpTransportConfig|install_skill_archive|SkillPreference)\b/.test(
      desktopPreludeCode
    ) && "prelude_legacy_symbol",
    !/\bagent-harness\s*=\s*\{/.test(desktopCargo) &&
      "desktop_harness_dependency",
    !/"crates\/agent-harness"/.test(workspaceCargo) &&
      "workspace_harness_member",
    /\btauri\b/.test(harnessCargo) && "harness_tauri_dependency",
    /\borchestrator-eval\b/.test(desktopCargo) &&
      "desktop_research_dependency",
    /\borchestrator-eval\b/.test(workspaceDefaultMembers) &&
      "research_default_build_member",
    !/\bpub struct RunRegistry\b/.test(harnessSource) &&
      "missing_run_registry",
    !/\bpub struct ExclusiveKeyRegistry\b/.test(harnessSource) &&
      "missing_exclusive_key_registry",
    !/\bagent_run_controls\s*:\s*RunRegistry\b/.test(appStateCode) &&
      "app_state_agent_run_registry",
    !/\bprompt_evaluation_controls\s*:\s*RunRegistry\b/.test(appStateCode) &&
      "app_state_prompt_run_registry",
    !/\bqueue_dispatching_sessions\s*:\s*ExclusiveKeyRegistry\b/.test(
      appStateCode
    ) && "app_state_queue_registry",
    !/\bsession_title_refinement_sessions\s*:\s*ExclusiveKeyRegistry\b/.test(
      appStateCode
    ) && "app_state_title_registry",
    !/\bsuspended_agent_runs\s*:\s*SuspendedRunStore\b/.test(appStateCode) &&
      "app_state_suspended_store",
    !/\bsession_output_cache\s*:\s*SessionOutputCache\b/.test(appStateCode) &&
      "app_state_output_cache",
    /\bstruct\s+(?:RegisteredRunControl|ExclusiveKeyLease)\b/.test(
      runLifecycleCode
    ) && "desktop_owned_harness_guard",
    /\b(?:agent_run_controls|prompt_evaluation_controls|queue_dispatching_sessions|session_title_refinement_sessions|suspended_agent_runs|session_output_cache)\s*\.\s*lock\s*\(/.test(
      productionDesktopCode
    ) && "runtime_registry_lock_escape",
    !sameNames(INTEGRATION_COMMAND_HANDLERS, integrationDefinitions) &&
      "command_definitions",
    !sameNames(INTEGRATION_COMMAND_HANDLERS, registeredIntegrationHandlers) &&
      "command_registration",
    unexpectedPreludeGlobs.length > 0 && "new_prelude_consumer",
    rootGlobImports.length > ROOT_GLOB_IMPORT_BUDGET && "root_glob_budget",
    productionSuperGlobModules.length > PRODUCTION_SUPER_GLOB_MODULE_BUDGET &&
      "production_super_glob_budget",
  ].filter(Boolean);

  return {
    ok: failures.length === 0,
    message: `Desktop integration boundary regressed: ${failures.join(",") || "none"}; integration_globs=${integrationGlobs.length}, source_overrides=${integrationSourceOverrides.length}, root_imports=${rootIntegrationImports.length}, definitions=${integrationDefinitions.length}, registered=${registeredIntegrationHandlers.length}, unexpected_prelude_globs=${unexpectedPreludeGlobs
      .map(({ entry }) => entry)
      .join(",")}; root_globs=${rootGlobImports.length}/${ROOT_GLOB_IMPORT_BUDGET}, production_super_globs=${productionSuperGlobModules.length}/${PRODUCTION_SUPER_GLOB_MODULE_BUDGET}`,
  };
};
