export const CUSTOM_COMMAND_ARGUMENT_TOKEN = "$ARGUMENTS";

export function customCommandRequiresArguments(template: string): boolean {
  return template.includes(CUSTOM_COMMAND_ARGUMENT_TOKEN);
}

export function applyCustomCommandTemplate(template: string, currentText: string): string {
  const trimmed = currentText.trim();
  if (customCommandRequiresArguments(template)) {
    return template.split(CUSTOM_COMMAND_ARGUMENT_TOKEN).join(trimmed);
  }
  if (trimmed.length === 0) {
    return template;
  }
  return `${template}\n\n${trimmed}`;
}
