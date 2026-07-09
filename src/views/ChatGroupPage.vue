<script setup lang="ts">
import "./ChatGroupPage.css";
import { computed, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import MarkdownIt from "markdown-it";
import { ElMessage, ElMessageBox } from "element-plus";
import { storeToRefs } from "pinia";
import { useI18n } from "vue-i18n";
import {
  type AgentModel,
  type AgentPatchProposal,
  type ChatMessage,
  type PatchApprovalStatus,
  type PatchRiskLevel,
  type ProviderId,
  useSettingsStore,
} from "../stores/settings";
import ChatConversationPanel, {
  type SpeakerQueueItem,
  type SpeakerQueueStatus,
} from "../components/ChatConversationPanel.vue";
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

type MemberDecision = "speak" | "wait";
type MemberVote = "agree" | "supplement" | "disagree";

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

const { t } = useI18n();
const settingsStore = useSettingsStore();
const { providers, groups, activeGroup, activeMembers, ownerProfile, historicalMembers } =
  storeToRefs(settingsStore);
const legacySystemModelName = "\u7cfb\u7edf";
const statusText = computed<Record<MessageStatus, string>>(() => ({
  done: t("chat.status.done"),
  thinking: t("chat.status.thinking"),
  error: t("chat.status.error"),
}));

const composer = ref("");
const groupDialogOpen = ref(false);
const sending = ref(false);
const activeRunId = ref("");
const pendingMessageIds = ref<string[]>([]);
const speakerQueue = ref<SpeakerQueueItem[]>([]);
const chatPanel = ref<InstanceType<typeof ChatConversationPanel> | null>(null);
const speakingTimers = new Map<string, number>();

const newGroupName = ref(t("defaults.newGroup.name"));
const newGroupDescription = ref(t("defaults.newGroup.description"));
const newGroupAnnouncement = ref(t("defaults.newGroup.announcement"));
const newGroupMembers = ref<AgentModel[]>([]);

const activeMessages = computed<ChatMessage[]>(() => activeGroup.value?.messages ?? []);
const activeGroupMembers = computed(() => activeGroup.value?.members ?? []);
const orderedActiveMembers = computed(() => prioritizeMembers(activeMembers.value));
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
  newGroupName.value = t("defaults.newGroup.name");
  newGroupDescription.value = t("defaults.newGroup.description");
  newGroupAnnouncement.value = t("defaults.newGroup.announcement");
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
    ElMessage.warning(t("messages.draftMemberAlreadyInNewGroup"));
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
    ElMessage.warning(t("messages.newGroupNeedsMember"));
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
    ElMessage.warning(t("messages.enterGroupName"));
    return;
  }

  for (const member of newGroupMembers.value) {
    const normalizedName = member.name.trim().toLocaleLowerCase();

    if (!normalizedName) {
      ElMessage.warning(t("messages.memberNameRequired"));
      return;
    }

    if (memberNames.has(normalizedName)) {
      ElMessage.warning(t("messages.memberNamesUnique"));
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
    ElMessage.warning(t("messages.memberAlreadyInCurrentGroup"));
  }
}

function renameMember(memberId: string, name: string) {
  settingsStore.renameMember(memberId, name);
}

function updateMemberProfile(member: AgentModel) {
  settingsStore.updateMemberProfile(member);
}

function updateMemberProvider(member: AgentModel) {
  settingsStore.updateMemberProvider(member);
}

function prioritizeMembers(members: AgentModel[]) {
  return members
    .map((member, index) => ({ member, index }))
    .sort((left, right) => {
      const adminPriority = Number(right.member.isAdmin) - Number(left.member.isAdmin);
      return adminPriority || left.index - right.index;
    })
    .map(({ member }) => member);
}

function setSpeakerQueue(members: AgentModel[], status: SpeakerQueueStatus) {
  speakerQueue.value = members.map((member) => ({
    id: member.id,
    name: member.name,
    color: member.color,
    isAdmin: Boolean(member.isAdmin),
    status,
  }));
}

function upsertSpeakerQueueMember(member: AgentModel, status: SpeakerQueueStatus) {
  const existing = speakerQueue.value.find((item) => item.id === member.id);

  if (existing) {
    existing.name = member.name;
    existing.color = member.color;
    existing.isAdmin = Boolean(member.isAdmin);
    existing.status = status;
    return;
  }

  speakerQueue.value.push({
    id: member.id,
    name: member.name,
    color: member.color,
    isAdmin: Boolean(member.isAdmin),
    status,
  });
}

