import { describe, expect, test } from "bun:test";

import { sanitizeAssistantMessageContent } from "../src/utils/messageTransport";

describe("sanitizeAssistantMessageContent", () => {
  test("strips DeepSeek DSML tool payloads and preserves surrounding prose", () => {
    const dsml = "\uFF5C\uFF5CDSML\uFF5C\uFF5C";
    const content = `Planning the next read.\n<${dsml}tool_calls>\n<${dsml}invoke name="read_file">\n<${dsml}parameter name="file" string="true">src/main.rs</${dsml}parameter>\n</${dsml}invoke>\n</${dsml}tool_calls>\nDone.`;

    const result = sanitizeAssistantMessageContent(content);

    expect(result).toContain("Planning the next read.");
    expect(result).toContain("Done.");
    expect(result).not.toContain("DSML");
    expect(result).not.toContain("src/main.rs");
  });

  test("drops unterminated DSML tool payloads instead of restoring the raw block", () => {
    const dsml = "\uFF5C\uFF5CDSML\uFF5C\uFF5C";
    const content = `<${dsml}tool_calls>\n<${dsml}invoke name="read_file">\n<${dsml}parameter name="file" string="true">src/main.rs</${dsml}parameter>\n<${dsml}`;

    expect(sanitizeAssistantMessageContent(content)).toBe("");
  });

  test("keeps ordinary assistant text unchanged", () => {
    expect(sanitizeAssistantMessageContent("Normal answer with no tool payloads.")).toBe(
      "Normal answer with no tool payloads.",
    );
  });
});
