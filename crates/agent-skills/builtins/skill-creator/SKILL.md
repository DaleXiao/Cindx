---
name: Claude Code Skill Creator
description: Create and improve Claude Code compatible skills with focused instructions, resources, and validation.
allowed-tools: file.read file.list file.search file.patch file.write shell.run
---
# Claude Code Skill Creator

Create reusable skills that are easy for an agent to discover and safe to apply.

## Workflow

1. Clarify the skill's triggering requests and the concrete outcome it must produce.
2. Inspect existing project conventions before creating files.
3. Create one skill folder with `SKILL.md` as its entry point.
4. Keep the main instructions concise. Put detailed reference material in `references/`, deterministic helpers in `scripts/`, and reusable inputs in `assets/`.
5. Validate the skill against at least one request that should trigger it and one nearby request that should not.
6. Report the files created, validation performed, and any requirements that remain external.

## SKILL.md contract

Start with YAML frontmatter containing:

- `name`: a clear human-readable title.
- `description`: what the skill does and when it should be selected.
- `allowed-tools`: only the tools required by the workflow.

Write the body as direct operational guidance. Prefer ordered steps, explicit inputs and outputs, and verifiable completion criteria. Avoid generic advice, duplicated background, and instructions that weaken permission or security boundaries.

## Resource rules

- Use `references/` for material the agent should read only when needed.
- Use `scripts/` for repeatable operations where deterministic execution is better than re-generating logic.
- Use `assets/` for templates or files copied into outputs.
- Reference every bundled resource from `SKILL.md`; do not leave unexplained files.
- Keep scripts non-interactive and make failures explicit.

## Validation

Check that frontmatter parses, referenced paths exist, instructions fit the intended agent, and the workflow produces its promised artifact. Do not claim a skill is complete when its required scripts or references were not exercised.
