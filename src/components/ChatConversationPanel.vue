<script setup lang="ts">
import { computed, nextTick, ref } from "vue";
import { open } from "@tauri-apps/plugin-dialog";
import { FolderOpened, Promotion } from "@element-plus/icons-vue";
import PatchApprovalPanel from "./PatchApprovalPanel.vue";
import type { AgentModel, ChatGroup, ChatMessage, PatchApprovalStatus } from "../stores/settings";

const props = defineProps<{
  activeGroup?: ChatGroup;
  activeMemberCount: number;
  activeMembers: AgentModel[];
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
}>();

const messagesPanel = ref<HTMLElement | null>(null);

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
    title: "选择当前群工作文件夹",
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
          <el-tag type="info" size="small">{{ activeMemberCount }} 群友在线</el-tag>
        </div>
      </div>
      <div class="chat-header-tools">
        <span class="workspace-label">工作文件夹</span>
        <el-input
          class="workspace-input"
          :model-value="workspacePath"
          placeholder="选择或输入路径…"
          size="small"
          clearable
          @update:model-value="emit('update:workspacePath', String($event))"
        >
          <template #append>
            <el-button :icon="FolderOpened" size="small" title="选择工作文件夹" @click="chooseWorkspacePath" />
          </template>
        </el-input>
      </div>
    </header>

    <section ref="messagesPanel" class="messages-panel">
      <PatchApprovalPanel
        :patch-proposals="patchProposals"
        @update-patch-status="(proposalId, status) => emit('updatePatchStatus', proposalId, status)"
        @remove-patch-proposal="(proposalId) => emit('removePatchProposal', proposalId)"
      />

      <article
        v-for="message in messages"
        :key="message.id"
        class="message-row"
        :class="message.role"
        :style="{ '--accent': message.color }"
      >
        <div class="message-meta">
          <span class="accent-line"></span>
          <div class="message-title">
            <strong>{{ message.modelName }}</strong>
            <span v-if="message.providerName">{{ message.providerName }}</span>
          </div>
          <span class="status-pill" :class="message.status">
            {{ statusText[message.status] }}
          </span>
          <time>{{ message.time }}</time>
        </div>

        <div class="message-body" v-html="renderMarkdown(message.content)"></div>

        <details
          v-if="(message.thoughtSteps ?? []).length > 0"
          class="thought-steps"
          :open="message.status === 'thinking'"
        >
          <summary>思考过程</summary>
          <ol>
            <li v-for="step in message.thoughtSteps" :key="step">{{ step }}</li>
          </ol>
        </details>

        <div class="message-reactions">
          <span class="reaction-pill agree">
            同意 {{ (message.agreeMemberIds ?? []).length }}
          </span>
          <span class="reaction-pill disagree">
            不同意 {{ (message.disagreeMemberIds ?? []).length }}
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
        </button>
      </div>

      <el-input
        :model-value="composer"
        type="textarea"
        :autosize="{ minRows: 3, maxRows: 7 }"
        resize="none"
        placeholder="输入消息，Enter 发送，Shift + Enter 换行"
        @update:model-value="emit('update:composer', String($event))"
        @keydown.enter.exact.prevent="emit('sendMessage')"
      />

      <el-button
        v-if="sending"
        type="danger"
        plain
        @click="emit('stopGeneration')"
      >
        中断
      </el-button>

      <el-button
        type="primary"
        :loading="sending"
        :disabled="!canSend"
        :icon="Promotion"
        @click="emit('sendMessage')"
      >
        发给 {{ activeMemberCount }} 个群友
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