function removeSpeakerQueueMember(memberId: string) {
  speakerQueue.value = speakerQueue.value.filter((member) => member.id !== memberId);
}

function removeMember(memberId: string) {
  if (activeGroupMembers.value.length <= 1) {
    ElMessage.warning(t("messages.keepOneMember"));
    return;
  }

  settingsStore.removeMember(memberId);
}

function buildConversation(): ApiChatMessage[] {
  return activeMessages.value
    .filter(
      (message) =>
        message.modelName !== t("common.systemName") && message.modelName !== legacySystemModelName,
    )
    .slice(-12)
    .map<ApiChatMessage>((message) => ({
      role: message.role,
      content:
        message.role === "assistant"
          ? `${message.modelName}: ${message.content}`
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
    title: t("patch.proposalTitle", { name: member.name }),
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
      ElMessage.error(t("messages.patchBlocked"));
      return;
    }

    if (!proposal.patchText.trim()) {
      ElMessage.error(t("messages.patchMissingDiff"));
      return;
    }

    if (proposal.safetyCheck.verdict === "needs-confirmation") {
      try {
        await ElMessageBox.confirm(
          t("patch.confirmApply.message"),
          t("patch.confirmApply.title"),
          {
            confirmButtonText: t("patch.confirmApply.confirm"),
            cancelButtonText: t("patch.confirmApply.cancel"),
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
      modelName: t("common.systemName"),
      status: "done",
      color: "#6c6f75",
      content: [
        t("patchRuntime.appliedContent", { title: proposal.title }),
        result.appliedFiles.length > 0
          ? t("patchRuntime.appliedFiles", { files: result.appliedFiles.join(", ") })
          : "",
        result.stdout.trim() ? t("patchRuntime.output", { output: result.stdout.trim() }) : "",
        result.stderr.trim() ? t("patchRuntime.stderr", { stderr: result.stderr.trim() }) : "",
      ]
        .filter(Boolean)
        .join("\n"),
    });
    ElMessage.success(t("messages.patchApplied"));
  } catch (error) {
    appendMessage({
      role: "assistant",
      modelName: t("common.systemName"),
      status: "error",
      color: "#c45656",
      content: t("patchRuntime.failedContent", { title: proposal.title, error: String(error) }),
    });
    ElMessage.error(t("messages.patchFailedLogged"));
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

  if (
    normalized.includes("SUPPLEMENT") ||
    normalized.includes("补充") ||
    normalized.includes("需要补")
  ) {
    return "supplement";
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

function updateSpeakingDuration(messageId: string, startedAt: number) {
  settingsStore.updateMessage(messageId, {
    durationMs: Math.max(0, Date.now() - startedAt),
  });
}

function startSpeakingTimer(messageId: string, startedAt: number) {
  updateSpeakingDuration(messageId, startedAt);
  speakingTimers.set(
    messageId,
    window.setInterval(() => updateSpeakingDuration(messageId, startedAt), 1000),
  );
}

function stopSpeakingTimer(messageId: string, startedAt?: number) {
  const timer = speakingTimers.get(messageId);

  if (timer) {
    window.clearInterval(timer);
    speakingTimers.delete(messageId);
  }

  if (startedAt) {
    updateSpeakingDuration(messageId, startedAt);
  }
}

function stopGeneration() {
  if (!sending.value) {
    return;
  }

  activeRunId.value = "";
  sending.value = false;

  for (const messageId of pendingMessageIds.value) {
    const message = activeMessages.value.find((item) => item.id === messageId);
    stopSpeakingTimer(messageId, message?.startedAt);
    settingsStore.updateMessage(messageId, {
      status: "error",
      content: t("chatRuntime.interruptedContent"),
    });
    addThoughtStep(messageId, t("chatRuntime.interruptedStep"));
  }

  pendingMessageIds.value = [];
  speakerQueue.value = [];
}

async function resetCurrentSession() {
  if (!activeGroup.value) {
    return;
  }

  try {
    await ElMessageBox.confirm(
      t("chat.resetSession.confirmMessage"),
      t("chat.resetSession.confirmTitle"),
      {
        confirmButtonText: t("chat.resetSession.confirm"),
        cancelButtonText: t("common.cancel"),
        type: "warning",
      },
    );
  } catch {
    return;
  }

  for (const timer of speakingTimers.values()) {
    window.clearInterval(timer);
  }
  speakingTimers.clear();

  activeRunId.value = "";
  sending.value = false;
  pendingMessageIds.value = [];
  speakerQueue.value = [];
  settingsStore.clearActiveGroupMessages();
  ElMessage.success(t("chat.resetSession.success"));
}

async function decideMemberResponse(
  member: AgentModel,
  phase: "first" | "afterPeers",
  runId: string,
): Promise<MemberDecision> {
  if (isRunInterrupted(runId)) {
    return "wait";
  }

  upsertSpeakerQueueMember(member, "checking");
  const provider = getProvider(member);
  const phaseRule =
    phase === "first"
      ? t("chatRuntime.phaseFirst")
      : t("chatRuntime.phaseAfterPeers");

  try {
    const response = await invoke<ChatCompletionResponse>("chat_completion", {
      request: {
        providerName: provider.name,
        baseUrl: provider.baseUrl,
        apiKey: provider.apiKey,
        model: member.model,
        reasoningEffort: member.reasoningEffort,
        temperature: 0,
        systemPrompt: buildSystemPrompt(member, activeGroup.value, phaseRule),
        messages: buildConversation(),
      },
    });

    if (isRunInterrupted(runId)) {
      return "wait";
    }

    const decision = parseMemberDecision(response.content);
    upsertSpeakerQueueMember(member, decision === "wait" ? "waiting" : "queued");
    return decision;
  } catch {
    upsertSpeakerQueueMember(member, "queued");
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
  const startedAt = Date.now();
  const conversation = buildConversation();
  const responseRule =
    extraRule ||
    t("chatRuntime.responseRule");

  upsertSpeakerQueueMember(member, "speaking");
  settingsStore.appendPendingMessage({
    id: pendingId,
    role: "assistant",
    modelName: member.name,
    providerName: getProviderLabel(member.provider),
    avatar: member.avatar,
    apiModel: member.model,
    reasoningEffort: member.reasoningEffort,
    startedAt,
    durationMs: 0,
    status: "thinking",
    content: t("chatRuntime.pendingContent"),
    color: member.color,
    thoughtSteps: [t("chatRuntime.stepQueued")],
  });
  startSpeakingTimer(pendingId, startedAt);
  pendingMessageIds.value = [...pendingMessageIds.value, pendingId];
  await scrollToBottom();

  try {
    const codeToolsEnabled = shouldInspectWorkspace(responseRule);
    addThoughtStep(
      pendingId,
      codeToolsEnabled ? t("chatRuntime.stepGotContext") : t("chatRuntime.stepNoContext"),
    );
    addThoughtStep(pendingId, t("chatRuntime.stepWaitingModel"));
    const response = await invoke<ChatCompletionResponse>("chat_completion", {
      request: {
        providerName: provider.name,
        baseUrl: provider.baseUrl,
        apiKey: provider.apiKey,
        model: member.model,
        reasoningEffort: member.reasoningEffort,
        temperature: member.temperature,
        workspacePath: activeGroup.value?.workspacePath ?? "",
        codeToolsEnabled,
        systemPrompt: buildSystemPrompt(member, activeGroup.value, responseRule),
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
    stopSpeakingTimer(pendingId, startedAt);
    addThoughtStep(pendingId, t("chatRuntime.stepDone"));
    await maybeCreatePatchProposal(member, response.content);
    return {
      messageId: pendingId,
      member,
      content: response.content,
    };
  } catch (error) {
    stopSpeakingTimer(pendingId, startedAt);
    settingsStore.updateMessage(pendingId, {
      status: "error",
      content: t("chatRuntime.callFailedContent", { error: String(error) }),
    });
    addThoughtStep(pendingId, t("chatRuntime.stepFailed"));
    return null;
  } finally {
    stopSpeakingTimer(pendingId, startedAt);
    removeSpeakerQueueMember(member.id);
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
        reasoningEffort: voter.reasoningEffort,
        temperature: 0,
        systemPrompt: buildSystemPrompt(
          voter,
          activeGroup.value,
          [
            t("chatRuntime.voteQuestion", { name: answer.member.name }),
            t("chatRuntime.voteAgreeRule"),
            t("chatRuntime.voteSupplementRule"),
            t("chatRuntime.voteDisagreeRule"),
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
  const voters = orderedActiveMembers.value.filter((member) => member.id !== answer.member.id);
  const agreeMemberIds: string[] = [];
  const supplementMemberIds: string[] = [];
  const disagreeMemberIds: string[] = [];

  for (const voter of voters) {
    if (isRunInterrupted(runId)) {
      return [];
    }

    const vote = await voteOnAnswer(answer, voter, runId);

    if (vote === "disagree") {
      disagreeMemberIds.push(voter.id);
    } else if (vote === "supplement") {
      supplementMemberIds.push(voter.id);
    } else {
      agreeMemberIds.push(voter.id);
    }
  }

  settingsStore.updateMessage(answer.messageId, {
    agreeMemberIds,
    supplementMemberIds,
    disagreeMemberIds,
  });

  return voters.filter(
    (member) => supplementMemberIds.includes(member.id) || disagreeMemberIds.includes(member.id),
  );
}

async function resolveConsensus(initialAnswer: MemberAnswer | null, runId: string) {
  let currentAnswer = initialAnswer;
  let round = 0;

  while (currentAnswer && round < maxConsensusRounds) {
    if (isRunInterrupted(runId)) {
      return;
    }

    const membersNeedingFollowUp = await collectMemberVotes(currentAnswer, runId);

    if (membersNeedingFollowUp.length === 0) {
      return;
    }

    for (const member of membersNeedingFollowUp) {
      round += 1;

      if (round > maxConsensusRounds) {
        break;
      }

      currentAnswer = await askMember(
        member,
        [
          t("chatRuntime.disagreementRule1"),
          t("chatRuntime.disagreementRule2"),
        ].join("\n"),
        runId,
      );
    }
  }

  if (round >= maxConsensusRounds) {
    appendMessage({
      role: "assistant",
      modelName: t("common.systemName"),
      status: "error",
      color: "#c45656",
      content: t("chatRuntime.consensusLimit"),
    });
  }
}

async function sendMessage() {
  const userText = composer.value.trim();

  if (!userText || sending.value) {
    return;
  }

  if (activeMembers.value.length === 0) {
    ElMessage.warning(t("messages.unmuteOneMember"));
    return;
  }

  const missingKey = orderedActiveMembers.value.find((member) => !getProvider(member).apiKey.trim());
  if (missingKey) {
    ElMessage.warning(t("messages.configureApiKey", { provider: getProviderLabel(missingKey.provider) }));
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
    modelName: ownerProfile.value.name || t("common.ownerName"),
    avatar: ownerProfile.value.avatar,
    reasoningEffort: "off",
    status: "done",
    content: userText,
    color: ownerProfile.value.color,
  });

  const members = [...orderedActiveMembers.value];
  setSpeakerQueue(members, "queued");
  const mentionedMembers = parseMentionedMembers(userText, members);
  const waitingMembers: AgentModel[] = [];

  try {
    if (mentionedMembers.length > 0) {
      setSpeakerQueue(mentionedMembers, "queued");
      for (const member of mentionedMembers) {
        if (isRunInterrupted(runId)) {
          return;
        }

        const answer = await askMember(
          member,
          t("chatRuntime.mentionedRule"),
          runId,
        );
        await resolveConsensus(answer, runId);
      }

      const observerMembers = prioritizeMembers(
        members.filter((member) => !mentionedMembers.some((mentioned) => mentioned.id === member.id)),
      );
      setSpeakerQueue(observerMembers, "waiting");

      for (const member of observerMembers) {
        if (isRunInterrupted(runId)) {
          return;
        }

        const decision = await decideMemberResponse(member, "afterPeers", runId);

        if (decision === "speak") {
          const answer = await askMember(
            member,
            t("chatRuntime.observerRule"),
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
      speakerQueue.value = [];
    }
  }
}

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
          :active-members="orderedActiveMembers"
          :speaker-queue="speakerQueue"
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
          @reset-session="resetCurrentSession"
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
          :provider-options="providerOptions"
          :model-presets="modelPresets"
          @add-member="addMember"
          @add-historical-member="addHistoricalMember"
          @remove-member="removeMember"
          @rename-member="renameMember"
          @update-member-profile="updateMemberProfile"
          @update-member-provider="updateMemberProvider"
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


