<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { open } from "@tauri-apps/plugin-dialog";
import { CircleClose, FolderOpened, Promotion, RefreshLeft, Tools } from "@element-plus/icons-vue";
import { useI18n } from "vue-i18n";
import PatchApprovalPanel from "./PatchApprovalPanel.vue";
import { getAvatarSrc } from "../utils/avatar";
import type {
  AgentModel,
  ChatGroup,
  ChatMessage,
  ChatMessageExecutionItem,
  PatchApprovalStatus,
} from "../stores/settings";

export type SpeakerQueueStatus = "queued" | "checking" | "waiting" | "speaking";

export interface SpeakerQueueItem {
  id: string;
  name: string;
  color: string;
  isAdmin: boolean;
  status: SpeakerQueueStatus;
}

interface ExecutionRenderBlock {
  id: string;
  item: ChatMessageExecutionItem;
  type: "markdown" | "code";
  text: string;
  language?: string;
}

const props = defineProps<{
  activeGroup?: ChatGroup;
  activeMemberCount: number;
  activeMembers: AgentModel[];
  speakerQueue: SpeakerQueueItem[];
  messages: ChatMessage[];
  patchProposals: ChatGroup["patchProposals"];
  composer: string;
  workspacePath: string;
  sending: boolean;
  canSend: boolean;
  statusText: Record<ChatMessage["status"], string>;
  renderMarkdown: (source: string) => string;
}>();

const emit = defineEmits<{
  "update:composer": [value: string];
  "update:workspacePath": [value: string];
  updatePatchStatus: [proposalId: string, status: PatchApprovalStatus];
  removePatchProposal: [proposalId: string];
  sendMessage: [];
  stopGeneration: [];
  resetSession: [];
}>();

const messagesPanel = ref<HTMLElement | null>(null);
const messagesStack = ref<HTMLElement | null>(null);
const messagesEnd = ref<HTMLElement | null>(null);
const composerFooter = ref<HTMLElement | null>(null);
const approvalPanelOpen = ref(false);
const executionPanelOpen = ref<Record<string, boolean>>({});
const stickToBottom = ref(true);
const { t } = useI18n();
const previousMessageStatuses = new Map<string, ChatMessage["status"]>();
let pendingScrollFrame = 0;
let layoutResizeObserver: ResizeObserver | null = null;
const bottomStickyThreshold = 96;
const followScrollFrames = 3;

const mentionMatch = computed(() => {
  const match = props.composer.match(/(?:^|\s)@([^@\s]*)$/);
  return match ? match[1] : "";
});

const mentionOpen = computed(() => /(?:^|\s)@[^@\s]*$/.test(props.composer));

const mentionCandidates = computed(() => {
  const query = mentionMatch.value.trim().toLocaleLowerCase();
  return props.activeMembers
    .filter((member) => member.name.toLocaleLowerCase().includes(query))
    .slice(0, 8);
});

