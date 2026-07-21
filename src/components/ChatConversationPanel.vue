<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { open } from "@tauri-apps/plugin-dialog";
import { ElMessage } from "element-plus";
import {
  Check,
  ChevronDown,
  CircleAlert,
  CircleStop as CircleClose,
  ClipboardList,
  Copy as CopyDocument,
  FolderOpen as FolderOpened,
  MessagesSquare,
  RotateCcw as RefreshLeft,
  ScrollText,
  Send as Promotion,
  Wifi,
  Wrench as Tools,
  X,
} from "@lucide/vue";
import { useI18n } from "vue-i18n";
import PatchApprovalPanel from "./PatchApprovalPanel.vue";
import GroupPlanCard from "./GroupPlanCard.vue";
import { getAvatarSrc } from "../utils/avatar";
import { getReadableTextColor } from "../utils/colorContrast";
import { sanitizeAssistantMessageContent } from "../utils/messageTransport";
import type {
  AgentApprovalMode,
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
  | "error"
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
  /** Insertion order of the segment's first item, when known. */
  startedSeq?: number;
}

interface MessageTimelineEntry {
  id: string;
  order: number;
  timestamp?: number;
  /** Monotonic insertion order; preferred over timestamp when present. */
  seq?: number;
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
  plans: ChatGroup["plans"];
  composer: string;
  approvalMode: AgentApprovalMode;
  workspacePath: string;
  sending: boolean;
  canSend: boolean;
  statusText: Record<ChatMessage["status"], string>;
  renderMarkdown: (source: string) => string;
}>();

const emit = defineEmits<{
  "update:composer": [value: string];
  "update:approvalMode": [value: AgentApprovalMode];
  "update:workspacePath": [value: string];
  updatePatchStatus: [proposalId: string, status: PatchApprovalStatus];
  removePatchProposal: [proposalId: string];
  executePlan: [planId: string, memberId: string];
  sendMessage: [];
  stopGeneration: [];
  resetSession: [];
}>();

const messagesPanel = ref<HTMLElement | null>(null);
const messagesStack = ref<HTMLElement | null>(null);
const messagesEnd = ref<HTMLElement | null>(null);
const composerFooter = ref<HTMLElement | null>(null);
const composerInput = ref<{ focus: () => void } | null>(null);
const executionSegmentOpen = ref<Record<string, boolean>>({});
const failedMentionAvatars = ref<Set<string>>(new Set());
const mentionActiveIndex = ref(0);
const mentionDismissed = ref(false);
const stickToBottom = ref(true);
const planPanelOpen = ref(false);
const latestPlan = computed(() => props.plans[props.plans.length - 1]);
const latestPlanStatusTagType = computed(() => {
  switch (latestPlan.value?.status) {
    case "approved":
      return "success";
    case "executing":
      return "primary";
    case "done":
      return "info";
    default:
      return "warning";
  }
});
const { t } = useI18n();
const previousMessageStatuses = new Map<string, ChatMessage["status"]>();
let pendingScrollFrame = 0;
let layoutResizeObserver: ResizeObserver | null = null;
const bottomStickyThreshold = 96;
const followScrollFrames = 3;
const approvalModeOptions = computed<Array<{ label: string; value: AgentApprovalMode }>>(() => [
  { label: t("rightPanel.approvalModeOptions.manual"), value: "manual" },
  { label: t("rightPanel.approvalModeOptions.confirmRisky"), value: "confirm-risky" },
  { label: t("rightPanel.approvalModeOptions.auto"), value: "auto" },
]);

function updateApprovalMode(value: string | number | boolean) {
  emit("update:approvalMode", String(value) as AgentApprovalMode);
}

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

const mentionMenuOpen = computed(
  () => mentionOpen.value && !mentionDismissed.value && mentionCandidates.value.length > 0,
);

watch(
  () => props.composer,
  () => {
    mentionActiveIndex.value = 0;
    mentionDismissed.value = false;
  },
);

watch(mentionCandidates, (candidates) => {
  mentionActiveIndex.value = Math.min(
    mentionActiveIndex.value,
    Math.max(0, candidates.length - 1),
  );
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
  error: "danger",
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
  window.addEventListener("pointerdown", handlePlanDockPointerDown, true);

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
  window.removeEventListener("pointerdown", handlePlanDockPointerDown, true);

  if (pendingScrollFrame) {
    window.cancelAnimationFrame(pendingScrollFrame);
    pendingScrollFrame = 0;
  }

  layoutResizeObserver?.disconnect();
  layoutResizeObserver = null;
});

