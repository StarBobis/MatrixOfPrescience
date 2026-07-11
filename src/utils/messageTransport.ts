const internalStatusPrefixPattern =
  /^[^\n:[\]]{1,80}\s\[status=(?:done|thinking|error|interrupted)\]:\s*/u;
const internalExecutionHistoryPattern =
  /\n\nExecution history visible to future turns:\n(?:- \[(?:status|tool|reasoning|network)\/(?:running|done|error|info|interrupted)\][^\n]*(?:\n|$))+/u;
const dsmlTagPrefixPattern = String.raw`(?:DSML|\uFF5C\uFF5CDSML\uFF5C\uFF5C)`;
const dsmlTagSeparatorPattern = String.raw`(?:\s*[|\uFF5C]\s*)?`;
const dsmlToolCallsOpenPattern = new RegExp(
  String.raw`<${dsmlTagPrefixPattern}${dsmlTagSeparatorPattern}tool_calls>`,
  "iu",
);
const dsmlToolCallsClosePattern = new RegExp(
  String.raw`</${dsmlTagPrefixPattern}${dsmlTagSeparatorPattern}tool_calls>`,
  "iu",
);
const dsmlInvokeOpenPattern = new RegExp(
  String.raw`<${dsmlTagPrefixPattern}${dsmlTagSeparatorPattern}invoke\b[^>]*>`,
  "iu",
);
const dsmlInvokeClosePattern = new RegExp(
  String.raw`</${dsmlTagPrefixPattern}${dsmlTagSeparatorPattern}invoke>`,
  "iu",
);

function stripDsmlBlockRanges(content: string, openPattern: RegExp, closePattern: RegExp) {
  let cleaned = content;

  for (let iteration = 0; iteration < 12; iteration += 1) {
    const open = openPattern.exec(cleaned);

    if (!open || open.index < 0) {
      break;
    }

    const start = open.index;
    const searchFrom = start + open[0].length;
    const tail = cleaned.slice(searchFrom);
    const close = closePattern.exec(tail);
    const end = close ? searchFrom + close.index + close[0].length : cleaned.length;

    cleaned = `${cleaned.slice(0, start)}${cleaned.slice(end)}`;
  }

  return cleaned;
}

function stripDsmlToolPayloads(content: string) {
  const withoutWrappers = stripDsmlBlockRanges(
    content,
    dsmlToolCallsOpenPattern,
    dsmlToolCallsClosePattern,
  );

  return stripDsmlBlockRanges(withoutWrappers, dsmlInvokeOpenPattern, dsmlInvokeClosePattern);
}

export function sanitizeAssistantMessageContent(content: string): string {
  const original = content.trim();

  if (!original) {
    return "";
  }

  let cleaned = original;
  let changed = false;

  for (let iteration = 0; iteration < 6; iteration += 1) {
    const next = cleaned
      .replace(internalExecutionHistoryPattern, "")
      .replace(internalStatusPrefixPattern, "");
    const stripped = stripDsmlToolPayloads(next).trim();

    if (stripped !== cleaned) {
      changed = true;
    }

    if (stripped === cleaned) {
      break;
    }

    cleaned = stripped;
  }

  return changed ? cleaned : original;
}