const speakerQueueStatusType: Record<SpeakerQueueStatus, "info" | "warning" | "success" | "primary"> = {
  queued: "info",
  checking: "warning",
  waiting: "info",
  speaking: "success",
};
const codeFenceLinePattern = /^\s*```([A-Za-z0-9_+.#-]*)?\s*$/;
const hashCommentLanguages = new Set([
  "bash",
  "ini",
  "ps1",
  "py",
  "python",
  "rb",
  "ruby",
  "sh",
  "toml",
  "yaml",
  "yml",
  "zsh",
]);
const codeKeywords = new Set([
  "abstract",
  "and",
  "as",
  "async",
  "await",
  "break",
  "case",
  "catch",
  "class",
  "const",
  "continue",
  "def",
  "default",
  "defer",
  "del",
  "do",
  "else",
  "enum",
  "export",
  "extends",
  "final",
  "finally",
  "fn",
  "for",
  "from",
  "func",
  "function",
  "global",
  "go",
  "if",
  "implements",
  "import",
  "in",
  "interface",
  "is",
  "let",
  "match",
  "mut",
  "new",
  "not",
  "or",
  "package",
  "private",
  "protected",
  "public",
  "return",
  "select",
  "static",
  "struct",
  "switch",
  "template",
  "throw",
  "try",
  "type",
  "typedef",
  "using",
  "var",
  "void",
  "while",
  "with",
  "yield",
]);
const codeLiterals = new Set([
  "False",
  "Infinity",
  "NaN",
  "None",
  "True",
  "false",
  "nil",
  "null",
  "self",
  "super",
  "this",
  "true",
  "undefined",
]);
const codeOperatorCharacters = new Set(["=", "+", "-", "*", "/", "%", "<", ">", "!", "&", "|", "^", "~", "?", ":"]);

function hasApplicablePatchShape(patchText: string) {
  const hasTargetHeader = /^(diff --git|---\s+(?:a\/)?\S+|\+\+\+\s+(?:b\/)?\S+)/m.test(patchText);
  const hasApplyBody =
    /^(@@\s|GIT binary patch|new file mode|deleted file mode|old mode|new mode|rename from|rename to|copy from|copy to)/m.test(
      patchText,
    );

  return hasTargetHeader && hasApplyBody;
}

const actionablePatchProposals = computed(() =>
  props.patchProposals.filter(
    (proposal) =>
      proposal.patchText.trim().length > 0 &&
      proposal.files.length > 0 &&
      hasApplicablePatchShape(proposal.patchText),
  ),
);

const pendingPatchCount = computed(
  () => actionablePatchProposals.value.filter((proposal) => proposal.status === "pending").length,
);

function getBottomDistance(panel: HTMLElement) {
  return panel.scrollHeight - panel.scrollTop - panel.clientHeight;
}

function updateStickToBottom() {
  const panel = messagesPanel.value;

  if (!panel) {
    stickToBottom.value = true;
    return;
  }

  stickToBottom.value = getBottomDistance(panel) <= bottomStickyThreshold;
}

function settleAtBottom() {
  const panel = messagesPanel.value;

  if (!panel) {
    return;
  }

  messagesEnd.value?.scrollIntoView({ block: "end" });
  panel.scrollTop = panel.scrollHeight;
}

function runFollowScrollFrames(remainingFrames: number) {
  if (pendingScrollFrame) {
    window.cancelAnimationFrame(pendingScrollFrame);
  }

  pendingScrollFrame = window.requestAnimationFrame(() => {
    pendingScrollFrame = 0;

    if (!stickToBottom.value) {
      return;
    }

    settleAtBottom();

    if (remainingFrames > 1) {
      runFollowScrollFrames(remainingFrames - 1);
    }
  });
}

function scheduleFollowScroll(force = false) {
  if (!force && !stickToBottom.value) {
    return;
  }

  void nextTick(() => {
    if (!force && !stickToBottom.value) {
      return;
    }

    runFollowScrollFrames(followScrollFrames);
  });
}

async function scrollToBottom() {
  stickToBottom.value = true;
  await nextTick();

  settleAtBottom();
  runFollowScrollFrames(followScrollFrames);
}

const messageScrollSignature = computed(() =>
  props.messages
    .map((message) =>
      [
        message.id,
        message.status,
        message.content.length,
        message.durationMs ?? 0,
        message.contextUsedTokens ?? 0,
        message.contextCacheHitTokens ?? 0,
        message.contextCacheMissTokens ?? 0,
        (message.thoughtSteps ?? []).join("\n").length,
        (message.activityItems ?? [])
          .map((item) => `${item.id}:${item.status}:${item.text.length}:${item.detail?.length ?? 0}`)
          .join(","),
        (message.executionItems ?? [])
          .map((item) => `${item.id}:${item.kind}:${item.status}:${item.text.length}:${item.detail?.length ?? 0}`)
          .join(","),
      ].join(":"),
    )
    .join("|"),
);

watch(
  messageScrollSignature,
  () => {
    scheduleFollowScroll();
  },
  { flush: "post" },
);

watch(
  () => props.messages.map((message) => `${message.id}:${message.status}`).join("|"),
  () => {
    const nextOpenState = { ...executionPanelOpen.value };
    const activeMessageIds = new Set(props.messages.map((message) => message.id));
    let changed = false;

    for (const message of props.messages) {
      const previousStatus = previousMessageStatuses.get(message.id);

      if (previousStatus === "thinking" && message.status !== "thinking") {
        nextOpenState[message.id] = false;
        changed = true;
      }

      previousMessageStatuses.set(message.id, message.status);
    }

    for (const messageId of previousMessageStatuses.keys()) {
      if (!activeMessageIds.has(messageId)) {
        previousMessageStatuses.delete(messageId);
        delete nextOpenState[messageId];
        changed = true;
      }
    }

    if (changed) {
      executionPanelOpen.value = nextOpenState;
    }
  },
  { flush: "post", immediate: true },
);

watch(
  () => props.composer,
  () => {
    scheduleFollowScroll();
  },
  { flush: "post" },
);

onMounted(() => {
  if (typeof ResizeObserver === "undefined") {
    return;
  }

  layoutResizeObserver = new ResizeObserver(() => {
    scheduleFollowScroll();
  });

  if (messagesStack.value) {
    layoutResizeObserver.observe(messagesStack.value);
  }

  if (composerFooter.value) {
    layoutResizeObserver.observe(composerFooter.value);
  }
});

onBeforeUnmount(() => {
  if (pendingScrollFrame) {
    window.cancelAnimationFrame(pendingScrollFrame);
    pendingScrollFrame = 0;
  }

  layoutResizeObserver?.disconnect();
  layoutResizeObserver = null;
});

async function chooseWorkspacePath() {
  const selected = await open({
    directory: true,
    multiple: false,
    title: t("chat.workspace.pickDialogTitle"),
  });

  if (typeof selected === "string") {
    emit("update:workspacePath", selected);
  }
}

function insertMention(member: AgentModel) {
  const nextComposer = props.composer.replace(/(?:^|\s)@[^@\s]*$/, (match) => {
    const prefix = match.startsWith(" ") ? " " : "";
    return `${prefix}@${member.name} `;
  });

  emit("update:composer", nextComposer);
}

function getInitial(name: string) {
  return name.trim().slice(0, 1) || "?";
}

function findMessageMember(message: ChatMessage) {
  return (props.activeGroup?.members ?? props.activeMembers).find(
    (member) => member.name === message.modelName,
  );
}

function getMessageAvatar(message: ChatMessage) {
  return message.avatar || findMessageMember(message)?.avatar || "";
}

function getMessageAvatarSrc(message: ChatMessage) {
  return getAvatarSrc(getMessageAvatar(message));
}

function getMessageApiModel(message: ChatMessage) {
  return message.apiModel || findMessageMember(message)?.model || "";
}

function getMessageReasoningEffort(message: ChatMessage) {
  return message.reasoningEffort || findMessageMember(message)?.reasoningEffort || "off";
}

function isAdminMessage(message: ChatMessage) {
  return message.role === "assistant" && Boolean(findMessageMember(message)?.isAdmin);
}

function formatDuration(durationMs?: number) {
  if (!Number.isFinite(durationMs)) {
    return "";
  }

  const totalSeconds = Math.max(0, Math.round((durationMs ?? 0) / 1000));
  const minutes = Math.floor(totalSeconds / 60);
  const seconds = totalSeconds % 60;

  if (minutes <= 0) {
    return `${seconds}s`;
  }

  return `${minutes}m ${seconds.toString().padStart(2, "0")}s`;
}

function getReasoningLabel(message: ChatMessage) {
  const effort = getMessageReasoningEffort(message);

  return t(`members.reasoningEffortOptions.${effort}`);
}

function allPeersAgreed(message: ChatMessage) {
  if (message.role !== "assistant") {
    return false;
  }

  const author = findMessageMember(message);

  if (!author) {
    return false;
  }

  const voters = props.activeMembers.filter((member) => member.id !== author.id);

  if (voters.length === 0) {
    return false;
  }

  const agreeMemberIds = new Set(message.agreeMemberIds ?? []);
  const supplementCount = (message.supplementMemberIds ?? []).length;
  const disagreeCount = (message.disagreeMemberIds ?? []).length;

  return supplementCount === 0 && disagreeCount === 0 && voters.every((member) => agreeMemberIds.has(member.id));
}

function getAgreeLabel(message: ChatMessage) {
  if (allPeersAgreed(message)) {
    return t("chat.allAgree");
  }

  return t("chat.agree", { count: (message.agreeMemberIds ?? []).length });
}

function getExecutionItems(message: ChatMessage): ChatMessageExecutionItem[] {
  const executionItems = message.executionItems ?? [];

  if (executionItems.length > 0) {
    return executionItems;
  }

  return [
    ...(message.activityItems ?? []).map((item, index) => ({
      ...item,
      createdAt: index,
    })),
    ...(message.thoughtSteps ?? [])
      .map((step, index) => ({
        id: `${message.id}-legacy-reasoning-${index}`,
        kind: "reasoning" as const,
        status: "done" as const,
        text: step,
        createdAt: (message.activityItems?.length ?? 0) + index,
      }))
      .filter((item) => item.text.trim().length > 0),
  ];
}

function normalizeCodeLanguage(language?: string) {
  return (language ?? "").trim().replace(/[^\w+.#-]/g, "").slice(0, 32).toLowerCase();
}

function pushMarkdownExecutionBlock(
  blocks: ExecutionRenderBlock[],
  item: ChatMessageExecutionItem,
  text: string,
  suffix: string,
) {
  if (text.trim().length === 0 && !item.detail) {
    return;
  }

  blocks.push({
    id: `${item.id}-markdown-${suffix}`,
    item,
    text,
    type: "markdown",
  });
}

function pushCodeExecutionBlock(
  blocks: ExecutionRenderBlock[],
  openCodeBlock: { item: ChatMessageExecutionItem; language: string; lines: string[] },
) {
  const text = openCodeBlock.lines.join("\n").replace(/\n+$/g, "");

  if (text.trim().length === 0) {
    return;
  }

  blocks.push({
    id: `${openCodeBlock.item.id}-code-${blocks.length}`,
    item: openCodeBlock.item,
    language: openCodeBlock.language,
    text,
    type: "code",
  });
}

function getExecutionBlocks(message: ChatMessage): ExecutionRenderBlock[] {
  const blocks: ExecutionRenderBlock[] = [];
  let openCodeBlock: { item: ChatMessageExecutionItem; language: string; lines: string[] } | null = null;

  getExecutionItems(message).forEach((item, itemIndex) => {
    if (item.kind !== "reasoning") {
      if (openCodeBlock) {
        pushCodeExecutionBlock(blocks, openCodeBlock);
        openCodeBlock = null;
      }

      pushMarkdownExecutionBlock(blocks, item, item.text, `${itemIndex}`);
      return;
    }

    const lines = item.text.replace(/\r\n/g, "\n").split("\n");
    const markdownLines: string[] = [];
    let partIndex = 0;

    lines.forEach((line) => {
      const fenceMatch = line.match(codeFenceLinePattern);

      if (fenceMatch) {
        if (openCodeBlock) {
          pushCodeExecutionBlock(blocks, openCodeBlock);
          openCodeBlock = null;
        } else {
          pushMarkdownExecutionBlock(blocks, item, markdownLines.join("\n"), `${itemIndex}-${partIndex}`);
          markdownLines.length = 0;
          partIndex += 1;
          openCodeBlock = {
            item,
            language: normalizeCodeLanguage(fenceMatch[1]),
            lines: [],
          };
        }

        return;
      }

      if (openCodeBlock) {
        openCodeBlock.lines.push(line);
      } else {
        markdownLines.push(line);
      }
    });

    pushMarkdownExecutionBlock(blocks, item, markdownLines.join("\n"), `${itemIndex}-${partIndex}`);
  });

  if (openCodeBlock) {
    pushCodeExecutionBlock(blocks, openCodeBlock);
  }

  return blocks;
}

function hasExecutionItems(message: ChatMessage) {
  return getExecutionItems(message).length > 0;
}

function shouldShowExecutionToggle(message: ChatMessage) {
  return message.role === "assistant" && (message.status === "thinking" || hasExecutionItems(message));
}

function getExecutionCount(message: ChatMessage) {
  return getExecutionBlocks(message).length;
}

function isExecutionOpen(message: ChatMessage) {
  return executionPanelOpen.value[message.id] ?? message.status === "thinking";
}

function toggleExecutionPanel(message: ChatMessage) {
  executionPanelOpen.value = {
    ...executionPanelOpen.value,
    [message.id]: !isExecutionOpen(message),
  };
  scheduleFollowScroll();
}

function getExecutionToggleLabel(message: ChatMessage) {
  return isExecutionOpen(message) ? t("chat.execution.collapse") : t("chat.execution.expand");
}

function getLatestExecutionItem(message: ChatMessage) {
  const items = getExecutionItems(message);
  return items[items.length - 1];
}

function getLatestExecutionItems(message: ChatMessage) {
  const item = getLatestExecutionItem(message);
  return item ? [item] : [];
}

function getExecutionIcon(item: ChatMessageExecutionItem) {
  return item.kind === "tool" ? Tools : RefreshLeft;
}

function getExecutionKindLabel(item: ChatMessageExecutionItem) {
  return t(`chat.execution.kind.${item.kind}`);
}

function renderExecutionMarkdown(text: string) {
  return props.renderMarkdown(text);
}

function formatExecutionCodeLanguage(language?: string) {
  return language ? language.toUpperCase() : "code";
}

function escapeHtml(value: string) {
  return value.replace(/[&<>"']/g, (character) => {
    switch (character) {
      case "&":
        return "&amp;";
      case "<":
        return "&lt;";
      case ">":
        return "&gt;";
      case "\"":
        return "&quot;";
      case "'":
        return "&#39;";
      default:
        return character;
    }
  });
}

function wrapSyntaxToken(kind: string, value: string) {
  return `<span class="syntax-${kind}">${escapeHtml(value)}</span>`;
}

function isIdentifierStart(character: string) {
  return /[A-Za-z_$]/.test(character);
}

function isIdentifierPart(character: string) {
  return /[A-Za-z0-9_$]/.test(character);
}

function readQuotedToken(line: string, startIndex: number) {
  const quote = line[startIndex];
  let index = startIndex + 1;
  let escaped = false;

  while (index < line.length) {
    const character = line[index];

    if (escaped) {
      escaped = false;
      index += 1;
      continue;
    }

    if (character === "\\") {
      escaped = true;
      index += 1;
      continue;
    }

    index += 1;

    if (character === quote) {
      break;
    }
  }

  return index;
}

function highlightCodeLine(line: string, language: string) {
  let html = "";
  let index = 0;

  while (index < line.length) {
    const remaining = line.slice(index);
    const character = line[index];

    if (remaining.startsWith("<!--")) {
      const commentEnd = line.indexOf("-->", index + 4);
      const endIndex = commentEnd >= 0 ? commentEnd + 3 : line.length;
      html += wrapSyntaxToken("comment", line.slice(index, endIndex));
      index = endIndex;
      continue;
    }

    if (remaining.startsWith("/*")) {
      const commentEnd = line.indexOf("*/", index + 2);
      const endIndex = commentEnd >= 0 ? commentEnd + 2 : line.length;
      html += wrapSyntaxToken("comment", line.slice(index, endIndex));
      index = endIndex;
      continue;
    }

    if (remaining.startsWith("//")) {
      html += wrapSyntaxToken("comment", remaining);
      break;
    }

    if (character === "#" && (language === "" || hashCommentLanguages.has(language))) {
      html += wrapSyntaxToken("comment", remaining);
      break;
    }

    if (character === "\"" || character === "'" || character === "`") {
      const endIndex = readQuotedToken(line, index);
      html += wrapSyntaxToken("string", line.slice(index, endIndex));
      index = endIndex;
      continue;
    }

    if (/\d/.test(character)) {
      const numberMatch = remaining.match(/^\d[\d_]*(?:\.\d[\d_]*)?(?:e[+-]?\d+)?/i);

      if (numberMatch) {
        html += wrapSyntaxToken("number", numberMatch[0]);
        index += numberMatch[0].length;
        continue;
      }
    }

    if (isIdentifierStart(character)) {
      let endIndex = index + 1;

      while (endIndex < line.length && isIdentifierPart(line[endIndex])) {
        endIndex += 1;
      }

      const word = line.slice(index, endIndex);
      const nextVisibleCharacter = line.slice(endIndex).match(/\S/)?.[0];

      if (codeKeywords.has(word)) {
        html += wrapSyntaxToken("keyword", word);
      } else if (codeLiterals.has(word)) {
        html += wrapSyntaxToken("literal", word);
      } else if (nextVisibleCharacter === "(") {
        html += wrapSyntaxToken("function", word);
      } else {
        html += escapeHtml(word);
      }

      index = endIndex;
      continue;
    }

    if (codeOperatorCharacters.has(character)) {
      html += wrapSyntaxToken("operator", character);
      index += 1;
      continue;
    }

    html += escapeHtml(character);
    index += 1;
  }

  return html;
}

