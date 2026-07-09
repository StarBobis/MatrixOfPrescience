import type { AgentModel, ChatGroup } from "../stores/settings";

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
    "群聊协作规则：你能看到用户和其它群友已经发出的消息。不要只各说各话；需要回应、补充、质疑或修正其它群友观点时，请明确指出。若你的专业分工暂时不该发言，可以等待其它群友先说。",
    workspacePath
      ? `当前聊天群工作文件夹：${workspacePath}`
      : "当前聊天群尚未设置工作文件夹。涉及代码编辑时，请先要求设置工作文件夹。",
    config
      ? [
          "本地 Agent 协作模式：",
          `- Agent 模式：${config.agentMode}`,
          `- 工作流：${config.workflowMode}`,
          `- 审批模式：${config.approvalMode}`,
          `- 安全模型：${config.safetyModel}`,
          `- EditBeforeAsk：${config.editBeforeAsk ? "开启" : "关闭"}`,
          `- YOLO 模式：${config.yoloMode ? "开启" : "关闭"}`,
        ].join("\n")
      : "",
    buildSafetyPolicy(group),
    "你的核心角色：",
    member.systemPrompt.trim(),
    codeContext
      ? [
          "代码阅读工具结果：",
          "你可以基于以下工具输出讨论代码。优先信任 CodeGraph；如果结果来自 LocalCommands，需要说明这是降级读取。",
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
    "代码协作安全边界：",
    "- 涉及代码阅读时，软件会优先调用 CodeGraph；CodeGraph 不可用时才降级为本地命令。你必须基于“代码阅读工具结果”回答，不能声称只能看到路径。",
    "- 只能围绕当前聊天群工作文件夹进行分析和编辑计划。",
    "- 涉及删除、覆盖、迁移、大范围重命名、运行外部命令、网络请求、密钥或权限变更时，必须显式标记风险。",
    "- 不要编造已执行的文件修改；只有审批队列显示补丁已应用后，才可以声称文件已写入。",
    "- 输出 diff 或补丁会进入审批队列；批准并应用前不得声称已经写入文件。",
  ];

  const safetyRules: Record<string, string> = {
    strict: "- Strict：所有文件写入、命令执行和依赖安装都必须先给出计划并等待确认。",
    balanced: "- Balanced：低风险阅读/小补丁可先建议，高风险操作必须确认。",
    "security-analyzer":
      "- Security Analyzer：先进行风险分类，输出允许/需确认/禁止三类结论，再给出编辑建议。",
    "sandbox-yolo":
      "- Sandbox YOLO：可自动推进低中风险改动计划，但必须假设运行在隔离工作区；禁止越过工作文件夹、删除用户数据或处理密钥。",
  };

  const workflowRules: Record<string, string> = {
    ask: "- Ask：默认解释、诊断、提问，不主动写补丁。",
    "edit-before-ask": "- EditBeforeAsk：先给出最小可行补丁方案，再列出需要用户确认的问题。",
    code: "- Code：优先给出可执行的代码修改计划和补丁。",
    yolo: "- YOLO：尽量减少打断，但仍必须遵守安全模型和工作文件夹边界。",
  };

  return [
    ...commonRules,
    workflowRules[config.workflowMode],
    safetyRules[config.safetyModel],
    config.approvalMode === "manual" ? "- 审批：每个编辑或命令步骤都需要用户确认。" : "",
    config.approvalMode === "confirm-risky"
      ? "- 审批：低风险步骤可合并建议，高风险步骤必须用户确认。"
      : "",
    config.approvalMode === "auto"
      ? "- 审批：自动推进前仍要在回复中记录将要改动的文件和风险。"
      : "",
  ]
    .filter(Boolean)
    .join("\n");
}
