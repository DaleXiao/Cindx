/**
 * Parses the drafted plan markdown into a structured visual document. The
 * drafting prompt asks for a fixed shape (Objective / numbered Steps with
 * optional files notes / Verification); anything that does not parse into at
 * least one numbered step falls back to raw markdown rendering, so older or
 * freer plans keep working.
 */

export type PlanStep = {
  index: number;
  title: string;
  detail: string | null;
  files: string[];
};

export type PlanDocument = {
  objective: string | null;
  steps: PlanStep[];
  verification: string | null;
  /** True when at least one numbered step parsed; the card renders the visual layout only then. */
  structured: boolean;
};

const STEP_LINE = /^\s*(\d+)[.)]\s+(.*)$/;
const FILES_NOTE = /[(（]?\s*(?:files?|文件)\s*[:：]\s*([^)）\n]+)[)）]?/i;

function splitTitleAndDetail(text: string): { title: string; detail: string | null } {
  const match = text.split(/\s+[-—]\s+/);
  if (match.length > 1) {
    return { title: match[0].trim(), detail: match.slice(1).join(" - ").trim() };
  }
  return { title: text.trim(), detail: null };
}

export function parsePlanDocument(markdown: string): PlanDocument {
  let objective: string | null = null;
  let verification: string | null = null;
  const steps: PlanStep[] = [];

  for (const rawLine of markdown.split("\n")) {
    const line = rawLine.trim();
    if (!line) continue;
    const objectiveMatch = line.match(/^(?:objective|目标)\s*[:：]\s*(.+)$/i);
    if (objectiveMatch) {
      objective = objectiveMatch[1].trim();
      continue;
    }
    const verificationMatch = line.match(/^(?:verification|验证)\s*[:：]\s*(.+)$/i);
    if (verificationMatch) {
      verification = verificationMatch[1].trim();
      continue;
    }
    if (/^(?:steps|步骤)\s*[:：]?$/i.test(line)) continue;
    const stepMatch = line.match(STEP_LINE);
    if (stepMatch) {
      let body = stepMatch[2];
      const filesMatch = body.match(FILES_NOTE);
      const files = filesMatch
        ? filesMatch[1]
            .split(/[,，、]/)
            .map((file) => file.trim())
            .filter((file) => file.length > 0)
        : [];
      if (filesMatch) body = body.replace(filesMatch[0], "").trim();
      const { title, detail } = splitTitleAndDetail(body);
      steps.push({ index: Number(stepMatch[1]), title, detail, files });
    }
  }

  return { objective, steps, verification, structured: steps.length > 0 };
}