function highlightExecutionCode(code: string, language?: string) {
  const normalizedLanguage = normalizeCodeLanguage(language);
  return code
    .replace(/\r\n/g, "\n")
    .split("\n")
    .map((line) => highlightCodeLine(line, normalizedLanguage))
    .join("\n");
}

function hasContextUsage(message: ChatMessage) {
  return Number.isFinite(message.contextUsedTokens) && Number.isFinite(message.contextLimitTokens);
}

function formatTokenCount(value?: number) {
  if (!Number.isFinite(value)) {
    return "-";
  }

  const tokens = Math.max(0, Math.round(value ?? 0));

  if (tokens >= 1_000_000) {
    return `${Number((tokens / 1_000_000).toFixed(1))}M`;
  }

  if (tokens >= 1000) {
    return `${Number((tokens / 1000).toFixed(1))}k`;
  }

  return `${tokens}`;
}

function getContextRingStyle(message: ChatMessage) {
  const used = Math.max(0, message.contextUsedTokens ?? 0);
  const limit = Math.max(1, message.contextLimitTokens ?? 1);
  const ratio = Math.min(1, used / limit);

  return {
    "--context-deg": `${Math.round(ratio * 360)}deg`,
  };
}

function getContextPercent(message: ChatMessage) {
  const used = Math.max(0, message.contextUsedTokens ?? 0);
  const limit = Math.max(1, message.contextLimitTokens ?? 1);
  return Math.min(100, Math.max(0, (used / limit) * 100)).toFixed(1);
}

