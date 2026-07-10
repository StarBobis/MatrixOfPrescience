import { describe, expect, test } from "bun:test";

import { describeError } from "../src/utils/errorPresentation";

describe("describeError", () => {
  test("preserves string errors", () => {
    expect(describeError("HTTP 429: rate limited", "Unknown error")).toEqual({
      summary: "HTTP 429: rate limited",
      detail: "HTTP 429: rate limited",
    });
  });

  test("uses an Error message as the summary and keeps diagnostic detail", () => {
    const result = describeError(new Error("Connection refused"), "Unknown error");

    expect(result.summary).toBe("Connection refused");
    expect(result.detail).toContain("Connection refused");
  });

  test("shows structured Tauri errors instead of object coercion", () => {
    const result = describeError(
      {
        code: "HTTP_400",
        message: "Invalid model request",
        response: { type: "invalid_request_error", param: "tools" },
      },
      "Unknown error",
    );

    expect(result.summary).toBe("Invalid model request");
    expect(result.detail).toContain('"code": "HTTP_400"');
    expect(result.detail).toContain('"param": "tools"');
    expect(result.detail).not.toContain("[object Object]");
  });

  test("uses the localized fallback for empty values", () => {
    expect(describeError(null, "未知错误")).toEqual({
      summary: "未知错误",
      detail: "未知错误",
    });
  });
});
