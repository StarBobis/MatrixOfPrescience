<script setup lang="ts">
import { nextTick, ref } from "vue";
import { open } from "@tauri-apps/plugin-dialog";
import { FolderOpened, Promotion } from "@element-plus/icons-vue";
import PatchApprovalPanel from "./PatchApprovalPanel.vue";
import type { ChatGroup, ChatMessage, PatchApprovalStatus } from "../stores/settings";

defineProps<{
  activeGroup?: ChatGroup;
  activeMemberCount: number;
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
}>();

const messagesPanel = ref<HTMLElement | null>(null);

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