function hasCacheUsage(message: ChatMessage) {
  return (message.contextCacheHitTokens ?? 0) + (message.contextCacheMissTokens ?? 0) > 0;
}

function getCacheHitRate(message: ChatMessage) {
  const hit = Math.max(0, message.contextCacheHitTokens ?? 0);
  const miss = Math.max(0, message.contextCacheMissTokens ?? 0);
  const total = hit + miss;

  if (total <= 0) {
    return "";
  }

  return ((hit / total) * 100).toFixed(1);
}

defineExpose({
  scrollToBottom,
});
</script>

<template>
  <main class="chat-workspace">
    <header class="chat-header">
      <div class="chat-header-main">
        <div class="chat-header-info">
          <h2 class="chat-group-name">{{ activeGroup?.name }}</h2>
          <p v-if="activeGroup?.description" class="chat-group-desc">{{ activeGroup?.description }}</p>
        </div>
        <div class="chat-header-meta">
          <el-tag type="info" size="small">
            {{ t("chat.onlineMembers", { count: activeMemberCount }) }}
          </el-tag>
        </div>
      </div>
      <div class="chat-header-tools">
        <span class="workspace-label">{{ t("chat.workspace.label") }}</span>
        <el-input
          class="workspace-input"
          :model-value="workspacePath"
          :placeholder="t('chat.workspace.placeholder')"
          size="small"
          clearable
          @update:model-value="emit('update:workspacePath', String($event))"
        >
          <template #append>
            <el-button
              :icon="FolderOpened"
              size="small"
              :title="t('chat.chooseWorkspaceTitle')"
              @click="chooseWorkspacePath"
            />
          </template>
        </el-input>
        <el-button
          class="reset-session-button"
          :icon="RefreshLeft"
          size="small"
          plain
          :disabled="messages.length === 0 && !sending"
          @click="emit('resetSession')"
        >
          {{ t("chat.resetSession.label") }}
        </el-button>
      </div>
    </header>

    <section v-if="speakerQueue.length > 0" class="speaker-queue">
      <div class="speaker-queue-head">
        <span class="speaker-queue-title">{{ t("chat.queue.title") }}</span>
        <el-button
          class="speaker-queue-stop"
          :icon="CircleClose"
          circle
          plain
          type="danger"
          size="small"
          :title="t('chat.queue.stopAll')"
          :aria-label="t('chat.queue.stopAll')"
          @click="emit('stopGeneration')"
        />
      </div>
      <div class="speaker-queue-list">
        <span
          v-for="member in speakerQueue"
          :key="member.id"
          class="speaker-queue-pill"
          :style="{ '--queue-accent': member.color }"
        >
          <span class="queue-dot"></span>
          <strong>{{ member.name }}</strong>
          <span v-if="member.isAdmin" class="identity-badge admin">
            {{ t("members.adminRole") }}
          </span>
          <el-tag size="small" :type="speakerQueueStatusType[member.status]">
            {{ t(`chat.queue.status.${member.status}`) }}
          </el-tag>
        </span>
      </div>
    </section>

    <section v-if="actionablePatchProposals.length > 0" class="approval-dock">
      <button
        class="approval-toggle"
        type="button"
        :class="{ open: approvalPanelOpen }"
        @click="approvalPanelOpen = !approvalPanelOpen"
      >
        <span class="approval-toggle-copy">
          <strong>{{ t("patch.panelTitle") }}</strong>
          <span>{{ approvalPanelOpen ? t("patch.hidePanel") : t("patch.showPanel") }}</span>
        </span>
        <span class="approval-count">
          {{ t("patch.pendingCount", { count: pendingPatchCount }) }}
        </span>
      </button>

      <PatchApprovalPanel
        v-if="approvalPanelOpen"
        :patch-proposals="actionablePatchProposals"
        @update-patch-status="(proposalId, status) => emit('updatePatchStatus', proposalId, status)"
        @remove-patch-proposal="(proposalId) => emit('removePatchProposal', proposalId)"
      />
    </section>

    <section ref="messagesPanel" class="messages-panel" @scroll="updateStickToBottom">
      <div ref="messagesStack" class="messages-stack">
        <article
          v-for="message in messages"
          :key="message.id"
          class="message-row"
          :class="[message.role, message.status]"
          :style="{ '--accent': message.color }"
        >
          <div class="message-meta">
            <span class="message-avatar" :style="{ '--avatar-accent': message.color }">
              <img v-if="getMessageAvatarSrc(message)" :src="getMessageAvatarSrc(message)" alt="" />
              <span v-else>{{ getInitial(message.modelName) }}</span>
            </span>
            <div class="message-heading">
              <div class="message-title">
                <strong>{{ message.modelName }}</strong>
                <span v-if="message.role === 'user'" class="identity-badge owner">
                  {{ t("common.ownerRole") }}
                </span>
                <span v-else-if="isAdminMessage(message)" class="identity-badge admin">
                  {{ t("members.adminRole") }}
                </span>
                <span v-if="message.providerName">{{ message.providerName }}</span>
              </div>
              <div class="message-detail-list">
                <span v-if="getMessageApiModel(message)">
                  {{ t("chat.messageMeta.model", { model: getMessageApiModel(message) }) }}
                </span>
                <span v-if="message.role === 'assistant'">
                  {{ t("chat.messageMeta.reasoning", { effort: getReasoningLabel(message) }) }}
                </span>
                <span v-if="formatDuration(message.durationMs)">
                  {{ t("chat.messageMeta.duration", { duration: formatDuration(message.durationMs) }) }}
                </span>
              </div>
            </div>
            <div class="message-meta-actions">
              <el-button
                v-if="shouldShowExecutionToggle(message)"
                class="execution-toggle"
                :class="{ open: isExecutionOpen(message) }"
                :icon="Tools"
                size="small"
                type="success"
                :plain="!isExecutionOpen(message)"
                :data-count="getExecutionCount(message)"
                :data-label="`${getExecutionToggleLabel(message)} · ${getExecutionCount(message)}`"
                :title="getExecutionToggleLabel(message)"
                :aria-label="`${getExecutionToggleLabel(message)} · ${getExecutionCount(message)}`"
                :aria-expanded="isExecutionOpen(message)"
                @click="toggleExecutionPanel(message)"
              >
                {{ t("chat.execution.title") }} · {{ getExecutionCount(message) }}
              </el-button>
              <span class="status-pill" :class="message.status">
                {{ statusText[message.status] }}
              </span>
              <time>{{ message.time }}</time>
            </div>
          </div>

          <div class="message-body" v-html="renderMarkdown(message.content)"></div>

          <section
            v-if="hasExecutionItems(message) && isExecutionOpen(message)"
            class="execution-panel"
          >
            <div
              v-for="block in getExecutionBlocks(message)"
              :key="block.id"
              class="execution-line"
              :class="[block.item.kind, block.item.status, block.type]"
            >
              <span class="execution-icon">
                <el-icon>
                  <component :is="getExecutionIcon(block.item)" />
                </el-icon>
              </span>
              <span class="execution-kind">{{ getExecutionKindLabel(block.item) }}</span>
              <div class="execution-copy">
                <div v-if="block.type === 'code'" class="execution-code-block">
                  <div class="execution-code-header">
                    {{ formatExecutionCodeLanguage(block.language) }}
                  </div>
                  <pre class="execution-code-pre"><code v-html="highlightExecutionCode(block.text, block.language)"></code></pre>
                </div>
                <div v-else class="execution-markdown" v-html="renderExecutionMarkdown(block.text)"></div>
                <details
                  v-if="block.type === 'markdown' && block.item.detail"
                  class="execution-detail"
                  @toggle="scheduleFollowScroll()"
                >
                  <summary>{{ t("chat.execution.detail") }}</summary>
                  <pre>{{ block.item.detail }}</pre>
                </details>
              </div>
            </div>
          </section>

          <div class="message-status-bar" :class="message.status">
            <div class="message-activity-feed">
              <template v-if="getLatestExecutionItems(message).length > 0">
                <span
                  v-for="item in getLatestExecutionItems(message)"
                  :key="item.id"
                  class="activity-chip"
                  :class="[item.kind, item.status]"
                  :title="item.text"
                >
                  <el-icon>
                    <component :is="getExecutionIcon(item)" />
                  </el-icon>
                  <span>{{ item.text }}</span>
                </span>
              </template>
              <span v-else class="activity-empty">{{ statusText[message.status] }}</span>
            </div>

            <el-tooltip
              v-if="hasContextUsage(message)"
              placement="top"
              effect="light"
              :show-after="180"
            >
              <template #content>
                <div class="context-tooltip">
                  <strong>
                    {{ t("chat.context.used", {
                      used: formatTokenCount(message.contextUsedTokens),
                      total: formatTokenCount(message.contextLimitTokens),
                    }) }}
                  </strong>
                  <span>{{ t("chat.context.percent", { percent: getContextPercent(message) }) }}</span>
                  <span v-if="Number.isFinite(message.contextPromptTokens)">
                    {{ t("chat.context.prompt", { tokens: formatTokenCount(message.contextPromptTokens) }) }}
                  </span>
                  <span v-if="Number.isFinite(message.contextCompletionTokens)">
                    {{ t("chat.context.completion", { tokens: formatTokenCount(message.contextCompletionTokens) }) }}
                  </span>
                  <span v-if="hasCacheUsage(message)">
                    {{ t("chat.context.cache", {
                      rate: getCacheHitRate(message),
                      hit: formatTokenCount(message.contextCacheHitTokens),
                      miss: formatTokenCount(message.contextCacheMissTokens),
                    }) }}
                  </span>
                </div>
              </template>
              <span class="context-meter" :style="getContextRingStyle(message)">
                <span class="context-ring"></span>
                <span class="context-token-label">{{ formatTokenCount(message.contextUsedTokens) }}</span>
                <span v-if="hasCacheUsage(message)" class="context-cache-label">
                  {{ t("chat.context.cacheShort", { rate: getCacheHitRate(message) }) }}
                </span>
              </span>
            </el-tooltip>
          </div>

          <div class="message-reactions">
            <span class="reaction-pill agree" :class="{ complete: allPeersAgreed(message) }">
              {{ getAgreeLabel(message) }}
            </span>
            <span class="reaction-pill supplement">
              {{ t("chat.supplement", { count: (message.supplementMemberIds ?? []).length }) }}
            </span>
            <span class="reaction-pill disagree">
              {{ t("chat.disagree", { count: (message.disagreeMemberIds ?? []).length }) }}
            </span>
          </div>
        </article>
        <div ref="messagesEnd" class="messages-end" aria-hidden="true"></div>
      </div>
    </section>

    <footer ref="composerFooter" class="composer">
      <div v-if="mentionOpen && mentionCandidates.length > 0" class="mention-menu">
        <button
          v-for="member in mentionCandidates"
          :key="member.id"
          type="button"
          @click="insertMention(member)"
        >
          <span class="mention-avatar" :style="{ background: member.color }">
            {{ member.name.trim().slice(0, 1) || "?" }}
          </span>
          <span>{{ member.name }}</span>
          <span v-if="member.isAdmin" class="identity-badge admin">
            {{ t("members.adminRole") }}
          </span>
        </button>
      </div>

      <el-input
        :model-value="composer"
        type="textarea"
        :autosize="{ minRows: 3, maxRows: 7 }"
        resize="none"
        :placeholder="t('chat.composerPlaceholder')"
        @update:model-value="emit('update:composer', String($event))"
        @keydown.enter.exact.prevent="emit('sendMessage')"
      />

      <el-button
        v-if="sending"
        type="danger"
        plain
        @click="emit('stopGeneration')"
      >
        {{ t("chat.stop") }}
      </el-button>

      <el-button
        type="primary"
        :loading="sending"
        :disabled="!canSend"
        :icon="Promotion"
        @click="emit('sendMessage')"
      >
        {{ t("chat.sendToMembers", { count: activeMemberCount }) }}
      </el-button>
    </footer>
  </main>
