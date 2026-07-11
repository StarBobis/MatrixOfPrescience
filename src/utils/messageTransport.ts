const internalStatusPrefixPattern =
  /^[^\n:[\]]{1,80}\s\[status=(?:done|thinking|error|interrupted)\]:\s*/u;
const internalExecutionHistoryPattern =
  /\n\nExecution history visible to future turns:\n(?:- \[(?:status|tool|reasoning|network)\/(?:running|done|error|info|interrupted)\][^\n]*(?:\n|$))+/u;

export function sanitizeAssistantMessageContent(content: string): string {
  const original = content.trim();

  if (!original) {
    return "";
  }

  let cleaned = original;

  for (let iteration = 0; iteration < 6; iteration += 1) {
    const next = cleaned
      .replace(internalExecutionHistoryPattern, "")
      .replace(internalStatusPrefixPattern, "")
      .trim();

    if (next === cleaned) {
      break;
    }

    cleaned = next;
  }

  return cleaned || original;
}
