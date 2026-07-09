import type { AgentSafetyModel, PatchRiskLevel } from "../stores/settings";

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
    reasons.push("未设置当前群工作文件夹，不能确认补丁边界。");
  }

  if (files.length === 0) {
    warnings.push("未识别具体文件，应用前需要人工确认目标文件。");
  }

  for (const file of files) {
    if (isAbsolutePath(file)) {
      reasons.push(`补丁包含绝对路径：${file}`);
    }

    if (forbiddenPathPatterns.some((pattern) => pattern.test(file))) {
      reasons.push(`补丁目标路径不在常规源码边界内：${file}`);
    }

    if (secretPatterns.some((pattern) => pattern.test(file))) {
      reasons.push(`补丁可能触及敏感文件或密钥：${file}`);
    }
  }

  if (secretPatterns.some((pattern) => pattern.test(searchable))) {
    warnings.push("内容疑似涉及密钥、令牌或密码。");
  }

  if (destructivePatterns.some((pattern) => pattern.test(searchable))) {
    reasons.push("内容包含删除、数据库破坏或高危权限命令。");
  }

  if (input.riskLevel === "high") {
    warnings.push("风险等级为 high，需要人工复核。");
  }

  if (input.safetyModel === "strict" && input.riskLevel !== "low") {
    reasons.push("Strict 安全模型要求中高风险补丁先阻止自动批准。");
  }

  if (input.safetyModel === "security-analyzer" && warnings.length > 0) {
    reasons.push("Security Analyzer 模式要求先处理全部安全警告。");
  }

  if (input.safetyModel === "sandbox-yolo" && input.riskLevel === "high") {
    reasons.push("Sandbox YOLO 也不允许自动推进高风险补丁。");
  }

  if (reasons.length > 0) {
    return { verdict: "blocked", reasons, warnings };
  }

  if (warnings.length > 0 || input.riskLevel === "medium") {
    return { verdict: "needs-confirmation", reasons, warnings };
  }

  return {
    verdict: "allow",
    reasons: ["补丁通过工作区边界和基础风险检查。"],
    warnings,
  };
}