</template>

<style scoped>
.messages-panel {
  display: block;
  overflow-anchor: none;
}

.messages-stack {
  display: flex;
  min-height: 100%;
  flex-direction: column;
  gap: 12px;
}

.message-row {
  flex: 0 0 auto;
}

.messages-end {
  width: 100%;
  height: 1px;
  flex: 0 0 1px;
}

.message-row.thinking {
  position: relative;
  overflow: hidden;
  border-color: transparent;
}

.message-row.thinking > * {
  position: relative;
  z-index: 1;
}

.message-row.thinking::before {
  position: absolute;
  z-index: 0;
  inset: 0;
  border-radius: 8px;
  padding: 1px;
  animation: thinking-border-flow 5.5s linear infinite;
  background: linear-gradient(120deg, #b7dfc7, #f0d386, #91c9b0, #d8efe1);
  background-size: 320% 320%;
  content: "";
  pointer-events: none;
  -webkit-mask:
    linear-gradient(#000 0 0) content-box,
    linear-gradient(#000 0 0);
  -webkit-mask-composite: xor;
  mask-composite: exclude;
}

@keyframes thinking-border-flow {
  0% {
    background-position: 0% 50%;
  }

  100% {
    background-position: 320% 50%;
  }
}

.message-meta-actions {
  display: inline-flex;
  flex: 0 0 auto;
  align-items: center;
  gap: 8px;
  margin-left: auto;
}

.execution-toggle {
  height: 28px;
  border-color: #b9d8c7;
  border-radius: 999px;
  padding: 5px 9px;
  color: #256b50;
  background: #eef8f2;
  font-weight: 800;
  box-shadow: 0 4px 10px rgba(47, 122, 97, 0.1);
}

.execution-toggle.open {
  color: #ffffff;
  background: #2f7a61;
}

.execution-toggle::after {
  content: attr(title) " (" attr(data-count) ")";
}

.execution-toggle :deep(.el-icon) {
  font-size: 13px;
}

.execution-toggle :deep(> span) {
  display: none;
}

.message-status-bar {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: 10px;
  min-height: 34px;
  margin-top: 10px;
  padding: 7px 0 0;
  border-top: 1px solid #edf1ee;
}

.message-activity-feed {
  display: flex;
  min-width: 0;
  flex: 1;
  flex-wrap: wrap;
  gap: 6px;
}

.activity-empty {
  display: inline-flex;
  align-items: center;
  min-height: 24px;
  color: #8a958e;
  font-size: 12px;
}

.activity-chip {
  display: inline-flex;
  max-width: min(100%, 520px);
  min-height: 24px;
  align-items: center;
  gap: 5px;
  overflow: hidden;
  border: 1px solid #dfe8e2;
  border-radius: 999px;
  padding: 3px 8px;
  color: #526058;
  background: #f8faf8;
  font-size: 12px;
  line-height: 1.2;
}

.activity-detail-chip {
  display: block;
  padding: 0;
  border-radius: 8px;
}

.activity-detail-chip summary {
  display: inline-flex;
  max-width: min(100%, 520px);
  min-height: 24px;
  align-items: center;
  gap: 5px;
  overflow: hidden;
  padding: 3px 8px;
  cursor: pointer;
  list-style: none;
}

.activity-detail-chip summary::-webkit-details-marker {
  display: none;
}

.activity-detail-chip[open] {
  flex-basis: min(100%, 620px);
  max-width: min(100%, 620px);
}

.activity-chip .el-icon {
  flex: 0 0 auto;
  font-size: 13px;
}

.activity-chip span:last-child,
.activity-detail-chip summary span:last-child {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.activity-chip.tool {
  border-color: #bed9cb;
  color: #24634f;
  background: #eef8f2;
}

.activity-chip.reasoning {
  border-color: #d8e2ee;
  color: #355d80;
  background: #f3f8fd;
}

.activity-chip.status.running {
  border-color: #efd7ad;
  color: #9a650c;
  background: #fff8ea;
}

.activity-chip.done {
  border-color: #cfe3d7;
  color: #2b7359;
  background: #f0faf4;
}

.activity-chip.error {
  border-color: #f0c7c7;
  color: #a33d3d;
  background: #fff0f0;
}

.activity-detail {
  max-height: 240px;
  overflow: auto;
  margin: 0;
  border-top: 1px solid rgba(47, 122, 97, 0.16);
  padding: 8px 10px;
  color: #304039;
  background: #fbfdfb;
  font-family: "Cascadia Code", "Fira Code", Consolas, monospace;
  font-size: 11px;
  line-height: 1.55;
  white-space: pre-wrap;
  word-break: break-word;
}

.context-meter {
  display: inline-flex;
  flex: 0 0 auto;
  align-items: center;
  gap: 6px;
  min-height: 28px;
  border: 1px solid #dfe8e2;
  border-radius: 999px;
  padding: 3px 8px 3px 4px;
  color: #405047;
  background: #ffffff;
  cursor: default;
}

.context-ring {
  position: relative;
  width: 24px;
  height: 24px;
  flex: 0 0 auto;
  border-radius: 50%;
  background: conic-gradient(#2f7a61 var(--context-deg), #e4ebe6 0deg);
}

.context-ring::after {
  position: absolute;
  inset: 4px;
  border-radius: 50%;
  background: #ffffff;
  content: "";
}

.context-token-label {
  font-size: 12px;
  font-weight: 800;
  line-height: 1;
}

.context-cache-label {
  border-left: 1px solid #e1e9e4;
  padding-left: 6px;
  color: #2f7a61;
  font-size: 11px;
  font-weight: 800;
  line-height: 1;
}

.context-tooltip {
  display: grid;
  gap: 4px;
  color: #334139;
  font-size: 12px;
}

.context-tooltip strong {
  color: #1f2c25;
}

.execution-panel {
  display: grid;
  gap: 8px;
  margin-top: 10px;
  border: 1px solid #dfe9e3;
  border-radius: 8px;
  background: #f7faf8;
  padding: 10px;
}

.execution-line {
  display: grid;
  grid-template-columns: 24px minmax(46px, max-content) minmax(0, 1fr);
  align-items: start;
  gap: 8px;
  color: #435149;
  font-size: 12px;
  line-height: 1.55;
}

.execution-icon {
  display: inline-grid;
  width: 22px;
  height: 22px;
  place-items: center;
  border: 1px solid #dfe8e2;
  border-radius: 999px;
  color: #5a6a61;
  background: #ffffff;
}

.execution-line.tool .execution-icon {
  border-color: #bed9cb;
  color: #24634f;
  background: #eef8f2;
}

.execution-line.reasoning .execution-icon {
  border-color: #d8e2ee;
  color: #355d80;
  background: #f3f8fd;
}

.execution-line.error .execution-icon {
  border-color: #f0c7c7;
  color: #a33d3d;
  background: #fff0f0;
}

.execution-kind {
  padding-top: 2px;
  color: #738078;
  font-weight: 800;
  white-space: nowrap;
}

.execution-copy {
  min-width: 0;
}

.execution-markdown {
  overflow-wrap: anywhere;
  color: #33413a;
}

.execution-markdown :deep(p),
.execution-markdown :deep(ul),
.execution-markdown :deep(ol),
.execution-markdown :deep(pre),
.execution-markdown :deep(blockquote) {
  margin: 0 0 8px;
}

.execution-markdown :deep(:first-child) {
  margin-top: 0;
}

.execution-markdown :deep(:last-child) {
  margin-bottom: 0;
}

.execution-markdown :deep(ul),
.execution-markdown :deep(ol) {
  padding-left: 18px;
}

.execution-markdown :deep(code) {
  border-radius: 5px;
  padding: 2px 5px;
  color: #365f51;
  background: #edf4ef;
  font-family: "Cascadia Code", "Fira Code", Consolas, monospace;
  font-size: 12px;
}

.execution-markdown :deep(pre) {
  overflow: auto;
  border: 1px solid #dbe4dd;
  border-radius: 8px;
  padding: 10px;
  background: #fbfcfd;
}

.execution-markdown :deep(pre code) {
  padding: 0;
  color: #27332d;
  background: transparent;
}

.execution-code-block {
  min-width: 0;
  overflow: hidden;
  border: 1px solid #d8e3dc;
  border-radius: 8px;
  background: #fbfcfd;
  box-shadow: 0 6px 18px rgba(31, 43, 36, 0.05);
}

.execution-code-header {
  display: flex;
  min-height: 28px;
  align-items: center;
  border-bottom: 1px solid #e1e9e4;
  padding: 5px 10px;
  color: #557064;
  background: #f2f7f4;
  font-family: "Cascadia Code", "Fira Code", Consolas, monospace;
  font-size: 11px;
  font-weight: 800;
  line-height: 1.2;
  text-transform: uppercase;
}

.execution-code-pre {
  overflow: auto;
  max-height: 420px;
  margin: 0;
  padding: 11px 12px;
  color: #25322b;
  background: #fbfcfd;
  font-family: "Cascadia Code", "Fira Code", Consolas, monospace;
  font-size: 12px;
  line-height: 1.6;
  tab-size: 2;
  white-space: pre;
}

.execution-code-pre code {
  padding: 0;
  color: inherit;
  background: transparent;
  font: inherit;
}

.execution-code-pre :deep(.syntax-keyword) {
  color: #245cbf;
  font-weight: 800;
}

.execution-code-pre :deep(.syntax-string) {
  color: #257247;
}

.execution-code-pre :deep(.syntax-comment) {
  color: #7a867e;
  font-style: italic;
}

.execution-code-pre :deep(.syntax-number) {
  color: #ad5f00;
}

.execution-code-pre :deep(.syntax-literal) {
  color: #7b4ab8;
  font-weight: 700;
}

.execution-code-pre :deep(.syntax-function) {
  color: #087987;
  font-weight: 700;
}

.execution-code-pre :deep(.syntax-operator) {
  color: #a34444;
}

.execution-detail {
  margin-top: 6px;
  border: 1px solid #dfe8e2;
  border-radius: 8px;
  background: #ffffff;
}

.execution-detail summary {
  min-height: 28px;
  padding: 5px 9px;
  cursor: pointer;
  color: #2f6f58;
  font-weight: 800;
  list-style: none;
}

.execution-detail summary::-webkit-details-marker {
  display: none;
}

.execution-detail pre {
  max-height: 260px;
  overflow: auto;
  margin: 0;
  border-top: 1px solid #e4ede7;
  padding: 8px 10px;
  color: #304039;
  background: #fbfdfb;
  font-family: "Cascadia Code", "Fira Code", Consolas, monospace;
  font-size: 11px;
  line-height: 1.55;
  white-space: pre-wrap;
  word-break: break-word;
}

.approval-dock {
  display: grid;
  gap: 10px;
  padding: 10px 18px;
  border-bottom: 1px solid #e1e7e2;
  background: #fbfcfb;
}

.approval-toggle {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  width: min(920px, 100%);
  min-height: 52px;
  border: 1px solid #dbe6de;
  border-radius: 8px;
  padding: 9px 12px;
  color: #26322b;
  background: #ffffff;
  cursor: pointer;
  text-align: left;
  transition: border-color 0.18s, background 0.18s, box-shadow 0.18s;
}

.approval-toggle:hover,
.approval-toggle.open {
  border-color: #c8d8ce;
  background: #f8fbf9;
  box-shadow: 0 6px 18px rgba(31, 43, 36, 0.06);
}

.approval-toggle-copy {
  display: grid;
  min-width: 0;
  gap: 2px;
}

.approval-toggle-copy strong {
  color: #1d2a22;
  font-size: 14px;
}

.approval-toggle-copy span {
  overflow: hidden;
  color: #7a867e;
  font-size: 12px;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.approval-count {
  display: inline-flex;
  flex: 0 0 auto;
  align-items: center;
  min-height: 28px;
  border: 1px solid #f1d7a6;
  border-radius: 8px;
  padding: 3px 10px;
  color: #b06f09;
  background: #fff8ea;
  font-size: 13px;
  font-weight: 800;
}

.speaker-queue {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 8px 12px;
  border-bottom: 1px solid #e1e7e2;
  background: #f8fbf9;
}

.speaker-queue-head {
  display: inline-flex;
  flex: 0 0 auto;
  align-items: center;
  gap: 8px;
}

.speaker-queue-title {
  flex: 0 0 auto;
  color: #647169;
  font-size: 12px;
  font-weight: 700;
}

.speaker-queue-stop {
  flex: 0 0 auto;
}

.speaker-queue-list {
  display: flex;
  min-width: 0;
  flex: 1;
  flex-wrap: wrap;
  gap: 6px;
}

.speaker-queue-pill {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  max-width: 220px;
  padding: 4px 6px;
  border: 1px solid #dbe5de;
  border-radius: 8px;
  background: #ffffff;
  font-size: 12px;
}

.speaker-queue-pill strong {
  min-width: 0;
  overflow: hidden;
  color: #26322b;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.identity-badge {
  display: inline-flex;
  flex: 0 0 auto;
  align-items: center;
  justify-content: center;
  min-height: 20px;
  border: 1px solid currentColor;
  border-radius: 999px;
  padding: 1px 7px;
  font-size: 11px;
  font-weight: 800;
  line-height: 1;
}

.identity-badge.admin {
  color: #2f7a61;
  background: #eef8f2;
}

.identity-badge.owner {
  color: #9a6a13;
  background: #fff6dc;
}

.message-title .identity-badge {
  color: #2f7a61;
  font-size: 11px;
}

.message-title .identity-badge.owner {
  color: #9a6a13;
}

.queue-dot {
  width: 8px;
  height: 8px;
  flex: 0 0 auto;
  border-radius: 50%;
  background: var(--queue-accent);
}

.mention-menu {
  display: flex;
  max-height: 148px;
  flex-direction: column;
  gap: 4px;
  overflow: auto;
  padding: 6px;
  border: 1px solid #d9e2dc;
  border-radius: 8px;
  background: #ffffff;
  box-shadow: 0 10px 24px rgba(31, 43, 36, 0.12);
}

.mention-menu button {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 7px 8px;
  border: 0;
  border-radius: 6px;
  color: #24312a;
  background: transparent;
  cursor: pointer;
  text-align: left;
}

.mention-menu button > span:nth-child(2) {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.mention-menu button:hover {
  background: #eef5f0;
}

.mention-avatar {
  display: grid;
  width: 24px;
  height: 24px;
  flex: 0 0 auto;
  place-items: center;
  border-radius: 50%;
  color: #ffffff;
  font-size: 12px;
  font-weight: 800;
}
</style>
