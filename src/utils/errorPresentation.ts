export interface ErrorPresentation {
  summary: string;
  detail: string;
}

const summaryKeys = ["message", "error", "detail", "details", "reason"] as const;

function preferredSummary(value: unknown, depth = 0): string {
  if (!value || typeof value !== "object" || depth > 2) {
    return "";
  }

  const record = value as Record<string, unknown>;

  for (const key of summaryKeys) {
    const candidate = record[key];

    if (typeof candidate === "string" && candidate.trim()) {
      return candidate.trim();
    }
  }

  for (const key of summaryKeys) {
    const nested = preferredSummary(record[key], depth + 1);

    if (nested) {
      return nested;
    }
  }

  return "";
}

function stringifyErrorObject(value: object): string {
  const seen = new WeakSet<object>();

  try {
    return JSON.stringify(
      value,
      (_key, nested) => {
        if (typeof nested === "bigint") {
          return nested.toString();
        }

        if (nested && typeof nested === "object") {
          if (seen.has(nested)) {
            return "[Circular]";
          }
          seen.add(nested);
        }

        return nested;
      },
      2,
    );
  } catch {
    return "";
  }
}

export function describeError(error: unknown, fallback: string): ErrorPresentation {
  const normalizedFallback = fallback.trim() || "Unknown error";

  if (typeof error === "string") {
    const text = error.trim() || normalizedFallback;
    return { summary: text, detail: text };
  }

  if (error instanceof Error) {
    const summary = error.message.trim() || normalizedFallback;
    const detail = error.stack?.trim() || summary;
    return { summary, detail };
  }

  if (error && typeof error === "object") {
    const detail = stringifyErrorObject(error);
    const summary = preferredSummary(error) || detail || normalizedFallback;
    return { summary, detail: detail || summary };
  }

  if (error !== null && error !== undefined) {
    const text = String(error).trim() || normalizedFallback;
    return { summary: text, detail: text };
  }

  return { summary: normalizedFallback, detail: normalizedFallback };
}
