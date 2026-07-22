export type MarkdownDiagram = {
  kind: "mermaid" | "mindmap";
  source: string;
};

function hasMarkdownHierarchy(source: string) {
  return source.split("\n").some((line) =>
    /^(?: {0,3}#{1,6}\s+|\s*(?:[-+*]|\d+[.)])\s+)/.test(line)
  );
}

function legacyMindmapLabel(value: string) {
  const label = value.trim();
  const shapedLabel =
    label.match(/^[\w.-]+\(\((.+)\)\)$/)?.[1] ??
    label.match(/^[\w.-]+\(\[(.+)\]\)$/)?.[1] ??
    label.match(/^[\w.-]+\[\[(.+)\]\]$/)?.[1] ??
    label.match(/^[\w.-]+\[(.+)\]$/)?.[1] ??
    label.match(/^[\w.-]+\((.+)\)$/)?.[1] ??
    label.match(/^[\w.-]+\{\{(.+)\}\}$/)?.[1] ??
    label.match(/^[\w.-]+\{(.+)\}$/)?.[1] ??
    label;
  return shapedLabel.replace(/^(["'])(.*)\1$/, "$2").trim();
}

function normalizeMindmapSource(source: string) {
  const lines = source.split("\n");
  if (/^mindmap\s*$/i.test(lines[0]?.trim() ?? "")) lines.shift();
  while (lines[0]?.trim() === "") lines.shift();
  while (lines[lines.length - 1]?.trim() === "") lines.pop();
  const content = lines.join("\n").trimEnd();
  if (!content || hasMarkdownHierarchy(content)) return content;

  const entries = content
    .split("\n")
    .filter((line) => line.trim())
    .map((line) => ({
      indent: line.match(/^\s*/)?.[0].replace(/\t/g, "  ").length ?? 0,
      label: legacyMindmapLabel(line)
    }));
  if (!entries.length) return content;

  const baseIndent = entries[0].indent;
  const indentSteps = entries
    .map((entry) => entry.indent - baseIndent)
    .filter((indent) => indent > 0);
  const indentStep = indentSteps.length ? Math.min(...indentSteps) : 2;
  return entries
    .map((entry, index) => {
      if (index === 0) return `# ${entry.label}`;
      const depth = Math.max(1, Math.round((entry.indent - baseIndent) / indentStep));
      return `${"  ".repeat(depth - 1)}- ${entry.label}`;
    })
    .join("\n");
}

export function markdownDiagramForCode(
  language: string,
  code: string
): MarkdownDiagram | null {
  const normalizedLanguage = language.trim().toLowerCase();
  const source = code.trim();
  if (!source) return null;

  if (normalizedLanguage === "mermaid") {
    return { kind: "mermaid", source };
  }
  if (normalizedLanguage !== "mindmap") return null;
  const mindmapSource = normalizeMindmapSource(source);
  return mindmapSource ? { kind: "mindmap", source: mindmapSource } : null;
}
