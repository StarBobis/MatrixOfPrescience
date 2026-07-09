import type { AgentSafetyModel, PatchRiskLevel } from "../stores/settings";
import { translate as t } from "../i18n";

export type PatchSafetyVerdict = "allow" | "needs-confirmation" | "blocked";

export interface PatchSafetyCheck {
  verdict: PatchSafetyVerdict;
  reasons: string[];
  warnings: string[];
}

export interface PatchSafetyInput {
  workspacePath: string;
  files: string[];
  content: string;
  patchText: string;
  riskLevel: PatchRiskLevel;
  safetyModel: AgentSafetyModel;
}

const forbiddenPathPatterns = [
  /(^|[/\\])\.\.([/\\]|$)/,
  /(^|[/\\])\.ssh([/\\]|$)/i,
  /(^|[/\\])\.aws([/\\]|$)/i,
  /(^|[/\\])\.config([/\\]|$)/i,
  /(^|[/\\])node_modules([/\\]|$)/i,
  /(^|[/\\])dist([/\\]|$)/i,
  /(^|[/\\])target([/\\]|$)/i,
];

const secretPatterns = [
  /api[_-]?key/i,
  /secret/i,
  /token/i,
  /password/i,
  /private[_-]?key/i,
  /\.env(\.|$)/i,
];

const destructivePatterns = [
  /remove-item/i,
  /rm\s+-rf/i,
  /delete\s+from/i,
  /drop\s+table/i,
  /truncate\s+table/i,
  /chmod\s+777/i,
  /format\s+[a-z]:/i,
];

function normalizeFilePath(file: string) {
  return file.trim().replace(/\\/g, "/");
}

function isAbsolutePath(file: string) {
  return /^[a-z]:\//i.test(file) || file.startsWith("/");
}

export function evaluatePatchSafety(input: PatchSafetyInput): PatchSafetyCheck {
  const reasons: string[] = [];
  const warnings: string[] = [];
  const workspacePath = input.workspacePath.trim();
  const files = input.files.map(normalizeFilePath).filter(Boolean);
  const searchable = `${input.content}\n${input.patchText}\n${files.join("\n")}`;

  if (!workspacePath) {
    reasons.push(t("patchSafety.noWorkspace"));
  }

  if (files.length === 0) {
    warnings.push(t("patchSafety.noFiles"));
  }

  for (const file of files) {
    if (isAbsolutePath(file)) {
      reasons.push(t("patchSafety.absolutePath", { file }));
    }

    if (forbiddenPathPatterns.some((pattern) => pattern.test(file))) {
      reasons.push(t("patchSafety.forbiddenPath", { file }));
    }

    if (secretPatterns.some((pattern) => pattern.test(file))) {
      reasons.push(t("patchSafety.secretPath", { file }));
    }
  }

  if (secretPatterns.some((pattern) => pattern.test(searchable))) {
    warnings.push(t("patchSafety.secretContent"));
  }

  if (destructivePatterns.some((pattern) => pattern.test(searchable))) {
    reasons.push(t("patchSafety.destructiveContent"));
  }

  if (input.riskLevel === "high") {
    warnings.push(t("patchSafety.highRisk"));
  }

  if (input.safetyModel === "strict" && input.riskLevel !== "low") {
    reasons.push(t("patchSafety.strictBlocks"));
  }

  if (input.safetyModel === "security-analyzer" && warnings.length > 0) {
    reasons.push(t("patchSafety.analyzerBlocks"));
  }

  if (input.safetyModel === "sandbox-yolo" && input.riskLevel === "high") {
    reasons.push(t("patchSafety.sandboxYoloBlocks"));
  }

  if (reasons.length > 0) {
    return { verdict: "blocked", reasons, warnings };
  }

  if (warnings.length > 0 || input.riskLevel === "medium") {
    return { verdict: "needs-confirmation", reasons, warnings };
  }

  return {
    verdict: "allow",
    reasons: [t("patchSafety.allow")],
    warnings,
  };
}