function handlePlanDockPointerDown(event: PointerEvent) {
  if (!planPanelOpen.value) {
    return;
  }

  const target = event.target;

  if (target instanceof Element && !target.closest(".plan-dock")) {
    planPanelOpen.value = false;
  }
}

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

  mentionDismissed.value = true;
  emit("update:composer", nextComposer);
  void nextTick(() => composerInput.value?.focus());
}

function getMentionOptionId(member: AgentModel) {
  return `mention-option-${member.id}`;
}

function scrollActiveMentionIntoView() {
  void nextTick(() => {
    const member = mentionCandidates.value[mentionActiveIndex.value];
    if (member) {
      document.getElementById(getMentionOptionId(member))?.scrollIntoView({ block: "nearest" });
    }
  });
}

function handleComposerKeydown(event: KeyboardEvent) {
  if (event.isComposing) {
    return;
  }

  if (mentionMenuOpen.value) {
    if (event.key === "ArrowDown") {
      event.preventDefault();
      mentionActiveIndex.value = (mentionActiveIndex.value + 1) % mentionCandidates.value.length;
      scrollActiveMentionIntoView();
      return;
    }

    if (event.key === "ArrowUp") {
      event.preventDefault();
      mentionActiveIndex.value =
        (mentionActiveIndex.value - 1 + mentionCandidates.value.length) %
        mentionCandidates.value.length;
      scrollActiveMentionIntoView();
      return;
    }

    if (event.key === "Escape") {
      event.preventDefault();
      mentionDismissed.value = true;
      return;
    }

    if (event.key === "Enter" && !event.shiftKey) {
      event.preventDefault();
      insertMention(mentionCandidates.value[mentionActiveIndex.value]);
      return;
    }
  }

  if (
    event.key === "Enter" &&
    !event.shiftKey &&
    !event.altKey &&
    !event.ctrlKey &&
    !event.metaKey
  ) {
    event.preventDefault();
    emit("sendMessage");
  }
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

type VoteStatus = "agree" | "supplement" | "disagree";

interface MessageVoter {
  member: AgentModel;
  status: VoteStatus;
}

function getMessageVoters(message: ChatMessage): MessageVoter[] {
  const members = props.activeGroup?.members ?? props.activeMembers;
  const seen = new Set<string>();
  const voters: MessageVoter[] = [];

  const push = (ids: string[] | undefined, status: VoteStatus) => {
    for (const id of ids ?? []) {
      if (seen.has(id)) {
        continue;
      }
      seen.add(id);
      const member = members.find((item) => item.id === id);
      if (member) {
        voters.push({ member, status });
      }
    }
  };

  push(message.agreeMemberIds, "agree");
  push(message.supplementMemberIds, "supplement");
  push(message.disagreeMemberIds, "disagree");
  return voters;
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
        startedSeq: draft.items
          .map((item) => item.seq)
          .find((seq): seq is number => typeof seq === "number"),
      };
    })
    .filter((segment) => segment.blocks.length > 0);
}

function getContentSegments(message: ChatMessage): ChatMessageContentSegment[] {
  const segments = (message.contentSegments ?? []).filter((segment) => segment.text.trim().length > 0);

  if (segments.length > 0) {
    if (message.role !== "assistant") {
      return segments;
    }

    return segments
      .map((segment) => ({
        ...segment,
        text: sanitizeAssistantMessageContent(segment.text),
      }))
      .filter((segment) => segment.text.trim().length > 0);
  }

  const visibleContent =
    message.role === "assistant" ? sanitizeAssistantMessageContent(message.content) : message.content.trim();

  return visibleContent
    ? [
        {
          id: `${message.id}-content`,
          text: visibleContent,
        },
      ]
    : [];
}

