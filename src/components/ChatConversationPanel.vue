<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { open } from "@tauri-apps/plugin-dialog";
import { ElMessage } from "element-plus";
import {
  CircleClose,
  CopyDocument,
  FolderOpened,
  Promotion,
  RefreshLeft,
  Tools,
} from "@element-plus/icons-vue";
import { Wifi } from "@lucide/vue";
import { useI18n } from "vue-i18n";
import PatchApprovalPanel from "./PatchApprovalPanel.vue";
import { getAvatarSrc } from "../utils/avatar";
import type {
  AgentModel,
  ChatGroup,
  ChatMessage,
  ChatMessageContentSegment,
  ChatMessageExecutionItem,
  PatchApprovalStatus,
} from "../stores/settings";

export type SpeakerQueueStatus =
  | "queued"
  | "checking"
  | "waiting"
  | "speaking"
  | "voting"
  | "voted"
  | "consensus"
  | "followup";

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

interface ExecutionRenderSegment {
  id: string;
  items: ChatMessageExecutionItem[];
  blocks: ExecutionRenderBlock[];
  startedAt?: number;
  endedAt?: number;
}

interface MessageTimelineEntry {
  id: string;
  order: number;
  timestamp?: number;
  content?: ChatMessageContentSegment;
  execution?: ExecutionRenderSegment;
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
const executionSegmentOpen = ref<Record<string, boolean>>({});
const failedMentionAvatars = ref<Set<string>>(new Set());
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

function getMentionAvatarSrc(member: AgentModel) {
  const source = getAvatarSrc(member.avatar);
  if (!source || failedMentionAvatars.value.has(`${member.id}:${source}`)) {
    return "";
  }

  return source;
}

function markMentionAvatarFailed(member: AgentModel) {
  const source = getAvatarSrc(member.avatar);
  if (!source) {
    return;
  }

  const nextFailed = new Set(failedMentionAvatars.value);
  nextFailed.add(`${member.id}:${source}`);
  failedMentionAvatars.value = nextFailed;
}

const speakerQueueStatusType: Record<
  SpeakerQueueStatus,
  "info" | "warning" | "success" | "primary" | "danger"
> = {
  queued: "info",
  checking: "warning",
  waiting: "info",
  speaking: "success",
  voting: "warning",
  voted: "success",
  consensus: "primary",
  followup: "warning",
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
const htmlCommentOpenToken = "<!" + "--";
const htmlCommentCloseToken = "--" + ">";

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
        (message.contentSegments ?? [])
          .map((segment) => `${segment.id}:${segment.createdAt ?? 0}:${segment.updatedAt ?? 0}:${segment.text.length}`)
          .join(","),
        message.durationMs ?? 0,
        message.contextUsedTokens ?? 0,
        message.contextCacheHitTokens ?? 0,
        message.contextCacheMissTokens ?? 0,
        (message.thoughtSteps ?? []).join("\n").length,
        (message.activityItems ?? [])
          .map((item) => `${item.id}:${item.status}:${item.text.length}:${item.detail?.length ?? 0}`)
          .join(","),
        (message.executionItems ?? [])
          .map(
            (item) =>
              `${item.id}:${item.segmentId ?? ""}:${item.kind}:${item.status}:${item.createdAt ?? 0}:${item.text.length}:${item.detail?.length ?? 0}`,
          )
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
    const nextOpenState = { ...executionSegmentOpen.value };
    const activeMessageIds = new Set(props.messages.map((message) => message.id));
    const activeSegmentIds = new Set(
      props.messages.flatMap((message) => getExecutionSegments(message).map((segment) => segment.id)),
    );
    let changed = false;

    for (const message of props.messages) {
      const previousStatus = previousMessageStatuses.get(message.id);

      if (previousStatus === "thinking" && message.status !== "thinking") {
        for (const segment of getExecutionSegments(message)) {
          if (nextOpenState[segment.id] !== false) {
            nextOpenState[segment.id] = false;
            changed = true;
          }
        }
      }

      previousMessageStatuses.set(message.id, message.status);
    }

    for (const messageId of previousMessageStatuses.keys()) {
      if (!activeMessageIds.has(messageId)) {
        previousMessageStatuses.delete(messageId);
        changed = true;
      }
    }

    for (const segmentId of Object.keys(nextOpenState)) {
      if (!activeSegmentIds.has(segmentId)) {
        delete nextOpenState[segmentId];
        changed = true;
      }
    }

    if (changed) {
      executionSegmentOpen.value = nextOpenState;
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

function buildExecutionBlocks(items: ChatMessageExecutionItem[]): ExecutionRenderBlock[] {
  const blocks: ExecutionRenderBlock[] = [];
  let openCodeBlock: { item: ChatMessageExecutionItem; language: string; lines: string[] } | null = null;

  items.forEach((item, itemIndex) => {
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

function getExecutionItemSegmentId(message: ChatMessage, item: ChatMessageExecutionItem) {
  return item.segmentId ?? `${message.id}-legacy-execution`;
}

function getExecutionTimestamp(item: ChatMessageExecutionItem) {
  const timestamp = item.createdAt;
  return typeof timestamp === "number" && timestamp > 946684800000 ? timestamp : undefined;
}

function getExecutionSegments(message: ChatMessage): ExecutionRenderSegment[] {
  const draftSegments: Array<{ id: string; items: ChatMessageExecutionItem[] }> = [];
  const draftById = new Map<string, { id: string; items: ChatMessageExecutionItem[] }>();

  getExecutionItems(message).forEach((item) => {
    const segmentId = getExecutionItemSegmentId(message, item);
    let draft = draftById.get(segmentId);

    if (!draft) {
      draft = { id: segmentId, items: [] };
      draftById.set(segmentId, draft);
      draftSegments.push(draft);
    }

    draft.items.push(item);
  });

  return draftSegments
    .map((draft) => {
      const timestamps = draft.items
        .map(getExecutionTimestamp)
        .filter((timestamp): timestamp is number => typeof timestamp === "number");

      return {
        id: draft.id,
        items: draft.items,
        blocks: buildExecutionBlocks(draft.items),
        startedAt: timestamps[0],
        endedAt: timestamps[timestamps.length - 1],
      };
    })
    .filter((segment) => segment.blocks.length > 0);
}

function getContentSegments(message: ChatMessage): ChatMessageContentSegment[] {
  const segments = (message.contentSegments ?? []).filter((segment) => segment.text.trim().length > 0);

  if (segments.length > 0) {
    return segments;
  }

  return message.content.trim()
    ? [
        {
          id: `${message.id}-content`,
          text: message.content,
        },
      ]
    : [];
}

function getMessageTimeline(message: ChatMessage): MessageTimelineEntry[] {
  const contentEntries: MessageTimelineEntry[] = getContentSegments(message).map((content, index) => ({
    id: content.id,
    order: index * 2,
    timestamp: content.createdAt,
    content,
  }));
  const executionEntries: MessageTimelineEntry[] = getExecutionSegments(message).map((execution, index) => ({
    id: execution.id,
    order: index * 2 + 1,
    timestamp: execution.startedAt,
    execution,
  }));

  return [...contentEntries, ...executionEntries].sort((left, right) => {
    if (left.timestamp !== undefined && right.timestamp !== undefined && left.timestamp !== right.timestamp) {
      return left.timestamp - right.timestamp;
    }

    if (left.timestamp !== undefined && right.timestamp === undefined) {
      return 1;
    }

    if (left.timestamp === undefined && right.timestamp !== undefined) {
      return -1;
    }

    return left.order - right.order;
  });
}

function isExecutionSegmentOpen(message: ChatMessage, segment: ExecutionRenderSegment) {
  return executionSegmentOpen.value[segment.id] ?? message.status === "thinking";
}

function collapseExecutionPanel(messageId: string) {
  if (!messageId) {
    return;
  }

  const message = props.messages.find((item) => item.id === messageId);

  if (!message) {
    return;
  }

  const nextOpenState = { ...executionSegmentOpen.value };
  let changed = false;

  for (const segment of getExecutionSegments(message)) {
    if (nextOpenState[segment.id] !== false) {
      nextOpenState[segment.id] = false;
      changed = true;
    }
  }

  if (changed) {
    executionSegmentOpen.value = nextOpenState;
    scheduleFollowScroll(true);
  }
}

function toggleExecutionSegment(message: ChatMessage, segment: ExecutionRenderSegment) {
  executionSegmentOpen.value = {
    ...executionSegmentOpen.value,
    [segment.id]: !isExecutionSegmentOpen(message, segment),
  };
  scheduleFollowScroll();
}

function getExecutionSegmentToggleLabel(message: ChatMessage, segment: ExecutionRenderSegment) {
  return isExecutionSegmentOpen(message, segment)
    ? t("chat.execution.collapse")
    : t("chat.execution.expand");
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
  if (item.kind === "network") {
    return Wifi;
  }

  return item.kind === "tool" ? Tools : RefreshLeft;
}

function getExecutionKindLabel(item: ChatMessageExecutionItem) {
  return t(`chat.execution.kind.${item.kind}`);
}

function formatExecutionTimestamp(timestamp?: number) {
  if (!Number.isFinite(timestamp)) {
    return "";
  }

  return new Intl.DateTimeFormat(undefined, {
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
  }).format(new Date(timestamp ?? 0));
}

function formatExecutionSegmentTime(segment: ExecutionRenderSegment) {
  const startedAt = formatExecutionTimestamp(segment.startedAt);
  const endedAt = formatExecutionTimestamp(segment.endedAt);

  if (!startedAt || !endedAt || startedAt === endedAt) {
    return startedAt;
  }

  return `${startedAt} - ${endedAt}`;
}

function getExecutionSegmentTitle(segment: ExecutionRenderSegment) {
  const kinds = new Set(segment.items.map((item) => item.kind));

  if (kinds.size === 1) {
    return getExecutionKindLabel(segment.items[0]);
  }

  return t("chat.execution.title");
}

function getExecutionSegmentIcon(segment: ExecutionRenderSegment) {
  return getExecutionIcon(segment.items[0]);
}

function getExecutionSegmentKindClass(segment: ExecutionRenderSegment) {
  const kinds = new Set(segment.items.map((item) => item.kind));
  return kinds.size === 1 ? segment.items[0].kind : "mixed";
}

function getExecutionSegmentStatus(segment: ExecutionRenderSegment) {
  return segment.items[segment.items.length - 1]?.status ?? "info";
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

    if (remaining.startsWith(htmlCommentOpenToken)) {
      const commentEnd = line.indexOf(htmlCommentCloseToken, index + htmlCommentOpenToken.length);
      const endIndex =
        commentEnd >= 0 ? commentEnd + htmlCommentCloseToken.length : line.length;
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

function fallbackCopyText(text: string) {
  const textArea = document.createElement("textarea");
  textArea.value = text;
  textArea.setAttribute("readonly", "true");
  textArea.style.position = "fixed";
  textArea.style.left = "-9999px";
  textArea.style.top = "0";
  document.body.appendChild(textArea);
  textArea.select();

  try {
    return document.execCommand("copy");
  } finally {
    document.body.removeChild(textArea);
  }
}

async function writeClipboardText(text: string) {
  if (navigator.clipboard?.writeText) {
    await navigator.clipboard.writeText(text);
    return;
  }

  if (!fallbackCopyText(text)) {
    throw new Error("Clipboard copy failed.");
  }
}

async function copyText(text: string) {
  if (!text.trim()) {
    return;
  }

  try {
    await writeClipboardText(text);
    ElMessage.success(t("chat.copy.success"));
  } catch {
    ElMessage.error(t("chat.copy.failed"));
  }
}

function formatExecutionItemForCopy(item: ChatMessageExecutionItem, index: number) {
  const title = `${index + 1}. [${getExecutionKindLabel(item)}] ${item.text}`.trim();
  const detail = item.detail?.trim();

  return detail ? `${title}\n\n${detail}` : title;
}

function getMessageCopyText(message: ChatMessage) {
  const sections = [`# ${message.modelName}`];
  const meta = [
    props.statusText[message.status],
    getMessageApiModel(message),
    message.time,
  ].filter(Boolean);

  if (meta.length > 0) {
    sections.push(meta.join(" · "));
  }

  if (message.content.trim()) {
    sections.push(`## ${t("chat.copy.generatedText")}\n${message.content.trim()}`);
  }

  const executionItems = getExecutionItems(message);

  if (executionItems.length > 0) {
    sections.push(
      `## ${t("chat.execution.title")}\n${executionItems
        .map((item, index) => formatExecutionItemForCopy(item, index))
        .join("\n\n")}`,
    );
  }

  return sections.join("\n\n");
}

function copyMessage(message: ChatMessage) {
  void copyText(getMessageCopyText(message));
}

function copyExecutionBlock(block: ExecutionRenderBlock) {
  void copyText(block.text);
}

function copyExecutionDetail(item: ChatMessageExecutionItem) {
  void copyText(item.detail ?? "");
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
  collapseExecutionPanel,
  scrollToBottom,
});
</script>

<template>
  <main class="chat-workspace">
    <header class="chat-header">
      <div class="chat-header-main">
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
          :class="member.status"
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

    <PatchApprovalPanel
      v-if="actionablePatchProposals.length > 0"
      :patch-proposals="actionablePatchProposals"
      @update-patch-status="(proposalId, status) => emit('updatePatchStatus', proposalId, status)"
      @remove-patch-proposal="(proposalId) => emit('removePatchProposal', proposalId)"
    />

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
              <span class="status-pill" :class="message.status">
                {{ statusText[message.status] }}
              </span>
              <time>{{ message.time }}</time>
            </div>
          </div>

          <div class="message-timeline">
            <template v-for="entry in getMessageTimeline(message)" :key="entry.id">
              <div
                v-if="entry.content"
                class="message-body message-content-segment"
                v-html="renderMarkdown(entry.content.text)"
              ></div>

              <section
                v-else-if="entry.execution"
                class="execution-panel"
              >
                <section
                  class="execution-segment"
                  :class="[
                    getExecutionSegmentKindClass(entry.execution),
                    getExecutionSegmentStatus(entry.execution),
                    { collapsed: !isExecutionSegmentOpen(message, entry.execution) },
                  ]"
                >
                  <button
                    class="execution-segment-summary"
                    type="button"
                    :title="getExecutionSegmentToggleLabel(message, entry.execution)"
                    :aria-label="getExecutionSegmentToggleLabel(message, entry.execution)"
                    :aria-expanded="isExecutionSegmentOpen(message, entry.execution)"
                    @click="toggleExecutionSegment(message, entry.execution)"
                  >
                    <span class="execution-icon">
                      <el-icon>
                        <component :is="getExecutionSegmentIcon(entry.execution)" />
                      </el-icon>
                    </span>
                    <span class="execution-segment-title">{{ getExecutionSegmentTitle(entry.execution) }}</span>
                    <span class="execution-segment-count">{{ entry.execution.blocks.length }}</span>
                    <time v-if="formatExecutionSegmentTime(entry.execution)" class="execution-segment-time">
                      {{ formatExecutionSegmentTime(entry.execution) }}
                    </time>
                  </button>

                  <div
                    v-if="isExecutionSegmentOpen(message, entry.execution)"
                    class="execution-segment-body"
                  >
                    <div
                      v-for="block in entry.execution.blocks"
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
                            <span>{{ formatExecutionCodeLanguage(block.language) }}</span>
                            <button
                              class="execution-copy-button"
                              type="button"
                              :title="t('chat.copy.copyCode')"
                              :aria-label="t('chat.copy.copyCode')"
                              @click="copyExecutionBlock(block)"
                            >
                              <el-icon><CopyDocument /></el-icon>
                            </button>
                          </div>
                          <pre class="execution-code-pre"><code v-html="highlightExecutionCode(block.text, block.language)"></code></pre>
                        </div>
                        <div v-else class="execution-markdown" v-html="renderExecutionMarkdown(block.text)"></div>
                        <details
                          v-if="block.type === 'markdown' && block.item.detail"
                          class="execution-detail"
                          @toggle="scheduleFollowScroll()"
                        >
                          <summary>
                            <span>{{ t("chat.execution.detail") }}</span>
                            <button
                              class="execution-copy-button"
                              type="button"
                              :title="t('chat.copy.copyDetail')"
                              :aria-label="t('chat.copy.copyDetail')"
                              @click.stop.prevent="copyExecutionDetail(block.item)"
                            >
                              <el-icon><CopyDocument /></el-icon>
                            </button>
                          </summary>
                          <pre>{{ block.item.detail }}</pre>
                        </details>
                      </div>
                    </div>
                  </div>
                </section>
              </section>
            </template>
          </div>

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

            <div class="message-status-actions">
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
              <button
                class="message-copy-button"
                type="button"
                :title="t('chat.copy.copyMessage')"
                :aria-label="t('chat.copy.copyMessage')"
                @click="copyMessage(message)"
              >
                <el-icon><CopyDocument /></el-icon>
              </button>
              <button
                v-if="sending && message.status === 'thinking'"
                class="message-stop-button"
                type="button"
                :title="t('chat.stop')"
                :aria-label="t('chat.stop')"
                @click="emit('stopGeneration')"
              >
                <el-icon><CircleClose /></el-icon>
              </button>
            </div>
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
      <div class="composer-input-wrap">
        <div v-if="mentionOpen && mentionCandidates.length > 0" class="mention-menu">
          <button
            v-for="member in mentionCandidates"
            :key="member.id"
            type="button"
            @click="insertMention(member)"
          >
            <span class="mention-avatar" :style="{ background: member.color }">
              <img
                v-if="getMentionAvatarSrc(member)"
                :src="getMentionAvatarSrc(member)"
                alt=""
                @error="markMentionAvatarFailed(member)"
              />
              <span v-else>{{ member.name.trim().slice(0, 1) || "?" }}</span>
            </span>
            <span class="mention-member-copy">
              <strong>{{ member.name }}</strong>
              <small>{{ member.model }}</small>
            </span>
            <span v-if="member.isAdmin" class="identity-badge admin">
              {{ t("members.adminRole") }}
            </span>
          </button>
        </div>

        <el-input
          :model-value="composer"
          type="textarea"
          :rows="4"
          resize="vertical"
          :placeholder="t('chat.composerPlaceholder')"
          @update:model-value="emit('update:composer', String($event))"
          @keydown.enter.exact.prevent="emit('sendMessage')"
        />
      </div>

      <div class="composer-actions">
        <el-button
          v-if="sending"
          type="danger"
          plain
          :icon="CircleClose"
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
      </div>
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
  overflow: visible;
  border-color: transparent;
}

.message-row.thinking > * {
  position: relative;
  z-index: 1;
}

.message-row.thinking::before {
  position: absolute;
  z-index: 1;
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

.message-row.thinking::after {
  position: absolute;
  z-index: 0;
  inset: -7px;
  border-radius: 14px;
  animation: thinking-glow-breathe 2.8s ease-in-out infinite;
  background: linear-gradient(120deg, rgba(145, 201, 176, 0.32), rgba(240, 211, 134, 0.28), rgba(183, 223, 199, 0.32));
  content: "";
  filter: blur(10px);
  opacity: 0.45;
  pointer-events: none;
}

@keyframes thinking-border-flow {
  0% {
    background-position: 0% 50%;
  }

  100% {
    background-position: 320% 50%;
  }
}

@keyframes thinking-glow-breathe {
  0%,
  100% {
    opacity: 0.28;
    transform: scale(0.992);
  }

  50% {
    opacity: 0.72;
    transform: scale(1.01);
  }
}

.message-meta-actions {
  display: inline-flex;
  flex: 0 0 auto;
  align-items: center;
  gap: 8px;
  margin-left: auto;
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

.message-status-actions {
  display: inline-flex;
  flex: 0 0 auto;
  align-items: center;
  justify-content: flex-end;
  gap: 8px;
  margin-left: auto;
}

.message-copy-button,
.message-stop-button,
.execution-copy-button {
  display: inline-grid;
  flex: 0 0 auto;
  place-items: center;
  border: 1px solid #cfe0d7;
  color: #2f6f58;
  background: #ffffff;
  cursor: pointer;
  transition: border-color 0.16s, color 0.16s, background 0.16s, box-shadow 0.16s;
}

.message-copy-button,
.message-stop-button {
  width: 28px;
  height: 28px;
  border-radius: 999px;
}

.execution-copy-button {
  width: 24px;
  height: 24px;
  border-radius: 6px;
}

.message-copy-button:hover,
.execution-copy-button:hover {
  border-color: #86bda3;
  color: #ffffff;
  background: #2f7a61;
  box-shadow: 0 4px 10px rgba(47, 122, 97, 0.16);
}

.message-stop-button {
  border-color: #f0c9c9;
  color: #b04545;
}

.message-stop-button:hover {
  border-color: #d66a6a;
  color: #ffffff;
  background: #c45656;
  box-shadow: 0 4px 10px rgba(196, 86, 86, 0.16);
}

.message-copy-button .el-icon,
.message-stop-button .el-icon,
.execution-copy-button .el-icon {
  font-size: 13px;
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

.activity-chip.interrupted {
  border-color: #efd7ad;
  color: #9a650c;
  background: #fff8ea;
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

.message-timeline {
  display: grid;
  gap: 10px;
}

.message-content-segment {
  min-width: 0;
}

.execution-panel {
  display: grid;
  gap: 8px;
  border: 1px solid #dfe9e3;
  border-radius: 8px;
  background: #f7faf8;
  padding: 10px;
}

.execution-segment {
  overflow: hidden;
  border: 1px solid #dfe8e2;
  border-radius: 8px;
  background: #ffffff;
}

.execution-segment.collapsed {
  background: #fbfdfb;
}

.execution-segment-summary {
  display: flex;
  width: 100%;
  min-height: 34px;
  align-items: center;
  gap: 8px;
  border: 0;
  padding: 6px 8px;
  color: #3c4a42;
  background: transparent;
  cursor: pointer;
  font: inherit;
  text-align: left;
}

.execution-segment-summary:hover {
  background: #f2f7f4;
}

.execution-segment-title {
  min-width: 0;
  overflow: hidden;
  color: #304039;
  font-size: 12px;
  font-weight: 800;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.execution-segment-count {
  display: inline-flex;
  flex: 0 0 auto;
  min-width: 20px;
  height: 20px;
  align-items: center;
  justify-content: center;
  border: 1px solid #dfe8e2;
  border-radius: 999px;
  padding: 0 6px;
  color: #617168;
  background: #f7faf8;
  font-size: 11px;
  font-weight: 800;
  line-height: 1;
}

.execution-segment-time {
  flex: 0 1 auto;
  margin-left: auto;
  overflow: hidden;
  color: #7a867e;
  font-size: 11px;
  font-variant-numeric: tabular-nums;
  font-weight: 700;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.execution-segment-body {
  display: grid;
  gap: 8px;
  border-top: 1px solid #edf2ee;
  padding: 8px 10px 10px;
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

.execution-line.tool .execution-icon,
.execution-segment.tool .execution-icon {
  border-color: #bed9cb;
  color: #24634f;
  background: #eef8f2;
}

.execution-line.reasoning .execution-icon,
.execution-segment.reasoning .execution-icon {
  border-color: #d8e2ee;
  color: #355d80;
  background: #f3f8fd;
}

.execution-segment.mixed .execution-icon {
  border-color: #d8e2ee;
  color: #4d6375;
  background: #f7fafc;
}

.execution-line.error .execution-icon,
.execution-segment.error .execution-icon {
  border-color: #f0c7c7;
  color: #a33d3d;
  background: #fff0f0;
}

.execution-line.interrupted .execution-icon,
.execution-segment.interrupted .execution-icon {
  border-color: #efd7ad;
  color: #9a650c;
  background: #fff8ea;
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
  justify-content: space-between;
  gap: 8px;
  border-bottom: 1px solid #e1e9e4;
  padding: 4px 6px 4px 10px;
  color: #557064;
  background: #f2f7f4;
  font-family: "Cascadia Code", "Fira Code", Consolas, monospace;
  font-size: 11px;
  font-weight: 800;
  line-height: 1.2;
  text-transform: uppercase;
}

.execution-code-header span {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
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
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  min-height: 28px;
  padding: 4px 6px 4px 9px;
  cursor: pointer;
  color: #2f6f58;
  font-weight: 800;
  list-style: none;
}

.execution-detail summary > span {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
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

.speaker-queue-pill.checking .queue-dot,
.speaker-queue-pill.speaking .queue-dot,
.speaker-queue-pill.voting .queue-dot,
.speaker-queue-pill.consensus .queue-dot,
.speaker-queue-pill.followup .queue-dot {
  animation: queue-dot-pulse 1.45s ease-in-out infinite;
}

@keyframes queue-dot-pulse {
  0%,
  100% {
    box-shadow: 0 0 0 0 color-mix(in srgb, var(--queue-accent), transparent 62%);
    opacity: 0.72;
  }

  50% {
    box-shadow: 0 0 0 5px color-mix(in srgb, var(--queue-accent), transparent 86%);
    opacity: 1;
  }
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

.composer {
  display: grid;
  gap: 10px;
  padding: 12px 18px 14px;
  border-top: 1px solid #dfe7e1;
  position: relative;
  background: #fbfcfb;
  overflow: visible;
}

.composer-input-wrap {
  position: relative;
  min-width: 0;
  width: 100%;
}

.composer-input-wrap :deep(.el-textarea__inner) {
  min-height: 96px;
  max-height: 42vh;
  resize: vertical;
}

.composer-actions {
  display: flex;
  flex-wrap: wrap;
  justify-content: flex-end;
  gap: 10px;
  min-width: 0;
}

.composer-actions :deep(.el-button) {
  margin-left: 0;
}

.mention-menu {
  position: absolute;
  z-index: 20;
  right: 0;
  bottom: calc(100% + 8px);
  left: 0;
  display: grid;
  max-height: 248px;
  gap: 6px;
  overflow: auto;
  padding: 8px;
  border: 1px solid #d9e2dc;
  border-radius: 8px;
  background: #ffffff;
  box-shadow: 0 16px 36px rgba(31, 43, 36, 0.16);
}

.mention-menu button {
  display: grid;
  grid-template-columns: 34px minmax(0, 1fr) max-content;
  align-items: center;
  gap: 10px;
  min-height: 48px;
  padding: 7px 9px;
  border: 1px solid transparent;
  border-radius: 6px;
  color: #24312a;
  background: transparent;
  cursor: pointer;
  text-align: left;
}

.mention-member-copy {
  display: grid;
  min-width: 0;
  gap: 2px;
}

.mention-member-copy strong,
.mention-member-copy small {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.mention-member-copy strong {
  color: #1f2c25;
  font-size: 13px;
}

.mention-member-copy small {
  color: #718077;
  font-size: 11px;
}

.mention-menu button:hover {
  border-color: #d7e6dc;
  background: #f2f8f5;
}

.mention-avatar {
  display: grid;
  width: 34px;
  height: 34px;
  flex: 0 0 auto;
  overflow: hidden;
  place-items: center;
  border-radius: 50%;
  box-shadow: inset 0 0 0 1px rgba(255, 255, 255, 0.4);
  color: #ffffff;
  font-size: 13px;
  font-weight: 800;
}

.mention-avatar img {
  width: 100%;
  height: 100%;
  object-fit: cover;
}
</style>
