<script setup lang="ts">
import { computed, nextTick, ref } from "vue";
import { open } from "@tauri-apps/plugin-dialog";
import { CircleClose, FolderOpened, Promotion, RefreshLeft, Tools } from "@element-plus/icons-vue";
import { useI18n } from "vue-i18n";
import PatchApprovalPanel from "./PatchApprovalPanel.vue";
import { getAvatarSrc } from "../utils/avatar";
import type {
  AgentModel,
  ChatGroup,
  ChatMessage,
  ChatMessageActivityItem,
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
const approvalPanelOpen = ref(false);
const { t } = useI18n();

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

async function scrollToBottom() {
  await nextTick();

  if (messagesPanel.value) {
    messagesPanel.value.scrollTop = messagesPanel.value.scrollHeight;
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

function getActivityIcon(item: ChatMessageActivityItem) {
  return item.kind === "tool" ? Tools : RefreshLeft;
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

    <section ref="messagesPanel" class="messages-panel">
      <article
        v-for="message in messages"
        :key="message.id"
        class="message-row"
        :class="message.role"
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
          <span class="status-pill" :class="message.status">
            {{ statusText[message.status] }}
          </span>
          <time>{{ message.time }}</time>
        </div>

        <div class="message-body" v-html="renderMarkdown(message.content)"></div>

        <div
          v-if="(message.activityItems ?? []).length > 0"
          class="message-activity-bar"
          :class="message.status"
        >
          <span
            v-for="item in message.activityItems"
            :key="item.id"
            class="activity-chip"
            :class="[item.kind, item.status]"
            :title="item.text"
          >
            <el-icon>
              <component :is="getActivityIcon(item)" />
            </el-icon>
            <span>{{ item.text }}</span>
          </span>
        </div>

        <details
          v-if="(message.thoughtSteps ?? []).length > 0"
          class="thought-steps"
          :open="message.status === 'thinking'"
        >
          <summary>{{ t("chat.thoughtSteps") }}</summary>
          <ol>
            <li v-for="step in message.thoughtSteps" :key="step">{{ step }}</li>
          </ol>
        </details>

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
    </section>

    <footer class="composer">
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
.thought-steps {
  margin-top: 10px;
  padding: 8px 10px;
  border-radius: 8px;
  color: #53615a;
  background: #f5f8f6;
  font-size: 12px;
}

.message-activity-bar {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
  min-height: 30px;
  margin-top: 10px;
  padding: 6px 0 0;
  border-top: 1px solid #edf1ee;
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

.activity-chip .el-icon {
  flex: 0 0 auto;
  font-size: 13px;
}

.activity-chip span:last-child {
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

.thought-steps summary {
  cursor: pointer;
  font-weight: 700;
}

.thought-steps ol {
  display: grid;
  gap: 4px;
  margin: 8px 0 0;
  padding-left: 18px;
}

.thought-steps li {
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
