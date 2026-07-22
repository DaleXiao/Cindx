export type MarkdownDiagram = {
  kind: "mermaid" | "mindmap";
  source: string;
};

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

  if (/^mindmap(?:\s|$)/i.test(source)) {
    return { kind: "mindmap", source };
  }
  return {
    kind: "mindmap",
    source: `mindmap\n${source
      .split("\n")
      .map((line) => `  ${line}`)
      .join("\n")}`
  };
}