function getMessageTimeline(message: ChatMessage): MessageTimelineEntry[] {
  const contentEntries: MessageTimelineEntry[] = getContentSegments(message).map((content, index) => ({
    id: content.id,
    order: index * 2,
    timestamp: content.createdAt,
    seq: content.seq,
    content,
  }));
  const executionEntries: MessageTimelineEntry[] = getExecutionSegments(message).map((execution, index) => ({
    id: execution.id,
    order: index * 2 + 1,
    timestamp: execution.startedAt,
    seq: execution.startedSeq,
    execution,
  }));

  return [...contentEntries, ...executionEntries].sort((left, right) => {
    // Monotonic insertion order is the source of truth: timestamp ties (items
    // flushed within the same millisecond) used to scramble the record back
    // into "content first, reasoning after".
    if (left.seq !== undefined && right.seq !== undefined && left.seq !== right.seq) {
      return left.seq - right.seq;
    }

    if (left.seq !== undefined && right.seq === undefined) {
      return -1;
    }

    if (left.seq === undefined && right.seq !== undefined) {
      return 1;
    }

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

  if (item.kind === "instruction") {
    return ScrollText;
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

  // Emit the record in chronological order: content output and thinking/tool
  // blocks interleaved exactly as they happened, instead of one big
  // generated-text block followed by one big execution dump.
  let executionIndex = 0;

  for (const entry of getMessageTimeline(message)) {
    if (entry.content) {
      const text = entry.content.text.trim();

      if (text) {
        sections.push(`## ${t("chat.copy.generatedText")}\n${text}`);
      }
      continue;
    }

    if (entry.execution) {
      const items = entry.execution.items
        .map((item) => formatExecutionItemForCopy(item, executionIndex++))
        .join("\n\n");

      if (items) {
        sections.push(`## ${t("chat.execution.title")}\n${items}`);
      }
    }
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

// Kun-style cumulative usage at the composer bottom: total tokens, turns and
// cache hit rate across the group's messages, not just per-message tooltips.
const groupUsage = computed(() => {
  let promptTokens = 0;
  let completionTokens = 0;
  let cacheHit = 0;
  let cacheMiss = 0;
  let turns = 0;

  for (const message of props.messages) {
    if (message.role !== "assistant") {
      continue;
    }
    turns += 1;
    promptTokens += message.contextPromptTokens ?? 0;
    completionTokens += message.contextCompletionTokens ?? 0;
    cacheHit += message.contextCacheHitTokens ?? 0;
    cacheMiss += message.contextCacheMissTokens ?? 0;
  }

  const totalTokens = promptTokens + completionTokens;
  const cacheTotal = cacheHit + cacheMiss;

  return {
    turns,
    tokens: formatTokenCount(totalTokens),
    cacheRate: cacheTotal > 0 ? `${Math.round((cacheHit / cacheTotal) * 100)}%` : "-",
    hasUsage: totalTokens > 0 || cacheTotal > 0,
  };
});

defineExpose({
  collapseExecutionPanel,
  scrollToBottom,
});
</script>

<template>
  <main class="chat-workspace">
    <header class="chat-header">
      <div class="chat-header-main">
        <div class="chat-header-info">
          <h1 class="chat-group-name">
            {{ activeGroup?.name || t("chat.empty.noGroupTitle") }}
          </h1>
          <p v-if="activeGroup?.description" class="chat-group-desc">
            {{ activeGroup.description }}
          </p>
        </div>
        <div class="chat-header-meta">
          <el-tag type="info" size="small">
            {{ t("chat.onlineMembers", { count: activeMemberCount }) }}
          </el-tag>
          <el-tag v-if="activeGroup" :type="activeGroup.mode === 'task' ? 'warning' : 'success'" size="small">
            {{ t(`createGroup.modes.${activeGroup.mode}`) }}
          </el-tag>
        </div>
      </div>
      <div class="chat-header-tools">
        <span class="workspace-label">{{ t("chat.workspace.label") }}</span>
        <el-input
          class="workspace-input"
          :model-value="workspacePath"
          :placeholder="t('chat.workspace.placeholder')"
          :aria-label="t('chat.workspace.label')"
          size="small"
          clearable
          @update:model-value="emit('update:workspacePath', String($event))"
        >
          <template #append>
            <el-button
              :icon="FolderOpened"
              size="small"
              :title="t('chat.chooseWorkspaceTitle')"
              :aria-label="t('chat.chooseWorkspaceTitle')"
              @click="chooseWorkspacePath"
            />
          </template>
        </el-input>
        <el-button
          class="reset-session-button"
          :icon="RefreshLeft"
          size="small"
          plain
          :title="t('chat.resetSession.label')"
          :disabled="messages.length === 0 && !sending"
          @click="emit('resetSession')"
        >
          {{ t("chat.resetSession.label") }}
        </el-button>
      </div>
    </header>

    <section
      v-if="speakerQueue.length > 0"
      class="speaker-queue"
      :aria-label="t('chat.queue.title')"
      aria-live="polite"
    >
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

    <div v-if="plans.length > 0" class="plan-dock">
      <button
        type="button"
        class="plan-dock-bar"
        :aria-expanded="planPanelOpen"
        :aria-label="t('plan.dockToggle')"
        @click="planPanelOpen = !planPanelOpen"
      >
        <ClipboardList class="plan-dock-icon" aria-hidden="true" />
        <span class="plan-dock-title">{{ latestPlan?.title }}</span>
        <el-tag size="small" :type="latestPlanStatusTagType">
          {{ t(`plan.status.${latestPlan?.status}`) }}
        </el-tag>
        <span class="plan-dock-count">{{ t("plan.dockCount", { count: plans.length }) }}</span>
        <ChevronDown class="plan-dock-chevron" :class="{ open: planPanelOpen }" aria-hidden="true" />
      </button>
      <div v-show="planPanelOpen" class="plan-dock-body">
        <GroupPlanCard
          v-for="plan in plans"
          :key="plan.id"
          :plan="plan"
          :members="activeMembers"
          :render-markdown="renderMarkdown"
          @execute="(planId, memberId) => emit('executePlan', planId, memberId)"
        />
      </div>
    </div>

    <section
      ref="messagesPanel"
      class="messages-panel"
      role="log"
      aria-live="polite"
      aria-relevant="additions"
      :aria-busy="sending"
      :aria-label="t('chat.messageLog')"
      @scroll="updateStickToBottom"
    >
      <div ref="messagesStack" class="messages-stack">
        <div v-if="messages.length === 0" class="chat-empty-state">
          <MessagesSquare aria-hidden="true" />
          <strong>
            {{ activeGroup ? t("chat.empty.title") : t("chat.empty.noGroupTitle") }}
          </strong>
          <span>
            {{ activeGroup ? t("chat.empty.description") : t("chat.empty.noGroupDescription") }}
          </span>
        </div>
        <article
          v-for="message in messages"
          :key="message.id"
          class="message-row"
          :class="[message.role, message.status]"
          :style="{ '--accent': message.color }"
          :aria-label="`${message.modelName}: ${statusText[message.status]}`"
        >
          <div class="message-meta">
            <span
              class="message-avatar"
              :style="{
                '--avatar-accent': message.color,
                color: getReadableTextColor(message.color),
              }"
            >
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
                <span
                  class="context-meter"
                  :style="getContextRingStyle(message)"
                  role="progressbar"
                  tabindex="0"
                  aria-valuemin="0"
                  aria-valuemax="100"
                  :aria-valuenow="Number(getContextPercent(message))"
                  :aria-label="t('chat.context.percent', { percent: getContextPercent(message) })"
                >
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
            <span v-if="formatDuration(message.durationMs)" class="message-duration-badge">
              {{ t("chat.messageMeta.duration", { duration: formatDuration(message.durationMs) }) }}
            </span>
          </div>

          <div
            v-if="getMessageVoters(message).length > 0"
            class="reaction-voters"
            :class="{ complete: allPeersAgreed(message) }"
          >
            <span
              v-for="entry in getMessageVoters(message)"
              :key="entry.member.id"
              class="reaction-voter"
              :class="entry.status"
              :title="t(`chat.voterTitle.${entry.status}`, { name: entry.member.name })"
            >
              <span
                class="reaction-voter-avatar"
                :style="{
                  background: entry.member.color,
                  color: getReadableTextColor(entry.member.color),
                }"
              >
                <img v-if="entry.member.avatar" :src="getAvatarSrc(entry.member.avatar)" alt="" />
                <span v-else>{{ getInitial(entry.member.name) }}</span>
              </span>
              <span class="reaction-voter-name">{{ entry.member.name }}</span>
              <el-icon class="reaction-voter-status">
                <Check v-if="entry.status === 'agree'" />
                <CircleAlert v-else-if="entry.status === 'supplement'" />
                <X v-else />
              </el-icon>
            </span>
          </div>
        </article>
        <div ref="messagesEnd" class="messages-end" aria-hidden="true"></div>
      </div>
    </section>

    <footer ref="composerFooter" class="composer">
      <div class="composer-input-wrap">
        <div
          v-if="mentionMenuOpen"
          id="mention-options"
          class="mention-menu"
          role="listbox"
          :aria-label="t('chat.mentions.label')"
        >
          <button
            v-for="(member, index) in mentionCandidates"
            :id="getMentionOptionId(member)"
            :key="member.id"
            :class="{ active: index === mentionActiveIndex }"
            type="button"
            role="option"
            tabindex="-1"
            :aria-selected="index === mentionActiveIndex"
            @mouseenter="mentionActiveIndex = index"
            @click="insertMention(member)"
          >
            <span
              class="mention-avatar"
              :style="{
                background: member.color,
                color: getReadableTextColor(member.color),
              }"
            >
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
          ref="composerInput"
          :model-value="composer"
          type="textarea"
          :rows="4"
          resize="vertical"
          :placeholder="t('chat.composerPlaceholder')"
          :aria-label="t('chat.composerLabel')"
          role="combobox"
          aria-autocomplete="list"
          aria-haspopup="listbox"
          aria-controls="mention-options"
          :aria-expanded="mentionMenuOpen"
          :aria-activedescendant="
            mentionMenuOpen && mentionCandidates[mentionActiveIndex]
              ? getMentionOptionId(mentionCandidates[mentionActiveIndex])
              : undefined
          "
          @update:model-value="emit('update:composer', String($event))"
          @keydown="handleComposerKeydown"
        />
      </div>

      <div class="composer-actions">
        <div class="composer-approval">
          <span class="composer-approval-label">{{ t("rightPanel.approvalMode") }}</span>
          <el-select
            :model-value="approvalMode"
            size="small"
            :aria-label="t('rightPanel.approvalMode')"
            @update:model-value="updateApprovalMode"
          >
            <el-option
              v-for="option in approvalModeOptions"
              :key="option.value"
              :label="option.label"
              :value="option.value"
            />
          </el-select>
        </div>

        <span v-if="groupUsage.hasUsage" class="composer-usage">
          {{
            t("chat.usageSummary", {
              tokens: groupUsage.tokens,
              turns: groupUsage.turns,
              rate: groupUsage.cacheRate,
            })
          }}
        </span>

        <div class="composer-button-group">
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

.activity-chip.network {
  border-color: #b7dce0;
  color: #176873;
  background: #eefafb;
}

.activity-chip.network.running .el-icon {
  position: relative;
  isolation: isolate;
  overflow: visible;
  color: #08747e;
  filter: drop-shadow(0 0 3px rgb(30 189 195 / 55%));
  animation: wifi-icon-breathe 1.5s ease-in-out infinite;
}

.activity-chip.network.running .el-icon::before {
  position: absolute;
  z-index: -1;
  inset: -5px;
  border-radius: 50%;
  background: conic-gradient(
    from 0deg,
    rgb(19 162 174 / 0%) 0deg,
    rgb(38 196 190 / 70%) 92deg,
    rgb(88 139 229 / 75%) 178deg,
    rgb(19 162 174 / 0%) 280deg
  );
  content: "";
  filter: blur(4px);
  opacity: 0.72;
  animation: wifi-gradient-orbit 3s linear infinite;
}

.activity-chip.status.running {
  border-color: color-mix(in srgb, var(--warning) 35%, transparent);
  color: var(--warning);
  background: var(--warning-soft);
}

.activity-chip.done {
  border-color: color-mix(in srgb, var(--success) 30%, transparent);
  color: var(--success);
  background: var(--success-soft);
}

.activity-chip.error {
  border-color: color-mix(in srgb, var(--danger) 30%, transparent);
  color: var(--danger);
  background: var(--danger-soft);
}

.activity-chip.interrupted {
  border-color: color-mix(in srgb, var(--warning) 35%, transparent);
  color: var(--warning);
  background: var(--warning-soft);
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

.execution-line.network .execution-icon,
.execution-segment.network .execution-icon {
  border-color: #b7dce0;
  color: #176873;
  background: #eefafb;
}

.execution-line.network.running .execution-icon,
.execution-segment.network.running .execution-icon {
  position: relative;
  isolation: isolate;
  overflow: visible;
  border-color: rgb(61 194 191 / 75%);
  color: #08747e;
  background: linear-gradient(145deg, #f4fffd 10%, #e7f7ff 55%, #f4f0ff 100%);
  box-shadow:
    0 0 0 1px rgb(44 185 185 / 10%),
    0 0 10px rgb(38 196 190 / 38%),
    0 0 18px rgb(88 139 229 / 18%);
  animation: wifi-icon-breathe 1.5s ease-in-out infinite;
}

.execution-line.network.running .execution-icon::before,
.execution-segment.network.running .execution-icon::before {
  position: absolute;
  z-index: -1;
  inset: -5px;
  border-radius: 50%;
  background: conic-gradient(
    from 0deg,
    rgb(19 162 174 / 0%) 0deg,
    rgb(38 196 190 / 72%) 92deg,
    rgb(88 139 229 / 78%) 178deg,
    rgb(19 162 174 / 0%) 280deg
  );
  content: "";
  filter: blur(5px);
  opacity: 0.78;
  animation: wifi-gradient-orbit 3s linear infinite;
}

@keyframes wifi-gradient-orbit {
  from {
    transform: rotate(0deg) scale(0.92);
  }

  to {
    transform: rotate(360deg) scale(0.92);
  }
}

@keyframes wifi-icon-breathe {
  0%,
  100% {
    filter: drop-shadow(0 0 2px rgb(30 189 195 / 35%));
  }

  50% {
    filter: drop-shadow(0 0 7px rgb(88 139 229 / 72%));
  }
}

@media (prefers-reduced-motion: reduce) {
  .activity-chip.network.running .el-icon,
  .activity-chip.network.running .el-icon::before,
  .execution-line.network.running .execution-icon,
  .execution-line.network.running .execution-icon::before,
  .execution-segment.network.running .execution-icon,
  .execution-segment.network.running .execution-icon::before {
    animation: none;
  }
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
  border-color: color-mix(in srgb, var(--warning) 35%, transparent);
  color: var(--warning);
  background: var(--warning-soft);
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
  color: var(--success);
  background: var(--success-soft);
}

.identity-badge.owner {
  color: var(--warning);
  background: var(--warning-soft);
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
  align-items: center;
  justify-content: space-between;
  gap: 10px;
  min-width: 0;
}

.composer-approval {
  display: inline-flex;
  flex: 1 1 240px;
  align-items: center;
  gap: 10px;
  min-width: 0;
}

.composer-usage {
  flex: 0 1 auto;
  min-width: 0;
  overflow: hidden;
  color: var(--text-secondary);
  font-size: 12px;
  font-variant-numeric: tabular-nums;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.composer-approval-label {
  flex: 0 0 auto;
  color: #5a6760;
  font-size: 12px;
  font-weight: 600;
}

.composer-approval :deep(.el-select) {
  width: min(100%, 220px);
}

.composer-button-group {
  display: inline-flex;
  flex: 0 0 auto;
  flex-wrap: wrap;
  justify-content: flex-end;
  gap: 10px;
}

.composer-button-group :deep(.el-button) {
  margin-left: 0;
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

<style scoped>
.chat-empty-state {
  display: grid;
  min-height: 100%;
  flex: 1;
  align-content: center;
  justify-items: center;
  gap: 8px;
  padding: 40px 24px;
  color: var(--text-secondary);
  text-align: center;
}

.chat-empty-state svg {
  width: 34px;
  height: 34px;
  margin-bottom: 4px;
  color: var(--text-tertiary);
  stroke-width: 1.6;
}

.chat-empty-state strong {
  color: var(--text-primary);
  font-size: 14px;
}

.chat-empty-state span {
  max-width: 360px;
  font-size: 13px;
  line-height: 1.5;
}

.message-row.thinking {
  border-color: color-mix(in srgb, var(--accent) 48%, var(--separator));
}

.message-row.thinking::before {
  border-radius: 8px;
  background: var(--accent);
  animation: none;
}

.message-row.thinking::after {
  display: none;
}

.message-status-bar {
  border-color: var(--separator);
}

.message-copy-button,
.message-stop-button,
.execution-copy-button {
  width: var(--hit-target-min);
  height: var(--hit-target-min);
  border-color: var(--separator-strong);
  border-radius: 6px;
  color: var(--text-secondary);
  background: var(--surface);
  box-shadow: none;
}

.message-copy-button:hover,
.execution-copy-button:hover {
  border-color: var(--accent);
  color: var(--accent-text);
  background: var(--accent-soft);
  box-shadow: none;
}

.message-stop-button {
  border-color: color-mix(in srgb, var(--danger) 45%, var(--separator));
  color: var(--danger);
}

.message-stop-button:hover {
  border-color: var(--danger);
  color: var(--danger);
  background: var(--danger-soft);
  box-shadow: none;
}

.activity-empty {
  color: var(--text-tertiary);
}

.activity-chip {
  border-color: var(--separator);
  color: var(--text-secondary);
  background: var(--surface-secondary);
}

.activity-chip.tool,
.activity-chip.done {
  border-color: color-mix(in srgb, var(--success) 30%, var(--separator));
  color: var(--success);
  background: var(--success-soft);
}

.activity-chip.reasoning {
  border-color: color-mix(in srgb, var(--accent) 28%, var(--separator));
  color: var(--accent-text);
  background: var(--accent-soft);
}

.activity-chip.network {
  border-color: color-mix(in srgb, var(--info) 32%, var(--separator));
  color: var(--info);
  background: var(--info-soft);
}

.activity-chip.status.running,
.activity-chip.interrupted {
  border-color: color-mix(in srgb, var(--warning) 34%, var(--separator));
  color: var(--warning);
  background: var(--warning-soft);
}

.activity-chip.error {
  border-color: color-mix(in srgb, var(--danger) 34%, var(--separator));
  color: var(--danger);
  background: var(--danger-soft);
}

.activity-chip.network.running .el-icon,
.activity-chip.network.running .el-icon::before {
  animation: none;
  filter: none;
}

.activity-chip.network.running .el-icon::before {
  display: none;
}

.activity-detail {
  border-color: var(--separator);
  color: var(--text-primary);
  background: var(--surface);
  font-family: "Cascadia Code", "SFMono-Regular", Consolas, monospace;
}

.context-meter {
  min-height: var(--hit-target-min);
  border-color: var(--separator);
  color: var(--text-secondary);
  background: var(--surface);
}

.context-ring {
  background: conic-gradient(var(--accent) var(--context-deg), var(--surface-tertiary) 0deg);
}

.context-ring::after {
  background: var(--surface);
}

.context-cache-label {
  border-color: var(--separator);
  color: var(--accent-text);
}

.context-tooltip,
.context-tooltip strong {
  color: var(--text-primary);
}

.execution-segment,
.execution-segment.collapsed {
  border-color: var(--separator);
  background: var(--surface);
}

.execution-segment-summary {
  min-height: 36px;
  color: var(--text-secondary);
}

.execution-segment-summary:hover {
  background: var(--control-hover);
}

.execution-segment-title {
  color: var(--text-primary);
}

.execution-segment-count {
  border-color: var(--separator);
  color: var(--text-secondary);
  background: var(--surface-secondary);
}

.execution-segment-time,
.execution-kind {
  color: var(--text-tertiary);
}

.execution-segment-body {
  border-color: var(--separator);
}

.execution-line,
.execution-markdown {
  color: var(--text-primary);
}

.execution-icon {
  border-color: var(--separator);
  color: var(--text-secondary);
  background: var(--surface);
}

.execution-line.tool .execution-icon,
.execution-segment.tool .execution-icon {
  border-color: color-mix(in srgb, var(--success) 34%, var(--separator));
  color: var(--success);
  background: var(--success-soft);
}

.execution-line.reasoning .execution-icon,
.execution-segment.reasoning .execution-icon {
  border-color: color-mix(in srgb, var(--accent) 30%, var(--separator));
  color: var(--accent-text);
  background: var(--accent-soft);
}

.execution-line.network .execution-icon,
.execution-segment.network .execution-icon,
.execution-line.network.running .execution-icon,
.execution-segment.network.running .execution-icon,
.execution-segment.mixed .execution-icon {
  border-color: color-mix(in srgb, var(--info) 32%, var(--separator));
  color: var(--info);
  background: var(--info-soft);
  box-shadow: none;
  animation: none;
}

.execution-line.network.running .execution-icon::before,
.execution-segment.network.running .execution-icon::before {
  display: none;
}

.execution-line.error .execution-icon,
.execution-segment.error .execution-icon {
  border-color: color-mix(in srgb, var(--danger) 34%, var(--separator));
  color: var(--danger);
  background: var(--danger-soft);
}

.execution-line.interrupted .execution-icon,
.execution-segment.interrupted .execution-icon {
  border-color: color-mix(in srgb, var(--warning) 34%, var(--separator));
  color: var(--warning);
  background: var(--warning-soft);
}

.execution-markdown :deep(code) {
  color: var(--accent-text);
  background: var(--accent-soft);
}

.execution-markdown :deep(pre) {
  border-color: var(--separator);
  background: var(--code-bg);
}

.execution-markdown :deep(pre code) {
  color: var(--code-text);
}

.execution-code-block {
  border-color: var(--separator);
  background: var(--code-bg);
  box-shadow: none;
}

.execution-code-header {
  border-color: var(--separator);
  color: var(--text-secondary);
  background: var(--surface-tertiary);
  font-family: "Cascadia Code", "SFMono-Regular", Consolas, monospace;
  text-transform: none;
}

.execution-code-pre {
  color: var(--code-text);
  background: var(--code-bg);
  font-family: "Cascadia Code", "SFMono-Regular", Consolas, monospace;
}

.execution-code-pre :deep(.syntax-keyword) {
  color: var(--syntax-keyword);
}

.execution-code-pre :deep(.syntax-string) {
  color: var(--syntax-string);
}

.execution-code-pre :deep(.syntax-comment) {
  color: var(--syntax-comment);
}

.execution-code-pre :deep(.syntax-number) {
  color: var(--syntax-number);
}

.execution-code-pre :deep(.syntax-literal) {
  color: var(--syntax-literal);
}

.execution-code-pre :deep(.syntax-function) {
  color: var(--syntax-function);
}

.execution-code-pre :deep(.syntax-operator) {
  color: var(--syntax-operator);
}

.execution-detail {
  border-color: var(--separator);
  background: var(--surface);
}

.execution-detail summary {
  min-height: 32px;
  color: var(--accent-text);
}

.execution-detail pre {
  border-color: var(--separator);
  color: var(--code-text);
  background: var(--code-bg);
}

.speaker-queue {
  border-color: var(--separator);
  background: var(--surface-secondary);
}

.speaker-queue-title {
  color: var(--text-secondary);
}

.speaker-queue-pill {
  border-color: var(--separator);
  background: var(--surface);
}

.speaker-queue-pill strong {
  color: var(--text-primary);
}

.speaker-queue-pill .queue-dot {
  animation: none;
}

.identity-badge.admin,
.message-title .identity-badge {
  color: var(--success);
  background: var(--success-soft);
}

.identity-badge.owner,
.message-title .identity-badge.owner {
  color: var(--warning);
  background: var(--warning-soft);
}

.composer {
  border-color: var(--separator);
  background: var(--surface);
}

.composer-approval-label {
  color: var(--text-secondary);
}

.mention-menu {
  border-color: var(--separator);
  background: var(--surface-elevated);
  box-shadow: var(--shadow-popover);
}

.mention-menu button {
  min-height: 48px;
  border-radius: 6px;
  color: var(--text-primary);
}

.mention-menu button:hover,
.mention-menu button.active {
  border-color: color-mix(in srgb, var(--accent) 28%, var(--separator));
  background: var(--accent-soft);
}

.mention-member-copy strong {
  color: var(--text-primary);
}

.mention-member-copy small {
  color: var(--text-secondary);
}

@media (max-width: 700px) {
  .composer-actions,
  .composer-approval {
    align-items: stretch;
    flex-direction: column;
  }

  .composer-approval {
    width: 100%;
    flex: 0 1 auto;
  }

  .composer-approval :deep(.el-select) {
    width: 100%;
  }

  .composer-button-group {
    justify-content: flex-end;
  }
}
</style>
