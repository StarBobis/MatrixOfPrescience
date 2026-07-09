<script setup lang="ts">
import "./ChatGroupPage.css";
import { computed, onMounted, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import MarkdownIt from "markdown-it";
import { ElMessage, ElMessageBox } from "element-plus";
import { storeToRefs } from "pinia";
import {
  type AgentModel,
  type AgentPatchProposal,
  type ChatMessage,
  type PatchApprovalStatus,
  type PatchRiskLevel,
  type ProviderId,
  useSettingsStore,
} from "../stores/settings";
import ChatConversationPanel from "../components/ChatConversationPanel.vue";
import CreateGroupDialog from "../components/CreateGroupDialog.vue";
import GroupSidebar from "../components/GroupSidebar.vue";
import GroupRightPanel from "../components/GroupRightPanel.vue";
import ResizableGroupLayout from "../components/ResizableGroupLayout.vue";
import { buildSystemPrompt } from "../utils/agentPrompt";
import { makeMemberNameUnique } from "../utils/memberNames";
import { parseMentionedMembers } from "../utils/mentions";
import { evaluatePatchSafety } from "../utils/patchSafety";

type ChatRole = "user" | "assistant";
type MessageStatus = "done" | "thinking" | "error";

interface ApiChatMessage {
  role: ChatRole;
  content: string;
}

interface ChatCompletionResponse { content: string }

interface ApplyPatchResponse {
  appliedFiles: string[];
  stdout: string;
  stderr: string;
}

interface InspectCodeWorkspaceResponse { tool: string; content: string }

type MemberDecision = "speak" | "wait";
type MemberVote = "agree" | "disagree";

interface MemberAnswer { messageId: string; member: AgentModel; content: string }

const maxConsensusRounds = 8;

const markdown = new MarkdownIt({
  breaks: true,
  html: false,
  linkify: true,
  typographer: true,
});

const providerOptions: Array<{ label: string; value: ProviderId }> = [
  {
    label: "ChatGPT / OpenAI",
    value: "openai",
  },
  {
    label: "DeepSeek",
    value: "deepseek",
  },
];

const modelPresets: Record<ProviderId, string[]> = {
  openai: ["gpt-4.1", "gpt-4.1-mini", "gpt-4o", "gpt-4o-mini"],
  deepseek: ["deepseek-v4-flash", "deepseek-v4-pro", "deepseek-chat"],
};

const statusText: Record<MessageStatus, string> = { done: "完成", thinking: "生成中", error: "错误" };

const settingsStore = useSettingsStore();
const { providers, groups, activeGroup, activeMembers, ownerProfile, historicalMembers } =
  storeToRefs(settingsStore);

const composer = ref("");
const groupDialogOpen = ref(false);
const sending = ref(false);
const activeRunId = ref("");
const pendingMessageIds = ref<string[]>([]);
const chatPanel = ref<InstanceType<typeof ChatConversationPanel> | null>(null);

const newGroupName = ref("新 Agent 群");
const newGroupDescription = ref("一个新的多模型讨论群");
const newGroupAnnouncement = ref(
  "群公告：所有群友需要先独立判断，再给出清晰、可执行的建议。",
);
const newGroupMembers = ref<AgentModel[]>([]);

const activeMessages = computed<ChatMessage[]>(() => activeGroup.value?.messages ?? []);
const activeGroupMembers = computed(() => activeGroup.value?.members ?? []);
const activeGroupAnnouncement = computed({
  get: () => activeGroup.value?.announcement ?? "",
  set: (value: string) => {
    if (activeGroup.value) {
      activeGroup.value.announcement = value;
    }
  },
});
const activeGroupWorkspacePath = computed({
  get: () => activeGroup.value?.workspacePath ?? "",
  set: (value: string) => {
    if (activeGroup.value) {
      activeGroup.value.workspacePath = value;
    }
  },
});
const activeGroupAgentConfig = computed({
  get: () => activeGroup.value?.agentConfig,
  set: (value) => {
    if (activeGroup.value && value) {
      activeGroup.value.agentConfig = value;
    }
  },
});
const canSend = computed(() => composer.value.trim().length > 0 && activeMembers.value.length > 0 && !sending.value);

const emit = defineEmits<{
  openSettings: [];
}>();

function getProvider(model: AgentModel) {
  return providers.value[model.provider];
}

function getProviderLabel(provider: ProviderId) {
  return providerOptions.find((option) => option.value === provider)?.label ?? provider;
}

function renderMarkdown(source: string) {
  return markdown.render(source);
}

function openCreateGroupDialog() {
  newGroupName.value = "新 Agent 群";
  newGroupDescription.value = "一个新的多模型讨论群";
  newGroupAnnouncement.value =
    "群公告：所有群友需要先独立判断，再给出清晰、可执行的建议。";
  newGroupMembers.value = [
    settingsStore.createMemberDraft("openai"),
    settingsStore.createMemberDraft("deepseek"),
  ];
  groupDialogOpen.value = true;
}

function addDraftMember(provider: ProviderId = "openai") {
  const member = settingsStore.createMemberDraft(provider);
  makeMemberNameUnique(member, newGroupMembers.value);
  newGroupMembers.value.push(member);
}

function addDraftMemberFromHistory(memberId: string) {
  const source = historicalMembers.value.find((member) => member.id === memberId);

  if (
    source &&
    newGroupMembers.value.some(
      (item) => item.name.trim().toLocaleLowerCase() === source.name.trim().toLocaleLowerCase(),
    )
  ) {
    ElMessage.warning("这个群友已经在新群里了");
    return;
  }

  const member = settingsStore.cloneHistoricalMember(memberId, newGroupMembers.value);

  if (!member) {
    return;
  }

  newGroupMembers.value.push(member);
}

function removeDraftMember(memberId: string) {
  if (newGroupMembers.value.length <= 1) {
    ElMessage.warning("新群至少需要一个虚拟群友");
    return;
  }

  newGroupMembers.value = newGroupMembers.value.filter((member) => member.id !== memberId);
}

function updateDraftMemberProvider(member: AgentModel) {
  const provider = providers.value[member.provider];

  member.model = provider.defaultModel;
  member.color = member.provider === "openai" ? "#2f76b7" : "#2f7a61";
}

function createGroup() {
  const groupName = newGroupName.value.trim();
  const memberNames = new Set<string>();

  if (!groupName) {
    ElMessage.warning("请输入群名称");
    return;
  }

  for (const member of newGroupMembers.value) {
    const normalizedName = member.name.trim().toLocaleLowerCase();

    if (!normalizedName) {
      ElMessage.warning("群友名称不能为空");
      return;
    }

    if (memberNames.has(normalizedName)) {
      ElMessage.warning("群友名称必须不同");
      return;
    }

    memberNames.add(normalizedName);
  }

  settingsStore.createGroup(
    groupName,
    newGroupDescription.value.trim(),
    newGroupAnnouncement.value.trim(),
    newGroupMembers.value.map((member) => ({ ...member })),
  );
  groupDialogOpen.value = false;
  scrollToBottom();
}

function selectGroup(groupId: string) {
  settingsStore.selectGroup(groupId);
  scrollToBottom();
}

function addMember(provider: ProviderId = "openai") {
  settingsStore.addMember(provider);
}

function addHistoricalMember(memberId: string) {
  if (!settingsStore.addMemberFromHistory(memberId)) {
    ElMessage.warning("这个群友已经在当前群里了");
  }
}

function renameMember(memberId: string, name: string) {
  settingsStore.renameMember(memberId, name);
}

function updateMemberProfile(member: AgentModel) {
  settingsStore.updateMemberProfile(member);
}

function removeMember(memberId: string) {
  if (activeGroupMembers.value.length <= 1) {
    ElMessage.warning("至少保留一个虚拟群友");
    return;
  }

  settingsStore.removeMember(memberId);
}

function buildConversation(): ApiChatMessage[] {
  return activeMessages.value
    .filter((message) => message.modelName !== "系统")
    .slice(-12)
    .map<ApiChatMessage>((message) => ({
      role: message.role,
      content:
        message.role === "assistant"
          ? `${message.modelName}：${message.content}`
          : message.content,
    }));
}

function shouldInspectWorkspace(extraRule: string) {
  const group = activeGroup.value;

  if (!group?.workspacePath?.trim()) {
    return false;
  }

  if (!group.agentConfig || group.agentConfig.agentMode === "chat") {
    return false;
  }

  const recentText = [...activeMessages.value.slice(-4).map((message) => message.content), extraRule]
    .join("\n")
    .toLowerCase();

  return /代码|源码|项目|文件|目录|实现|组件|函数|class|bug|报错|修改|编辑|patch|diff|read|code|source|file|folder|repo|repository/.test(
    recentText,
  );
}

async function inspectWorkspaceForMember(member: AgentModel, extraRule: string) {
  if (!shouldInspectWorkspace(extraRule)) {
    return "";
  }

  const workspacePath = activeGroup.value?.workspacePath?.trim() ?? "";
  const query = [
    `当前群友：${member.name}`,
    `角色身份：${member.systemPrompt}`,
    "请围绕最近群聊问题读取相关代码。优先定位符号、调用链、文件职责和影响范围。",
    activeMessages.value
      .slice(-6)
      .map((message) => `${message.modelName}：${message.content}`)
      .join("\n"),
    extraRule,
  ]
    .filter(Boolean)
    .join("\n\n");

  try {
    const result = await invoke<InspectCodeWorkspaceResponse>("inspect_code_workspace", {
      request: {
        workspacePath,
        query,
      },
    });

    return [`工具：${result.tool}`, result.content].join("\n\n");
  } catch (error) {
    return `代码阅读工具调用失败：${String(error)}`;
  }
}

function extractPatchText(content: string) {
  const diffBlock = content.match(/```(?:diff|patch)\s*([\s\S]*?)```/i);

  if (diffBlock?.[1]?.trim()) {
    return diffBlock[1].trim();
  }

  if (/^\s*(diff --git|--- |\+\+\+ |@@ )/m.test(content)) {
    return content.trim();
  }

  return "";
}

function inferPatchFiles(content: string, patchText: string) {
  const candidates = new Set<string>();
  const source = `${content}\n${patchText}`;
  const filePatterns = [
    /(?:diff --git a\/[^\s]+ b\/([^\s]+))/g,
    /(?:\+\+\+ b\/([^\s]+))/g,
    /(?:--- a\/([^\s]+))/g,
    /`([^`]+\.(?:ts|tsx|vue|js|jsx|rs|json|css|md|toml|yml|yaml))`/g,
  ];

  for (const pattern of filePatterns) {
    for (const match of source.matchAll(pattern)) {
      if (match[1]) {
        candidates.add(match[1]);
      }
    }
  }

  return [...candidates].slice(0, 12);
}

function inferRiskLevel(content: string, files: string[]): PatchRiskLevel {
  const normalized = content.toLowerCase();
  const highRiskTerms = ["delete", "remove-item", "drop table", "secret", "api key", "token"];

  if (
    files.some((file) => file.includes("src-tauri") || file.includes("capabilities")) ||
    highRiskTerms.some((term) => normalized.includes(term))
  ) {
    return "high";
  }

  if (files.length > 3 || normalized.includes("migration") || normalized.includes("dependency")) {
    return "medium";
  }

  return "low";
}

async function maybeCreatePatchProposal(member: AgentModel, content: string) {
  const config = activeGroup.value?.agentConfig;

  if (!config || config.agentMode === "chat" || config.workflowMode === "ask") {
    return;
  }

  const patchText = extractPatchText(content);
  const editIntent = /补丁|patch|diff|修改文件|编辑|apply_patch/i.test(content);

  if (!patchText && !editIntent) {
    return;
  }

  const files = inferPatchFiles(content, patchText);
  const riskLevel = inferRiskLevel(content, files);
  const workspacePath = activeGroup.value?.workspacePath ?? "";

  const proposal = {
    title: `${member.name} 的编辑提案`,
    proposerName: member.name,
    riskLevel,
    workspacePath,
    safetyCheck: evaluatePatchSafety({
      workspacePath,
      files,
      content,
      patchText,
      riskLevel,
      safetyModel: config.safetyModel,
    }),
    files,
    summary: content.slice(0, 420),
    patchText,
  };

  settingsStore.addPatchProposal(proposal);

  if (config.approvalMode === "auto" && proposal.safetyCheck.verdict === "allow" && patchText.trim()) {
    const created = activeGroup.value?.patchProposals[0];

    if (created) {
      await updatePatchProposalStatus(created.id, "approved");
    }
  }
}

async function updatePatchProposalStatus(proposalId: string, status: PatchApprovalStatus) {
  const proposal = activeGroup.value?.patchProposals.find((item) => item.id === proposalId);

  if (!proposal) {
    return;
  }

  if (status === "approved") {
    if (proposal.safetyCheck.verdict === "blocked") {
      ElMessage.error("该补丁被安全模型阻止，不能批准");
      return;
    }

    if (!proposal.patchText.trim()) {
      ElMessage.error("该提案没有可应用的 diff 补丁");
      return;
    }

    if (proposal.safetyCheck.verdict === "needs-confirmation") {
      try {
        await ElMessageBox.confirm(
          "该补丁需要人工确认。确认后会在当前群工作文件夹内执行 git apply。",
          "确认应用补丁",
          {
            confirmButtonText: "批准并应用",
            cancelButtonText: "取消",
            type: "warning",
          },
        );
      } catch {
        return;
      }
    }

    try {
      await applyPatchProposal(proposal);
    } catch {
      return;
    }

    settingsStore.updatePatchProposalStatus(proposalId, status);
    return;
  }

  settingsStore.updatePatchProposalStatus(proposalId, status);
}

async function applyPatchProposal(proposal: AgentPatchProposal) {
  try {
    const result = await invoke<ApplyPatchResponse>("apply_patch_proposal", {
      request: {
        workspacePath: proposal.workspacePath || activeGroup.value?.workspacePath || "",
        patchText: proposal.patchText,
        files: proposal.files,
      },
    });

    appendMessage({
      role: "assistant",
      modelName: "系统",
      status: "done",
      color: "#6c6f75",
      content: [
        `补丁「${proposal.title}」已应用。`,
        result.appliedFiles.length > 0 ? `涉及文件：${result.appliedFiles.join("、")}` : "",
        result.stdout.trim() ? `输出：${result.stdout.trim()}` : "",
        result.stderr.trim() ? `提示：${result.stderr.trim()}` : "",
      ]
        .filter(Boolean)
        .join("\n"),
    });
    ElMessage.success("补丁已应用");
  } catch (error) {
    appendMessage({
      role: "assistant",
      modelName: "系统",
      status: "error",
      color: "#c45656",
      content: `补丁「${proposal.title}」应用失败：${String(error)}`,
    });
    ElMessage.error("补丁应用失败，详情已写入聊天记录");
    throw error;
  }
}

function appendMessage(message: Omit<ChatMessage, "id" | "time">) {
  settingsStore.appendMessage(message);
  scrollToBottom();
}

async function scrollToBottom() {
  await chatPanel.value?.scrollToBottom();
}

function parseMemberDecision(content: string): MemberDecision {
  const normalized = content.trim().toUpperCase();

  if (normalized.includes("WAIT") || normalized.includes("等待")) {
    return "wait";
  }

  return "speak";
}

function parseMemberVote(content: string): MemberVote {
  const normalized = content.trim().toUpperCase();

  if (normalized.includes("DISAGREE") || normalized.includes("不同意") || normalized.includes("反对")) {
    return "disagree";
  }

  return "agree";
}

function isRunInterrupted(runId: string) {
  return !runId || activeRunId.value !== runId;
}

function addThoughtStep(messageId: string, step: string) {
  const message = activeMessages.value.find((item) => item.id === messageId);
  settingsStore.updateMessage(messageId, {
    thoughtSteps: [...(message?.thoughtSteps ?? []), step],
  });
}

function stopGeneration() {
  if (!sending.value) {
    return;
  }

  activeRunId.value = "";
  sending.value = false;

  for (const messageId of pendingMessageIds.value) {
    settingsStore.updateMessage(messageId, {
      status: "error",
      content: "已中断生成。",
    });
    addThoughtStep(messageId, "用户中断，本轮停止。");
  }

  pendingMessageIds.value = [];
}

async function decideMemberResponse(
  member: AgentModel,
  phase: "first" | "afterPeers",
  runId: string,
): Promise<MemberDecision> {
  if (isRunInterrupted(runId)) {
    return "wait";
  }

  const provider = getProvider(member);
  const phaseRule =
    phase === "first"
      ? "现在用户刚发言。请判断你是否应该立刻发言，还是等待其它群友先发言。只回复一个词：SPEAK 或 WAIT。"
      : "现在你已经看到其它群友本轮的新增发言。请判断你是否需要补充、反驳、总结或继续等待。只回复一个词：SPEAK 或 WAIT。";

  try {
    const response = await invoke<ChatCompletionResponse>("chat_completion", {
      request: {
        providerName: provider.name,
        baseUrl: provider.baseUrl,
        apiKey: provider.apiKey,
        model: member.model,
        temperature: 0,
        systemPrompt: buildSystemPrompt(member, activeGroup.value, phaseRule),
        messages: buildConversation(),
      },
    });

    if (isRunInterrupted(runId)) {
      return "wait";
    }

    return parseMemberDecision(response.content);
  } catch {
    return "speak";
  }
}

async function askMember(
  member: AgentModel,
  extraRule = "",
  runId: string,
): Promise<MemberAnswer | null> {
  if (isRunInterrupted(runId)) {
    return null;
  }

  const provider = getProvider(member);
  const pendingId = crypto.randomUUID();
  const conversation = buildConversation();
  const responseRule =
    extraRule ||
    "请基于当前完整群聊历史发言。优先回应用户需求，同时可以补充、质疑或修正其它群友已经说过的内容。";

  settingsStore.appendPendingMessage({
    id: pendingId,
    role: "assistant",
    modelName: member.name,
    providerName: getProviderLabel(member.provider),
    status: "thinking",
    content: "正在生成回复...",
    color: member.color,
    thoughtSteps: ["进入发言队列。"],
  });
  pendingMessageIds.value = [...pendingMessageIds.value, pendingId];
  await scrollToBottom();

  try {
    addThoughtStep(pendingId, "检查是否需要读取代码上下文。");
    const codeContext = await inspectWorkspaceForMember(member, responseRule);

    if (isRunInterrupted(runId)) {
      return null;
    }

    addThoughtStep(
      pendingId,
      codeContext ? "已获取代码阅读工具结果。" : "本轮不需要读取代码上下文。",
    );
    addThoughtStep(pendingId, "等待模型返回。");
    const response = await invoke<ChatCompletionResponse>("chat_completion", {
      request: {
        providerName: provider.name,
        baseUrl: provider.baseUrl,
        apiKey: provider.apiKey,
        model: member.model,
        temperature: member.temperature,
        systemPrompt: buildSystemPrompt(member, activeGroup.value, responseRule, codeContext),
        messages: conversation,
      },
    });

    if (isRunInterrupted(runId)) {
      return null;
    }

    settingsStore.updateMessage(pendingId, {
      status: "done",
      content: response.content,
    });
    addThoughtStep(pendingId, "生成完成。");
    await maybeCreatePatchProposal(member, response.content);
    return {
      messageId: pendingId,
      member,
      content: response.content,
    };
  } catch (error) {
    settingsStore.updateMessage(pendingId, {
      status: "error",
      content: `调用失败：${String(error)}`,
    });
    addThoughtStep(pendingId, "调用失败。");
    return null;
  } finally {
    pendingMessageIds.value = pendingMessageIds.value.filter((id) => id !== pendingId);
    await scrollToBottom();
  }
}

async function voteOnAnswer(
  answer: MemberAnswer,
  voter: AgentModel,
  runId: string,
): Promise<MemberVote> {
  if (isRunInterrupted(runId)) {
    return "agree";
  }

  const provider = getProvider(voter);

  try {
    const response = await invoke<ChatCompletionResponse>("chat_completion", {
      request: {
        providerName: provider.name,
        baseUrl: provider.baseUrl,
        apiKey: provider.apiKey,
        model: voter.model,
        temperature: 0,
        systemPrompt: buildSystemPrompt(
          voter,
          activeGroup.value,
          [
            `请判断你是否同意 ${answer.member.name} 的上一条发言。`,
            "如果结论、方案或关键依据可以接受，只回复 AGREE。",
            "如果你认为存在需要继续讨论的分歧，只回复 DISAGREE。",
          ].join("\n"),
        ),
        messages: buildConversation(),
      },
    });

    if (isRunInterrupted(runId)) {
      return "agree";
    }

    return parseMemberVote(response.content);
  } catch {
    return "agree";
  }
}

async function collectMemberVotes(answer: MemberAnswer, runId: string) {
  const voters = activeMembers.value.filter((member) => member.id !== answer.member.id);
  const agreeMemberIds: string[] = [];
  const disagreeMemberIds: string[] = [];

  for (const voter of voters) {
    if (isRunInterrupted(runId)) {
      return [];
    }

    const vote = await voteOnAnswer(answer, voter, runId);

    if (vote === "disagree") {
      disagreeMemberIds.push(voter.id);
    } else {
      agreeMemberIds.push(voter.id);
    }
  }

  settingsStore.updateMessage(answer.messageId, {
    agreeMemberIds,
    disagreeMemberIds,
  });

  return voters.filter((member) => disagreeMemberIds.includes(member.id));
}

async function resolveConsensus(initialAnswer: MemberAnswer | null, runId: string) {
  let currentAnswer = initialAnswer;
  let round = 0;

  while (currentAnswer && round < maxConsensusRounds) {
    if (isRunInterrupted(runId)) {
      return;
    }

    const disagreeingMembers = await collectMemberVotes(currentAnswer, runId);

    if (disagreeingMembers.length === 0) {
      return;
    }

    for (const member of disagreeingMembers) {
      round += 1;

      if (round > maxConsensusRounds) {
        break;
      }

      currentAnswer = await askMember(
        member,
        [
          "你刚才不同意上一条发言。请明确指出分歧点，并给出能推动集体达成一致的修正方案。",
          "不要重复已经说过的内容；优先提出可被其它群友同意的结论。",
        ].join("\n"),
        runId,
      );
    }
  }

  if (round >= maxConsensusRounds) {
    appendMessage({
      role: "assistant",
      modelName: "系统",
      status: "error",
      color: "#c45656",
      content: "本轮讨论达到共识轮次上限，仍存在分歧。请人工收束问题或调整群友角色后继续。",
    });
  }
}

async function sendMessage() {
  const userText = composer.value.trim();

  if (!userText || sending.value) {
    return;
  }

  if (activeMembers.value.length === 0) {
    ElMessage.warning("请至少解除一个虚拟群友的禁言");
    return;
  }

  const missingKey = activeMembers.value.find((member) => !getProvider(member).apiKey.trim());
  if (missingKey) {
    ElMessage.warning(`请先配置 ${getProviderLabel(missingKey.provider)} API Key`);
    emit("openSettings");
    return;
  }

  composer.value = "";
  sending.value = true;
  const runId = crypto.randomUUID();
  activeRunId.value = runId;
  pendingMessageIds.value = [];

  appendMessage({
    role: "user",
    modelName: "我",
    status: "done",
    content: userText,
    color: ownerProfile.value.color,
  });

  const members = [...activeMembers.value];
  const mentionedMembers = parseMentionedMembers(userText, members);
  const waitingMembers: AgentModel[] = [];

  try {
    if (mentionedMembers.length > 0) {
      for (const member of mentionedMembers) {
        if (isRunInterrupted(runId)) {
          return;
        }

        const answer = await askMember(
          member,
          "用户明确 @ 了你。请你先回答；其它群友会先观望，等你回答完再判断是否需要补充。",
          runId,
        );
        await resolveConsensus(answer, runId);
      }

      const observerMembers = members.filter(
        (member) => !mentionedMembers.some((mentioned) => mentioned.id === member.id),
      );

      for (const member of observerMembers) {
        if (isRunInterrupted(runId)) {
          return;
        }

        const decision = await decideMemberResponse(member, "afterPeers", runId);

        if (decision === "speak") {
          const answer = await askMember(
            member,
            "你刚才处于观望状态。现在被 @ 的群友已经回答，请判断是否需要补充、反驳或帮助收束共识。",
            runId,
          );
          await resolveConsensus(answer, runId);
        }
      }

      return;
    }

    for (const member of members) {
      if (isRunInterrupted(runId)) {
        return;
      }

      const decision = await decideMemberResponse(member, "first", runId);

      if (decision === "wait") {
        waitingMembers.push(member);
        continue;
      }

      const answer = await askMember(member, "", runId);
      await resolveConsensus(answer, runId);
    }

    for (const member of waitingMembers) {
      if (isRunInterrupted(runId)) {
        return;
      }

      const decision = await decideMemberResponse(member, "afterPeers", runId);

      if (decision === "speak") {
        const answer = await askMember(member, "", runId);
        await resolveConsensus(answer, runId);
      }
    }
  } finally {
    if (activeRunId.value === runId) {
      activeRunId.value = "";
      sending.value = false;
      pendingMessageIds.value = [];
    }
  }
}

onMounted(() => {
  try {
    settingsStore.hydrate();
    settingsStore.startPersistence();
  } catch {
    ElMessage.warning("本地群聊配置读取失败，已使用默认配置");
    settingsStore.startPersistence();
  }
});
</script>

<template>
  <div class="content-area">
    <ResizableGroupLayout>
      <template #left>
        <GroupSidebar
          :groups="groups"
          :active-group-id="activeGroup?.id"
          @select-group="selectGroup"
          @create-group="openCreateGroupDialog"
        />
      </template>

      <template #main>
        <ChatConversationPanel
          ref="chatPanel"
          v-model:composer="composer"
          :active-group="activeGroup"
          :active-member-count="activeMembers.length"
          :active-members="activeMembers"
          :messages="activeMessages"
          :patch-proposals="activeGroup?.patchProposals ?? []"
          v-model:workspace-path="activeGroupWorkspacePath"
          :sending="sending"
          :can-send="canSend"
          :status-text="statusText"
          :render-markdown="renderMarkdown"
          @update-patch-status="updatePatchProposalStatus"
          @remove-patch-proposal="settingsStore.removePatchProposal"
          @send-message="sendMessage"
          @stop-generation="stopGeneration"
        />
      </template>

      <template #right>
        <GroupRightPanel
          v-if="activeGroupAgentConfig"
          v-model:announcement="activeGroupAnnouncement"
          v-model:agent-config="activeGroupAgentConfig"
          :members="activeGroupMembers"
          :historical-members="historicalMembers"
          :owner-profile="ownerProfile"
          :get-provider-label="getProviderLabel"
          @add-member="addMember"
          @add-historical-member="addHistoricalMember"
          @remove-member="removeMember"
          @rename-member="renameMember"
          @update-member-profile="updateMemberProfile"
        />
      </template>
    </ResizableGroupLayout>

    <CreateGroupDialog
      v-model:open="groupDialogOpen"
      v-model:name="newGroupName"
      v-model:description="newGroupDescription"
      v-model:announcement="newGroupAnnouncement"
      :members="newGroupMembers"
      :historical-members="historicalMembers"
      :provider-options="providerOptions"
      :model-presets="modelPresets"
      @add-draft-member="addDraftMember"
      @add-draft-member-from-history="addDraftMemberFromHistory"
      @remove-draft-member="removeDraftMember"
      @update-draft-member-provider="updateDraftMemberProvider"
      @create-group="createGroup"
    />
  </div>
</template>


