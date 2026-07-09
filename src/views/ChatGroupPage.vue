<script setup lang="ts">
import "./ChatGroupPage.css";
import { computed, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import MarkdownIt from "markdown-it";
import { ElMessage, ElMessageBox } from "element-plus";
import { storeToRefs } from "pinia";
import { useI18n } from "vue-i18n";
import {
  type AgentModel,
  type AgentPatchProposal,
  type ChatMessage,
  type ChatMessageActivityItem,
  type ChatMessageActivityKind,
  type ChatMessageActivityStatus,
  type ChatMessageExecutionItem,
  type ChatMessageExecutionKind,
  type ChatMessageExecutionStatus,
  type OwnerProfile,
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
type MessageStatus = "done" | "thinking" | "error" | "interrupted";

interface ApiChatMessage {
  role: ChatRole;
  content: string;
}

interface ChatTraceStep {
  kind: "reasoning" | "tool";
  text: string;
  detail?: string;
}

interface ChatCompletionUsage {
  promptTokens?: number;
  completionTokens?: number;
  totalTokens?: number;
  promptCacheHitTokens?: number;
  promptCacheMissTokens?: number;
}

interface ChatCompletionResponse {
  content: string;
  traceSteps?: ChatTraceStep[];
  usage?: ChatCompletionUsage;
}

interface ChatCompletionStreamEvent {
  streamId: string;
  eventType: "traceChunk" | "traceStep" | "toolChunk" | "contentChunk" | "usage";
  traceKind?: ChatTraceStep["kind"];
  text: string;
  detail?: string;
  usage?: ChatCompletionUsage;
}

interface ApplyPatchResponse {
  appliedFiles: string[];
  stdout: string;
  stderr: string;
}

type MemberDecision = "speak" | "wait";
type MemberVote = "agree" | "supplement" | "disagree";

interface MemberAnswer { messageId: string; member: AgentModel; content: string }

interface PendingMemberMessage {
  id: string;
  startedAt: number;
}

interface MemberDecisionResult {
  decision: MemberDecision;
  pendingMessage?: PendingMemberMessage;
}

interface ChatCompletionInvokeRequest {
  providerName: string;
  baseUrl: string;
  apiKey: string;
  model: string;
  reasoningEffort: AgentModel["reasoningEffort"];
  temperature: number;
  workspacePath?: string;
  codeToolsEnabled?: boolean;
  canWrite?: boolean;
  streamId?: string;
  systemPrompt: string;
  messages: ApiChatMessage[];
}

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

const DEEPSEEK_LONG_CONTEXT_LIMIT = 1_000_000;
const DEEPSEEK_STANDARD_CONTEXT_LIMIT = 128_000;
const DEFAULT_CONTEXT_LIMIT = 128_000;
const CACHE_PREFIX_MESSAGE_COUNT = 4;
const RECENT_CONVERSATION_MESSAGE_COUNT = 14;
const MAX_MESSAGE_EXECUTION_ITEMS = 80;
const MAX_MESSAGE_THOUGHT_STEPS = 80;
const MAX_TRACE_TEXT_CHARS = 1200;
const MAX_TRACE_DETAIL_CHARS = 6000;
const MAX_STREAMING_TRACE_CHUNK_CHARS = 8000;
const MAX_CONTEXT_EXECUTION_ITEMS = 24;
const MAX_CONTEXT_EXECUTION_TEXT_CHARS = 700;
const MAX_CONTEXT_EXECUTION_DETAIL_CHARS = 1400;
const TRACE_TRUNCATED_MARKER = "\n[trace truncated]";

const { t } = useI18n();
const settingsStore = useSettingsStore();
const { providers, groups, activeGroup, activeMembers, ownerProfile, friends } =
  storeToRefs(settingsStore);
const legacySystemModelName = "\u7cfb\u7edf";
const statusText = computed<Record<MessageStatus, string>>(() => ({
  done: t("chat.status.done"),
  thinking: t("chat.status.thinking"),
  error: t("chat.status.error"),
  interrupted: t("chat.status.interrupted"),
}));

const composer = ref("");
const groupDialogOpen = ref(false);
const sending = ref(false);
const activeRunId = ref("");
const pendingMessageIds = ref<string[]>([]);
const speakerQueue = ref<SpeakerQueueItem[]>([]);
type ChatConversationPanelInstance = InstanceType<typeof ChatConversationPanel> & {
  collapseExecutionPanel: (messageId: string) => void;
};

const chatPanel = ref<ChatConversationPanelInstance | null>(null);
const speakingTimers = new Map<string, number>();
const streamingContent = new Map<string, string>();
const streamingTraceLines = new Map<
  string,
  { kind: ChatTraceStep["kind"]; itemId: string; index: number; text: string }
>();
const streamingToolOutputs = new Map<string, { itemId: string; detail: string }>();

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
  return markdown.render(annotateMarkdownCodeFences(source));
}

function estimateTextTokens(text: string) {
  const normalized = text.trim();

  if (!normalized) {
    return 0;
  }

  const cjkChars = normalized.match(/[\u3400-\u9fff]/g)?.length ?? 0;
  const nonCjkChars = normalized.replace(/[\u3400-\u9fff\s]/g, "").length;
  return Math.max(1, cjkChars + Math.ceil(nonCjkChars / 4));
}

function estimateConversationTokens(messages: ApiChatMessage[], systemPrompt = "") {
  return (
    estimateTextTokens(systemPrompt) +
    messages.reduce((total, message) => total + estimateTextTokens(message.content) + 4, 0) +
    8
  );
}

function getModelContextLimit(member: AgentModel) {
  if (member.provider === "deepseek") {
    return member.deepSeekLongContext ?? true
      ? DEEPSEEK_LONG_CONTEXT_LIMIT
      : DEEPSEEK_STANDARD_CONTEXT_LIMIT;
  }

  const model = member.model.toLowerCase();

  if (model.includes("gpt-4.1")) {
    return 1_000_000;
  }

  return DEFAULT_CONTEXT_LIMIT;
}

function buildContextUsageSnapshot(
  member: AgentModel,
  conversation: ApiChatMessage[],
  systemPrompt = "",
) {
  return {
    contextUsedTokens: estimateConversationTokens(conversation, systemPrompt),
    contextLimitTokens: getModelContextLimit(member),
  };
}

function usageTotalTokens(usage?: ChatCompletionUsage) {
  if (!usage) {
    return undefined;
  }

  return usage.totalTokens ?? (
    Number.isFinite(usage.promptTokens) || Number.isFinite(usage.completionTokens)
      ? (usage.promptTokens ?? 0) + (usage.completionTokens ?? 0)
      : undefined
  );
}

function applyCompletionUsage(messageId: string, usage?: ChatCompletionUsage) {
  const usedTokens = usageTotalTokens(usage);
  const promptTokens = usage?.promptTokens;
  const completionTokens = usage?.completionTokens;
  const cacheHitTokens = usage?.promptCacheHitTokens;
  const cacheMissTokens = usage?.promptCacheMissTokens;
  const hasCacheUsage =
    Number.isFinite(cacheHitTokens) || Number.isFinite(cacheMissTokens);

  const hasUsedTokens = typeof usedTokens === "number" && Number.isFinite(usedTokens);

  if (!hasUsedTokens && !hasCacheUsage) {
    return;
  }

  const patch: Partial<ChatMessage> = {};

  if (hasUsedTokens) {
    patch.contextUsedTokens = usedTokens;
  }

  if (typeof promptTokens === "number" && Number.isFinite(promptTokens)) {
    patch.contextPromptTokens = promptTokens;
  }

  if (typeof completionTokens === "number" && Number.isFinite(completionTokens)) {
    patch.contextCompletionTokens = completionTokens;
  }

  if (typeof cacheHitTokens === "number" && Number.isFinite(cacheHitTokens)) {
    patch.contextCacheHitTokens = cacheHitTokens;
  }

  if (typeof cacheMissTokens === "number" && Number.isFinite(cacheMissTokens)) {
    patch.contextCacheMissTokens = cacheMissTokens;
  }

  settingsStore.updateMessage(messageId, patch);
}

function buildUserContextUsageSnapshot(userText: string) {
  const nextConversation = [...buildConversation(), { role: "user" as const, content: userText }];
  const activeLimits = orderedActiveMembers.value.map(getModelContextLimit);

  return {
    contextUsedTokens: estimateConversationTokens(nextConversation),
    contextLimitTokens: Math.max(DEFAULT_CONTEXT_LIMIT, ...activeLimits),
  };
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

function addDraftMemberFromFriend(friendId: string) {
  const source = friends.value.find((member) => member.id === friendId);

  if (
    source &&
    newGroupMembers.value.some((item) => item.libraryId === source.libraryId)
  ) {
    ElMessage.warning(t("messages.draftMemberAlreadyInNewGroup"));
    return;
  }

  const member = settingsStore.cloneFriend(friendId, newGroupMembers.value);

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
  member.deepSeekLongContext = member.provider === "deepseek";
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

function addFriendMember(friendId: string) {
  if (!settingsStore.addMemberFromFriend(friendId)) {
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

function updateOwnerProfile(profile: OwnerProfile) {
  ownerProfile.value = profile;
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

function getOwnerDisplayName() {
  return ownerProfile.value.name?.trim() || t("common.ownerName");
}

function limitContextText(value: string, maxLength: number) {
  const normalized = value.trim();

  if (normalized.length <= maxLength) {
    return normalized;
  }

  return `${normalized.slice(0, maxLength)}\n[context truncated]`;
}

function getMessageExecutionContextItems(message: ChatMessage): ChatMessageExecutionItem[] {
  if ((message.executionItems ?? []).length > 0) {
    return message.executionItems ?? [];
  }

  return [
    ...(message.activityItems ?? []).map((item, index) => ({
      ...item,
      kind: item.kind,
      createdAt: index,
    })),
    ...(message.thoughtSteps ?? []).map((step, index) => ({
      id: `${message.id}-context-reasoning-${index}`,
      kind: "reasoning" as const,
      status: "done" as const,
      text: step,
      createdAt: (message.activityItems?.length ?? 0) + index,
    })),
  ];
}

function formatExecutionHistoryForModel(message: ChatMessage) {
  const executionItems = getMessageExecutionContextItems(message)
    .filter((item) => item.text.trim().length > 0 || item.detail?.trim())
    .slice(-MAX_CONTEXT_EXECUTION_ITEMS);

  if (executionItems.length === 0) {
    return "";
  }

  const lines = executionItems.map((item) => {
    const detail = item.detail?.trim()
      ? ` detail=${JSON.stringify(limitContextText(item.detail, MAX_CONTEXT_EXECUTION_DETAIL_CHARS))}`
      : "";
    return `- [${item.kind}/${item.status}] ${limitContextText(item.text, MAX_CONTEXT_EXECUTION_TEXT_CHARS)}${detail}`;
  });

  return [
    "Execution history visible to future turns:",
    ...lines,
  ].join("\n");
}

function formatMessageForModel(message: ChatMessage) {
  const speaker = message.role === "assistant" ? message.modelName : getOwnerDisplayName();
  const content = message.content.trim() || "(empty message)";
  const sections = [`${speaker} [status=${message.status}]: ${content}`];

  if (message.role === "assistant") {
    const executionHistory = formatExecutionHistoryForModel(message);

    if (executionHistory) {
      sections.push(executionHistory);
    }
  }

  return sections.join("\n\n");
}

function buildConversation(excludeMessageIds: string[] = []): ApiChatMessage[] {
  const excludedIds = new Set(excludeMessageIds);

  const visibleMessages = activeMessages.value.filter(
    (message) =>
      !excludedIds.has(message.id) &&
      message.modelName !== t("common.systemName") &&
      message.modelName !== legacySystemModelName,
  );
  const cacheFriendlyMessages =
    visibleMessages.length <= CACHE_PREFIX_MESSAGE_COUNT + RECENT_CONVERSATION_MESSAGE_COUNT
      ? visibleMessages
      : [
          ...visibleMessages.slice(0, CACHE_PREFIX_MESSAGE_COUNT),
          ...visibleMessages
            .slice(-RECENT_CONVERSATION_MESSAGE_COUNT)
            .filter(
              (message) =>
                !visibleMessages
                  .slice(0, CACHE_PREFIX_MESSAGE_COUNT)
                  .some((prefixMessage) => prefixMessage.id === message.id),
            ),
        ];

  return cacheFriendlyMessages
    .map<ApiChatMessage>((message) => ({
      role: message.role,
      content: formatMessageForModel(message),
    }));
}

function inferMarkdownCodeFenceLanguage(code: string) {
  const trimmed = code.trim();

  if (/^<template[\s>]|<script[\s>]/i.test(trimmed)) {
    return "vue";
  }

  if (/^\s*[{[]/.test(trimmed) && /[}\]]\s*$/.test(trimmed)) {
    return "json";
  }

  if (/#include\s*<|std::|using\s+namespace\s+std|int\s+main\s*\(|\bcout\s*<</.test(trimmed)) {
    return "cpp";
  }

  if (/\bfn\s+\w+\s*\(|\blet\s+mut\b|\bimpl\s+\w+|println!\s*\(/.test(trimmed)) {
    return "rust";
  }

  if (/\bdef\s+\w+\s*\(|\bfrom\s+\w+\s+import\b|\bprint\s*\(/.test(trimmed)) {
    return "python";
  }

  if (/\binterface\s+\w+|\btype\s+\w+\s*=|:\s*(string|number|boolean|unknown)\b/.test(trimmed)) {
    return "ts";
  }

  if (/\b(import|export)\s+|const\s+\w+\s*=|let\s+\w+\s*=|function\s+\w+\s*\(/.test(trimmed)) {
    return "ts";
  }

  if (/^\s*[.#]?[A-Za-z][\w-]*\s*\{[\s\S]*:\s*[^;]+;/.test(trimmed)) {
    return "css";
  }

  if (/^\s*<[^>]+>[\s\S]*<\/[^>]+>\s*$/.test(trimmed)) {
    return "html";
  }

  if (/^\s*(SELECT|INSERT|UPDATE|DELETE|CREATE|ALTER)\b/i.test(trimmed)) {
    return "sql";
  }

  if (/^\s*(git|npm|pnpm|bun|cargo|python|node|rg)\s+/.test(trimmed)) {
    return "bash";
  }

  return "text";
}

function repairMarkdownCodeFences(content: string) {
  const normalized = content.replace(/\r\n/g, "\n").replace(/\r/g, "\n");
  const lines = normalized.split("\n");
  const repairedLines: string[] = [];
  let openFence: { indent: string; marker: string } | null = null;
  const openingFencePattern = /^([ \t]*)(`{3,})([A-Za-z0-9_+.#-]*)[ \t]*$/;

  for (const line of lines) {
    const openingMatch = line.match(openingFencePattern);

    if (!openFence) {
      if (openingMatch) {
        openFence = {
          indent: openingMatch[1],
          marker: openingMatch[2],
        };
      }

      repairedLines.push(line);
      continue;
    }

    const trimmed = line.trim();
    const closingPattern = new RegExp(`^\`{${openFence.marker.length},}\\s*$`);
    const malformedClosingPattern = new RegExp(
      `^\`{${openFence.marker.length},}[A-Za-z0-9_+.#-]+\\s*$`,
    );

    if (closingPattern.test(trimmed) || malformedClosingPattern.test(trimmed)) {
      repairedLines.push(`${openFence.indent}${openFence.marker}`);
      openFence = null;
      continue;
    }

    repairedLines.push(line);
  }

  if (openFence) {
    repairedLines.push(`${openFence.indent}${openFence.marker}`);
  }

  return repairedLines.join("\n");
}

function annotateMarkdownCodeFences(content: string) {
  return repairMarkdownCodeFences(content).replace(
    /(^|\n)([ \t]*)```\s*\n([\s\S]*?)\n[ \t]*```/g,
    (match, prefix: string, indent: string, code: string) => {
      if (!code.trim()) {
        return match;
      }

      return `${prefix}${indent}\`\`\`${inferMarkdownCodeFenceLanguage(code)}\n${code}\n${indent}\`\`\``;
    },
  );
}

function ensureAddressedReply(content: string, targetName: string) {
  const target = targetName.trim();
  const normalizedContent = annotateMarkdownCodeFences(content).trim();

  if (!target || normalizedContent.startsWith(`@${target}`)) {
    return normalizedContent;
  }

  return `@${target}\n\n${normalizedContent}`;
}

function getInterruptedContent(messageId: string, fallbackContent = "") {
  const streamedContent = streamingContent.get(messageId)?.trim();
  const visibleContent = streamedContent || fallbackContent.trim();
  const ignoredPlaceholders = new Set([
    t("chatRuntime.pendingContent"),
    t("chatRuntime.decisionPendingContent"),
    t("chatRuntime.decisionWaitContent"),
    t("chatRuntime.interruptedContent"),
  ]);

  if (!visibleContent || ignoredPlaceholders.has(visibleContent)) {
    return t("chatRuntime.interruptedContent");
  }

  if (visibleContent.endsWith(t("chatRuntime.interruptedContent"))) {
    return visibleContent;
  }

  return `${annotateMarkdownCodeFences(visibleContent).trimEnd()}\n\n${t("chatRuntime.interruptedContent")}`;
}

function buildAddressedResponseRule(baseRule: string, targetName: string) {
  const rules = [
    baseRule,
    targetName.trim() ? t("chatRuntime.addressReplyRule", { name: targetName.trim() }) : "",
    t("chatRuntime.markdownCodeFenceRule"),
  ];

  return rules.filter(Boolean).join("\n");
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
    /(?:\+\+\+\s+(?:b\/)?([^\s]+))/g,
    /(?:---\s+(?:a\/)?([^\s]+))/g,
    /`([^`]+\.(?:ts|tsx|vue|js|jsx|rs|json|css|md|toml|yml|yaml|c|cc|cpp|cxx|h|hh|hpp|hxx|inl|ipp|hlsl|fx|shader))`/g,
  ];

  for (const pattern of filePatterns) {
    for (const match of source.matchAll(pattern)) {
      if (match[1]) {
        if (match[1] !== "/dev/null") {
          candidates.add(match[1]);
        }
      }
    }
  }

  return [...candidates].slice(0, 12);
}

function hasApplicablePatchShape(patchText: string) {
  const hasTargetHeader = /^(diff --git|---\s+(?:a\/)?\S+|\+\+\+\s+(?:b\/)?\S+)/m.test(patchText);
  const hasApplyBody =
    /^(@@\s|GIT binary patch|new file mode|deleted file mode|old mode|new mode|rename from|rename to|copy from|copy to)/m.test(
      patchText,
    );

  return hasTargetHeader && hasApplyBody;
}

function buildPatchProposalTitle(member: AgentModel, files: string[]) {
  if (files.length === 1) {
    return t("patch.proposalTitleWithFile", { name: member.name, file: files[0] });
  }

  return t("patch.proposalTitleWithCount", { name: member.name, count: files.length });
}

function buildPatchProposalSummary(content: string, patchText: string, files: string[]) {
  const withoutFencedDiff = content.replace(/```(?:diff|patch)\s*[\s\S]*?```/gi, "");
  const withoutRawPatch = patchText ? withoutFencedDiff.replace(patchText, "") : withoutFencedDiff;
  const summary = withoutRawPatch
    .split(/\n+/)
    .map((line) => line.trim())
    .filter(
      (line) =>
        line &&
        !/^(```|diff --git|--- |\+\+\+ |@@ |index |[+-]{1}[^+-])/.test(line),
    )
    .slice(0, 2)
    .join("\n");

  return summary || t("patch.generatedSummary", { count: files.length });
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

  if (!patchText.trim() || files.length === 0 || !hasApplicablePatchShape(patchText)) {
    return;
  }

  const riskLevel = inferRiskLevel(content, files);
  const workspacePath = activeGroup.value?.workspacePath ?? "";

  const proposal = {
    title: buildPatchProposalTitle(member, files),
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
    summary: buildPatchProposalSummary(content, patchText, files).slice(0, 420),
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
  addThoughtSteps(messageId, [step]);
}

function addThoughtSteps(messageId: string, steps: string[]) {
  const message = activeMessages.value.find((item) => item.id === messageId);
  const nextSteps = steps.map((step) => step.trim()).filter(Boolean);

  if (nextSteps.length === 0) {
    return;
  }

  settingsStore.updateMessage(messageId, {
    thoughtSteps: [...(message?.thoughtSteps ?? []), ...nextSteps],
  });
}

function formatReasoningStep(step: ChatTraceStep) {
  const text = limitRuntimeText(step.text.trim(), MAX_TRACE_TEXT_CHARS);

  if (!text) {
    return "";
  }

  return text;
}

function limitRuntimeText(value: string, maxLength: number) {
  if (value.length <= maxLength) {
    return value;
  }

  return `${value.slice(0, maxLength)}${TRACE_TRUNCATED_MARKER}`;
}

function appendRuntimeText(current: string, next: string, maxLength: number) {
  if (current.includes(TRACE_TRUNCATED_MARKER.trim())) {
    return current;
  }

  return limitRuntimeText(`${current}${next}`, maxLength);
}

function addResponseTraceSteps(messageId: string, response: ChatCompletionResponse) {
  for (const step of response.traceSteps ?? []) {
    if (step.kind === "tool") {
      addActivityItem(messageId, "tool", step.text, inferToolActivityStatus(step.text), step.detail);
    } else {
      const text = formatReasoningStep(step);

      if (text) {
        addThoughtStep(messageId, text);
        addExecutionItem(messageId, "reasoning", text, "done", step.detail);
      }
    }
  }
}

function createActivityItem(
  kind: ChatMessageActivityKind,
  text: string,
  status: ChatMessageActivityStatus = "info",
  detail = "",
): ChatMessageActivityItem {
  return {
    id: crypto.randomUUID(),
    kind,
    status,
    text: limitRuntimeText(text, MAX_TRACE_TEXT_CHARS),
    detail: detail.trim() ? limitRuntimeText(detail.trim(), MAX_TRACE_DETAIL_CHARS) : undefined,
  };
}

function createExecutionItem(
  kind: ChatMessageExecutionKind,
  text: string,
  status: ChatMessageExecutionStatus = "info",
  detail = "",
): ChatMessageExecutionItem {
  return {
    id: crypto.randomUUID(),
    kind,
    status,
    text: limitRuntimeText(text, MAX_TRACE_TEXT_CHARS),
    detail: detail.trim() ? limitRuntimeText(detail.trim(), MAX_TRACE_DETAIL_CHARS) : undefined,
    createdAt: Date.now(),
  };
}

function addActivityItem(
  messageId: string,
  kind: ChatMessageActivityKind,
  text: string,
  status: ChatMessageActivityStatus = "info",
  detail = "",
) {
  const trimmed = text.trim();

  if (!trimmed) {
    return;
  }

  const message = activeMessages.value.find((item) => item.id === messageId);
  const activityItem = createActivityItem(kind, trimmed, status, detail);
  const executionItem = createExecutionItem(kind, trimmed, status, detail);
  settingsStore.updateMessage(messageId, {
    activityItems: [
      ...(message?.activityItems ?? []),
      activityItem,
    ].slice(-36),
    executionItems: [
      ...(message?.executionItems ?? []),
      executionItem,
    ].slice(-MAX_MESSAGE_EXECUTION_ITEMS),
  });
}

function addExecutionItem(
  messageId: string,
  kind: ChatMessageExecutionKind,
  text: string,
  status: ChatMessageExecutionStatus = "info",
  detail = "",
) {
  const trimmed = text.trim();

  if (!trimmed) {
    return "";
  }

  const message = activeMessages.value.find((item) => item.id === messageId);
  const executionItem = createExecutionItem(kind, trimmed, status, detail);
  settingsStore.updateMessage(messageId, {
    executionItems: [
      ...(message?.executionItems ?? []),
      executionItem,
    ].slice(-MAX_MESSAGE_EXECUTION_ITEMS),
  });

  return executionItem.id;
}

function inferToolActivityStatus(text: string): ChatMessageActivityStatus {
  const normalized = text.toLowerCase();

  if (normalized.includes("failed") || normalized.includes("error") || normalized.includes("could not")) {
    return "error";
  }

  if (normalized.includes("returned") || normalized.includes("result")) {
    return "done";
  }

  return "running";
}

function updateThoughtStepAt(messageId: string, index: number, step: string) {
  const message = activeMessages.value.find((item) => item.id === messageId);
  const thoughtSteps = [...(message?.thoughtSteps ?? [])];
  const cappedStep = limitRuntimeText(step.trim(), MAX_TRACE_TEXT_CHARS);

  if (!cappedStep) {
    return;
  }

  if (index < thoughtSteps.length) {
    thoughtSteps[index] = cappedStep;
  } else {
    thoughtSteps.push(cappedStep);
  }

  settingsStore.updateMessage(messageId, { thoughtSteps: thoughtSteps.slice(-MAX_MESSAGE_THOUGHT_STEPS) });
}

function updateExecutionItem(
  messageId: string,
  itemId: string,
  patch: Partial<ChatMessageExecutionItem>,
  options: { preserveRawText?: boolean } = {},
) {
  const message = activeMessages.value.find((item) => item.id === messageId);
  const executionItems = (message?.executionItems ?? []).map((item) =>
    item.id === itemId
      ? {
          ...item,
          ...patch,
          text:
            patch.text === undefined
              ? item.text
              : options.preserveRawText
                ? patch.text
                : limitRuntimeText(patch.text, MAX_TRACE_TEXT_CHARS),
          detail:
            patch.detail === undefined
              ? item.detail
              : options.preserveRawText
                ? patch.detail
                : limitRuntimeText(patch.detail, MAX_TRACE_DETAIL_CHARS),
        }
      : item,
  );

  if (executionItems.length === 0) {
    return;
  }

  settingsStore.updateMessage(messageId, { executionItems });
}

function flushStreamingTraceLine(messageId: string) {
  const current = streamingTraceLines.get(messageId);

  if (current?.itemId) {
    updateExecutionItem(messageId, current.itemId, { status: "done" });
  }

  streamingTraceLines.delete(messageId);
}

function traceKindToExecutionKind(kind: ChatTraceStep["kind"]): ChatMessageExecutionKind {
  return kind === "tool" ? "tool" : "reasoning";
}

function appendStreamingTraceText(
  messageId: string,
  kind: ChatTraceStep["kind"],
  text: string,
) {
  const message = activeMessages.value.find((item) => item.id === messageId);
  const current = streamingTraceLines.get(messageId);
  const next =
    current && current.kind === kind
      ? current
      : {
          kind,
          itemId: addExecutionItem(messageId, traceKindToExecutionKind(kind), text, "running"),
          index: message?.thoughtSteps?.length ?? 0,
          text: "",
        };

  next.text = appendRuntimeText(next.text, text, MAX_TRACE_TEXT_CHARS);
  streamingTraceLines.set(messageId, next);
  const formatted = formatReasoningStep({ kind, text: next.text });
  updateThoughtStepAt(messageId, next.index, formatted);

  if (next.itemId) {
    updateExecutionItem(messageId, next.itemId, {
      text: formatted,
      status: "running",
    });
  }
}

function appendStreamingTraceChunk(
  messageId: string,
  kind: ChatTraceStep["kind"],
  chunk: string,
) {
  const normalizedChunk = limitRuntimeText(
    chunk.replace(/\r\n/g, "\n").replace(/\r/g, "\n"),
    MAX_STREAMING_TRACE_CHUNK_CHARS,
  );
  const lines = normalizedChunk.split("\n");

  lines.forEach((line, index) => {
    if (line) {
      appendStreamingTraceText(messageId, kind, line);
    }

    if (index < lines.length - 1) {
      flushStreamingTraceLine(messageId);
    }
  });
}

function formatToolOutputChunk(stream: string, chunk: string) {
  const normalized = chunk.replace(/\r\n/g, "\n").replace(/\r/g, "\n");
  const lines = normalized.split("\n");

  return lines
    .map((line, index) => {
      if (!line && index === lines.length - 1) {
        return "";
      }

      return `[${stream}] ${line}`;
    })
    .join("\n");
}

function latestToolOutputLine(detail: string) {
  const lines = detail
    .split("\n")
    .map((line) => line.trim())
    .filter(Boolean);

  return lines.length > 0 ? lines[lines.length - 1] : t("chatRuntime.commandOutputStreaming");
}

function appendStreamingToolOutputChunk(messageId: string, payload: ChatCompletionStreamEvent) {
  const chunk = payload.text.replace(/\r\n/g, "\n").replace(/\r/g, "\n");

  if (!chunk) {
    return;
  }

  const stream = payload.detail === "stderr" ? "stderr" : "stdout";
  let current = streamingToolOutputs.get(messageId);

  if (!current) {
    current = {
      itemId: addExecutionItem(
        messageId,
        "tool",
        t("chatRuntime.commandOutputStreaming"),
        "running",
      ),
      detail: "",
    };
  }

  const nextDetail = `${current.detail}${current.detail ? "\n" : ""}${formatToolOutputChunk(stream, chunk)}`;
  current.detail = nextDetail;
  streamingToolOutputs.set(messageId, current);

  if (current.itemId) {
    updateExecutionItem(messageId, current.itemId, {
      text: latestToolOutputLine(nextDetail),
      status: "running",
      detail: nextDetail,
    }, { preserveRawText: true });
  }
}

function flushStreamingToolOutput(messageId: string, status: ChatMessageExecutionStatus = "done") {
  const current = streamingToolOutputs.get(messageId);

  if (current?.itemId) {
    updateExecutionItem(messageId, current.itemId, { status });
  }

  streamingToolOutputs.delete(messageId);
}

function appendStreamingContentChunk(messageId: string, chunk: string) {
  if (!chunk) {
    return;
  }

  const previousContent = streamingContent.get(messageId) ?? "";

  if (!previousContent) {
    chatPanel.value?.collapseExecutionPanel(messageId);
  }

  const nextContent = `${previousContent}${chunk}`;
  streamingContent.set(messageId, nextContent);
  settingsStore.updateMessage(messageId, {
    content: nextContent,
  });
}

function applyCompletionStreamEvent(messageId: string, payload: ChatCompletionStreamEvent) {
  if (payload.eventType === "usage") {
    applyCompletionUsage(messageId, payload.usage);
    return;
  }

  if (payload.eventType === "contentChunk") {
    appendStreamingContentChunk(messageId, payload.text);
    return;
  }

  if (payload.eventType === "toolChunk") {
    appendStreamingToolOutputChunk(messageId, payload);
    return;
  }

  const traceKind = payload.traceKind ?? "reasoning";

  if (traceKind === "tool") {
    flushStreamingTraceLine(messageId);
    flushStreamingToolOutput(messageId, inferToolActivityStatus(payload.text));
    addActivityItem(
      messageId,
      "tool",
      payload.text,
      inferToolActivityStatus(payload.text),
      payload.detail,
    );
    return;
  }

  if (payload.eventType === "traceChunk") {
    appendStreamingTraceChunk(messageId, traceKind, payload.text);
    return;
  }

  flushStreamingTraceLine(messageId);
  const text = formatReasoningStep({ kind: traceKind, text: payload.text });
  addThoughtStep(messageId, text);
  addExecutionItem(messageId, traceKindToExecutionKind(traceKind), text, "done", payload.detail);
}

async function invokeStreamingCompletion(
  messageId: string,
  runId: string,
  request: ChatCompletionInvokeRequest,
  options: { showContent?: boolean } = {},
) {
  let sawStreamTrace = false;
  const showContent = options.showContent ?? true;
  const unlisten = await listen<ChatCompletionStreamEvent>(
    "chat-completion-stream",
    (event) => {
      const payload = event.payload;

      if (payload.streamId !== messageId || isRunInterrupted(runId)) {
        return;
      }

      if (payload.eventType === "traceChunk" || payload.eventType === "traceStep") {
        sawStreamTrace = true;
      }

      if (!showContent && payload.eventType === "contentChunk") {
        return;
      }

      applyCompletionStreamEvent(messageId, payload);
    },
  );

  try {
    const response = await invoke<ChatCompletionResponse>("chat_completion", {
      request: {
        ...request,
        streamId: messageId,
      },
    });

    return { response, sawStreamTrace };
  } finally {
    unlisten();
    flushStreamingTraceLine(messageId);
    flushStreamingToolOutput(messageId);
  }
}

function releasePendingMessage(messageId: string, startedAt?: number) {
  flushStreamingTraceLine(messageId);
  flushStreamingToolOutput(messageId);
  streamingContent.delete(messageId);
  stopSpeakingTimer(messageId, startedAt);
  pendingMessageIds.value = pendingMessageIds.value.filter((id) => id !== messageId);
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
    const interruptedContent = getInterruptedContent(messageId, message?.content ?? "");
    releasePendingMessage(messageId, message?.startedAt);
    settingsStore.updateMessage(messageId, {
      status: "interrupted",
      content: interruptedContent,
    });
    addActivityItem(messageId, "status", t("chatRuntime.interruptedStep"), "interrupted");
  }

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
  streamingTraceLines.clear();
  streamingContent.clear();

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
): Promise<MemberDecisionResult> {
  if (isRunInterrupted(runId)) {
    return { decision: "wait" };
  }

  upsertSpeakerQueueMember(member, "checking");
  const provider = getProvider(member);
  const pendingId = crypto.randomUUID();
  const startedAt = Date.now();
  const phaseRule =
    phase === "first"
      ? t("chatRuntime.phaseFirst")
      : t("chatRuntime.phaseAfterPeers");
  const conversation = buildConversation();
  const codeToolsEnabled = shouldInspectWorkspace(phaseRule);
  const systemPrompt = buildSystemPrompt(member, activeGroup.value, phaseRule);
  const contextUsage = buildContextUsageSnapshot(member, conversation, systemPrompt);

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
    ...contextUsage,
    status: "thinking",
    content: t("chatRuntime.decisionPendingContent"),
    color: member.color,
    thoughtSteps: [],
    activityItems: [
      createActivityItem("status", t("chatRuntime.stepCheckingDecision"), "running"),
    ],
    executionItems: [
      createExecutionItem("status", t("chatRuntime.stepCheckingDecision"), "running"),
    ],
  });
  startSpeakingTimer(pendingId, startedAt);
  pendingMessageIds.value = [...pendingMessageIds.value, pendingId];
  await scrollToBottom();

  try {
    addActivityItem(
      pendingId,
      "status",
      codeToolsEnabled ? t("chatRuntime.stepGotContext") : t("chatRuntime.stepNoContext"),
      codeToolsEnabled ? "running" : "info",
    );
    addActivityItem(pendingId, "status", t("chatRuntime.stepWaitingModel"), "running");

    const { response, sawStreamTrace } = await invokeStreamingCompletion(
      pendingId,
      runId,
      {
        providerName: provider.name,
        baseUrl: provider.baseUrl,
        apiKey: provider.apiKey,
        model: member.model,
        reasoningEffort: member.reasoningEffort,
        temperature: 0,
        workspacePath: activeGroup.value?.workspacePath ?? "",
        codeToolsEnabled,
        canWrite: Boolean(member.canWrite),
        systemPrompt,
        messages: conversation,
      },
      { showContent: false },
    );

    if (isRunInterrupted(runId)) {
      releasePendingMessage(pendingId, startedAt);
      return { decision: "wait" };
    }

    if (!sawStreamTrace) {
      addResponseTraceSteps(pendingId, response);
    }

    const decision = parseMemberDecision(response.content);

    if (decision === "wait") {
      settingsStore.updateMessage(pendingId, {
        status: "done",
        content: t("chatRuntime.decisionWaitContent"),
      });
      applyCompletionUsage(pendingId, response.usage);
      addActivityItem(pendingId, "status", t("chatRuntime.stepDecisionWait"), "done");
      releasePendingMessage(pendingId, startedAt);
      upsertSpeakerQueueMember(member, "waiting");
      return { decision };
    }

    streamingContent.delete(pendingId);
    settingsStore.updateMessage(pendingId, {
      status: "thinking",
      content: t("chatRuntime.pendingContent"),
    });
    applyCompletionUsage(pendingId, response.usage);
    addActivityItem(pendingId, "status", t("chatRuntime.stepDecisionSpeak"), "done");
    upsertSpeakerQueueMember(member, "queued");
    return { decision, pendingMessage: { id: pendingId, startedAt } };
  } catch {
    if (isRunInterrupted(runId)) {
      releasePendingMessage(pendingId, startedAt);
      return { decision: "wait" };
    }

    streamingContent.delete(pendingId);
    settingsStore.updateMessage(pendingId, {
      status: "thinking",
      content: t("chatRuntime.pendingContent"),
    });
    addActivityItem(pendingId, "status", t("chatRuntime.stepDecisionFallbackSpeak"), "error");
    upsertSpeakerQueueMember(member, "queued");
    return { decision: "speak", pendingMessage: { id: pendingId, startedAt } };
  }
}

async function askMember(
  member: AgentModel,
  extraRule = "",
  runId: string,
  pendingMessage?: PendingMemberMessage,
  replyTargetName = getOwnerDisplayName(),
): Promise<MemberAnswer | null> {
  if (isRunInterrupted(runId)) {
    return null;
  }

  const provider = getProvider(member);
  const pendingId = pendingMessage?.id ?? crypto.randomUUID();
  const startedAt = pendingMessage?.startedAt ?? Date.now();
  const conversation = buildConversation(pendingMessage ? [pendingId] : []);
  const responseRule = buildAddressedResponseRule(
    extraRule || t("chatRuntime.responseRule"),
    replyTargetName,
  );
  const systemPrompt = buildSystemPrompt(member, activeGroup.value, responseRule);
  const contextUsage = buildContextUsageSnapshot(member, conversation, systemPrompt);

  upsertSpeakerQueueMember(member, "speaking");

  if (pendingMessage) {
    settingsStore.updateMessage(pendingId, {
      status: "thinking",
      content: t("chatRuntime.pendingContent"),
      startedAt,
      durationMs: Math.max(0, Date.now() - startedAt),
      ...contextUsage,
    });
    addActivityItem(pendingId, "status", t("chatRuntime.stepGeneratingAnswer"), "running");
  } else {
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
      ...contextUsage,
      status: "thinking",
      content: t("chatRuntime.pendingContent"),
      color: member.color,
      thoughtSteps: [],
      activityItems: [createActivityItem("status", t("chatRuntime.stepQueued"), "running")],
      executionItems: [createExecutionItem("status", t("chatRuntime.stepQueued"), "running")],
    });
    startSpeakingTimer(pendingId, startedAt);
  }

  if (!pendingMessageIds.value.includes(pendingId)) {
    pendingMessageIds.value = [...pendingMessageIds.value, pendingId];
  }
  await scrollToBottom();

  try {
    const codeToolsEnabled = shouldInspectWorkspace(responseRule);
    addActivityItem(
      pendingId,
      "status",
      codeToolsEnabled ? t("chatRuntime.stepGotContext") : t("chatRuntime.stepNoContext"),
      codeToolsEnabled ? "running" : "info",
    );
    addActivityItem(pendingId, "status", t("chatRuntime.stepWaitingModel"), "running");
    const { response, sawStreamTrace } = await invokeStreamingCompletion(
      pendingId,
      runId,
      {
        providerName: provider.name,
        baseUrl: provider.baseUrl,
        apiKey: provider.apiKey,
        model: member.model,
        reasoningEffort: member.reasoningEffort,
        temperature: member.temperature,
        workspacePath: activeGroup.value?.workspacePath ?? "",
        codeToolsEnabled,
        canWrite: Boolean(member.canWrite),
        systemPrompt,
        messages: conversation,
      },
    );

    if (isRunInterrupted(runId)) {
      releasePendingMessage(pendingId, startedAt);
      return null;
    }

    chatPanel.value?.collapseExecutionPanel(pendingId);
    const finalContent = ensureAddressedReply(response.content, replyTargetName);
    settingsStore.updateMessage(pendingId, {
      status: "done",
      content: finalContent,
    });
    applyCompletionUsage(pendingId, response.usage);
    releasePendingMessage(pendingId, startedAt);
    if (!sawStreamTrace) {
      addResponseTraceSteps(pendingId, response);
    }
    addActivityItem(pendingId, "status", t("chatRuntime.stepDone"), "done");
    await maybeCreatePatchProposal(member, finalContent);
    return {
      messageId: pendingId,
      member,
      content: finalContent,
    };
  } catch (error) {
    if (isRunInterrupted(runId)) {
      const pendingMessage = activeMessages.value.find((item) => item.id === pendingId);
      const interruptedContent = getInterruptedContent(pendingId, pendingMessage?.content ?? "");
      releasePendingMessage(pendingId, startedAt);
      settingsStore.updateMessage(pendingId, {
        status: "interrupted",
        content: interruptedContent,
      });
      addActivityItem(pendingId, "status", t("chatRuntime.interruptedStep"), "interrupted");
      return null;
    }

    releasePendingMessage(pendingId, startedAt);
    settingsStore.updateMessage(pendingId, {
      status: "error",
      content: t("chatRuntime.callFailedContent", { error: String(error) }),
    });
    addActivityItem(pendingId, "status", t("chatRuntime.stepFailed"), "error");
    return null;
  } finally {
    removeSpeakerQueueMember(member.id);
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

  setSpeakerQueue([answer.member, ...voters], "voting");
  upsertSpeakerQueueMember(answer.member, "consensus");

  for (const voter of voters) {
    if (isRunInterrupted(runId)) {
      return [];
    }

    upsertSpeakerQueueMember(voter, "voting");
    const vote = await voteOnAnswer(answer, voter, runId);

    if (vote === "disagree") {
      disagreeMemberIds.push(voter.id);
      upsertSpeakerQueueMember(voter, "followup");
    } else if (vote === "supplement") {
      supplementMemberIds.push(voter.id);
      upsertSpeakerQueueMember(voter, "followup");
    } else {
      agreeMemberIds.push(voter.id);
      upsertSpeakerQueueMember(voter, "voted");
    }
  }

  settingsStore.updateMessage(answer.messageId, {
    agreeMemberIds,
    supplementMemberIds,
    disagreeMemberIds,
  });

  const membersNeedingFollowUp = voters.filter(
    (member) => supplementMemberIds.includes(member.id) || disagreeMemberIds.includes(member.id),
  );

  if (membersNeedingFollowUp.length > 0) {
    setSpeakerQueue(membersNeedingFollowUp, "followup");
  } else {
    speakerQueue.value = [];
  }

  return membersNeedingFollowUp;
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

      if (!currentAnswer) {
        break;
      }

      const replyTargetName = currentAnswer.member.name;
      currentAnswer = await askMember(
        member,
        [
          t("chatRuntime.disagreementRule1"),
          t("chatRuntime.disagreementRule2"),
        ].join("\n"),
        runId,
        undefined,
        replyTargetName,
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
  const userContextUsage = buildUserContextUsageSnapshot(userText);
  const ownerName = getOwnerDisplayName();

  appendMessage({
    role: "user",
    modelName: ownerProfile.value.name || t("common.ownerName"),
    avatar: ownerProfile.value.avatar,
    reasoningEffort: "off",
    status: "done",
    content: userText,
    color: ownerProfile.value.color,
    ...userContextUsage,
  });

  const members = [...orderedActiveMembers.value];
  setSpeakerQueue(members, "queued");
  const mentionedMembers = parseMentionedMembers(userText, members);

  try {
    if (mentionedMembers.length > 0) {
      setSpeakerQueue(mentionedMembers, "queued");
      let latestAnswer: MemberAnswer | null = null;

      for (const member of mentionedMembers) {
        if (isRunInterrupted(runId)) {
          return;
        }

        const answer = await askMember(
          member,
          t("chatRuntime.mentionedRule"),
          runId,
          undefined,
          ownerName,
        );
        latestAnswer = answer ?? latestAnswer;
        await resolveConsensus(answer, runId);
      }

      if (!latestAnswer) {
        return;
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

        if (decision.decision === "speak") {
          const answer = await askMember(
            member,
            t("chatRuntime.observerRule"),
            runId,
            decision.pendingMessage,
            latestAnswer?.member.name ?? ownerName,
          );
          latestAnswer = answer ?? latestAnswer;
          await resolveConsensus(answer, runId);
        }
      }

      return;
    }

    const primaryMember = members[0];

    if (!primaryMember) {
      return;
    }

    setSpeakerQueue([primaryMember], "queued");
    let latestAnswer = await askMember(
      primaryMember,
      t("chatRuntime.defaultResponderRule"),
      runId,
      undefined,
      ownerName,
    );
    await resolveConsensus(latestAnswer, runId);

    if (!latestAnswer) {
      return;
    }

    const observerMembers = prioritizeMembers(members.filter((member) => member.id !== primaryMember.id));
    setSpeakerQueue(observerMembers, "waiting");

    for (const member of observerMembers) {
      if (isRunInterrupted(runId)) {
        return;
      }

      const decision = await decideMemberResponse(member, "afterPeers", runId);

      if (decision.decision === "speak") {
        const answer = await askMember(
          member,
          t("chatRuntime.observerRule"),
          runId,
          decision.pendingMessage,
          latestAnswer?.member.name ?? ownerName,
        );
        latestAnswer = answer ?? latestAnswer;
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
          :friends="friends"
          :owner-profile="ownerProfile"
          :get-provider-label="getProviderLabel"
          :provider-options="providerOptions"
          :model-presets="modelPresets"
          @add-member="addMember"
          @add-friend-member="addFriendMember"
          @remove-member="removeMember"
          @rename-member="renameMember"
          @update-owner-profile="updateOwnerProfile"
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
      :friends="friends"
      :provider-options="providerOptions"
      :model-presets="modelPresets"
      @add-draft-member="addDraftMember"
      @add-draft-member-from-friend="addDraftMemberFromFriend"
      @remove-draft-member="removeDraftMember"
      @update-draft-member-provider="updateDraftMemberProvider"
      @create-group="createGroup"
    />
  </div>
</template>


