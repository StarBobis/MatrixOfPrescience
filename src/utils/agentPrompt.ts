import type { AgentModel, ChatGroup } from "../stores/settings";
import { translate as t } from "../i18n";

export function buildSystemPrompt(
  member: AgentModel,
  group: ChatGroup | undefined,
  extraRules = "",
  codeContext = "",
) {
  const config = group?.agentConfig;
  const workspacePath = group?.workspacePath?.trim();

  return [
    group?.announcement.trim(),
    t("agentPrompt.collaborationRule"),
    workspacePath
      ? t("agentPrompt.workspaceSet", { path: workspacePath })
      : t("agentPrompt.workspaceMissing"),
    config
      ? [
          t("agentPrompt.localModeTitle"),
          t("agentPrompt.modeAgent", { value: config.agentMode }),
          t("agentPrompt.modeWorkflow", { value: config.workflowMode }),
          t("agentPrompt.modeApproval", { value: config.approvalMode }),
          t("agentPrompt.modeSafety", { value: config.safetyModel }),
          t("agentPrompt.modeEditBeforeAsk", {
            value: config.editBeforeAsk ? t("agentPrompt.on") : t("agentPrompt.off"),
          }),
          t("agentPrompt.modeYolo", {
            value: config.yoloMode ? t("agentPrompt.on") : t("agentPrompt.off"),
          }),
        ].join("\n")
      : "",
    buildSafetyPolicy(group),
    t("agentPrompt.coreRole"),
    member.systemPrompt.trim(),
    codeContext
      ? [
          t("agentPrompt.codeContextTitle"),
          t("agentPrompt.codeContextGuide"),
          codeContext,
        ].join("\n")
      : "",
    extraRules,
  ]
    .filter(Boolean)
    .join("\n\n");
}

function buildSafetyPolicy(group: ChatGroup | undefined) {
  const config = group?.agentConfig;

  if (!config) {
    return "";
  }

  const commonRules = [
    t("agentPrompt.safety.commonTitle"),
    t("agentPrompt.safety.ruleTool"),
    t("agentPrompt.safety.ruleBoundary"),
    t("agentPrompt.safety.ruleRisk"),
    t("agentPrompt.safety.ruleNoFabrication"),
    t("agentPrompt.safety.rulePatchQueue"),
  ];

  const safetyRules: Record<string, string> = {
    strict: t("agentPrompt.safety.strict"),
    balanced: t("agentPrompt.safety.balanced"),
    "security-analyzer": t("agentPrompt.safety.securityAnalyzer"),
    "sandbox-yolo": t("agentPrompt.safety.sandboxYolo"),
  };

  const workflowRules: Record<string, string> = {
    ask: t("agentPrompt.workflow.ask"),
    "edit-before-ask": t("agentPrompt.workflow.editBeforeAsk"),
    code: t("agentPrompt.workflow.code"),
    yolo: t("agentPrompt.workflow.yolo"),
  };

  return [
    ...commonRules,
    workflowRules[config.workflowMode],
    safetyRules[config.safetyModel],
    config.approvalMode === "manual" ? t("agentPrompt.approval.manual") : "",
    config.approvalMode === "confirm-risky"
      ? t("agentPrompt.approval.confirmRisky")
      : "",
    config.approvalMode === "auto"
      ? t("agentPrompt.approval.auto")
      : "",
  ]
    .filter(Boolean)
    .join("\n");
}
