<script setup lang="ts">
import { computed, nextTick, onMounted, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import MarkdownIt from "markdown-it";
import { ElMessage } from "element-plus";
import { storeToRefs } from "pinia";
import {
  ChatDotRound,
  CirclePlus,
  Delete,
  DocumentCopy,
  MagicStick,
  Promotion,
  Setting,
} from "@element-plus/icons-vue";
import {
  type AgentModel,
  type ChatMessage,
  type ProviderId,
  useSettingsStore,
} from "./stores/settings";

type ChatRole = "user" | "assistant";
type MessageStatus = "done" | "thinking" | "error";

interface ApiChatMessage {
  role: ChatRole;
  content: string;
}

interface ChatCompletionResponse {
  content: string;
}

const markdown = new MarkdownIt({
  breaks: true,
  html: false,
  linkify: true,
  typographer: true,
});

const providerOptions = [
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

const statusText: Record<MessageStatus, string> = {
  done: "完成",
  thinking: "生成中",
  error: "错误",
};

const settingsStore = useSettingsStore();
const { providers, groups, activeGroup, activeMembers } = storeToRefs(settingsStore);

const composer = ref("");
const settingsOpen = ref(false);
const groupDialogOpen = ref(false);
const sending = ref(false);
const messagesPanel = ref<HTMLElement | null>(null);

const newGroupName = ref("新 Agent 群");
const newGroupDescription = ref("一个新的多模型讨论群");
const newGroupMembers = ref<AgentModel[]>([]);

const activeMessages = computed<ChatMessage[]>(() => activeGroup.value?.messages ?? []);
const activeGroupMembers = computed(() => activeGroup.value?.members ?? []);
const canSend = computed(
  () => composer.value.trim().length > 0 && activeMembers.value.length > 0 && !sending.value,
);

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
  newGroupMembers.value = [
    settingsStore.createMemberDraft("openai"),
    settingsStore.createMemberDraft("deepseek"),
  ];
  groupDialogOpen.value = true;
}

function addDraftMember(provider: ProviderId = "openai") {
  newGroupMembers.value.push(settingsStore.createMemberDraft(provider));
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

  if (!groupName) {
    ElMessage.warning("请输入群名称");
    return;
  }

  settingsStore.createGroup(
    groupName,
    newGroupDescription.value.trim(),
    newGroupMembers.value.map((member) => ({ ...member })),
  );
  groupDialogOpen.value = false;
  scrollToBottom();
}

function selectGroup(groupId: string) {
  settingsStore.selectGroup(groupId);
  scrollToBottom();
}

function removeGroup(groupId: string) {
  if (!settingsStore.removeGroup(groupId)) {
    ElMessage.warning("至少保留一个聊天群");
  }
}

function addMember(provider: ProviderId = "openai") {
  settingsStore.addMember(provider);
}

function duplicateMember(member: AgentModel) {
  settingsStore.duplicateMember(member);
}

function removeMember(memberId: string) {
  if (activeGroupMembers.value.length <= 1) {
    ElMessage.warning("至少保留一个虚拟群友");
    return;
  }

  settingsStore.removeMember(memberId);
}

function updateMemberProvider(member: AgentModel) {
  settingsStore.updateMemberProvider(member);
}

function buildConversation(userText: string): ApiChatMessage[] {
  const history = activeMessages.value
    .filter((message) => message.modelName !== "系统")
    .slice(-12)
    .map<ApiChatMessage>((message) => ({
      role: message.role,
      content:
        message.role === "assistant"
          ? `${message.modelName}：${message.content}`
          : message.content,
    }));

  history.push({
    role: "user",
    content: userText,
  });

  return history;
}

function appendMessage(message: Omit<ChatMessage, "id" | "time">) {
  settingsStore.appendMessage(message);
  scrollToBottom();
}

async function scrollToBottom() {
  await nextTick();

  if (messagesPanel.value) {
    messagesPanel.value.scrollTop = messagesPanel.value.scrollHeight;
  }
}

async function askMember(member: AgentModel, conversation: ApiChatMessage[]) {
  const provider = getProvider(member);
  const pendingId = crypto.randomUUID();

  settingsStore.appendPendingMessage({
    id: pendingId,
    role: "assistant",
    modelName: member.name,
    providerName: getProviderLabel(member.provider),
    status: "thinking",
    content: "正在生成回复...",
    color: member.color,
  });
  await scrollToBottom();

  try {
    const response = await invoke<ChatCompletionResponse>("chat_completion", {
      request: {
        providerName: provider.name,
        baseUrl: provider.baseUrl,
        apiKey: provider.apiKey,
        model: member.model,
        temperature: member.temperature,
        systemPrompt: member.systemPrompt,
        messages: conversation,
      },
    });

    settingsStore.updateMessage(pendingId, {
      status: "done",
      content: response.content,
    });
  } catch (error) {
    settingsStore.updateMessage(pendingId, {
      status: "error",
      content: `调用失败：${String(error)}`,
    });
  } finally {
    await scrollToBottom();
  }
}

async function sendMessage() {
  const userText = composer.value.trim();

  if (!userText || sending.value) {
    return;
  }

  if (activeMembers.value.length === 0) {
    ElMessage.warning("请至少启用一个虚拟群友");
    return;
  }

  const missingKey = activeMembers.value.find((member) => !getProvider(member).apiKey.trim());
  if (missingKey) {
    ElMessage.warning(`请先配置 ${getProviderLabel(missingKey.provider)} API Key`);
    settingsOpen.value = true;
    return;
  }

  composer.value = "";
  sending.value = true;

  appendMessage({
    role: "user",
    modelName: "我",
    status: "done",
    content: userText,
    color: "#4d5a61",
  });

  const conversation = buildConversation(userText);

  await Promise.all(activeMembers.value.map((member) => askMember(member, conversation)));

  sending.value = false;
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
  <div class="app-shell">
    <aside class="side-panel">
      <div class="brand-block">
        <div class="brand-mark">
          <el-icon>
            <MagicStick />
          </el-icon>
        </div>
        <div>
          <p class="eyebrow">Matrix Of Prescience</p>
          <h1>Agent 群聊</h1>
        </div>
      </div>

      <section class="section-block">
        <div class="section-heading">
          <span>聊天群</span>
          <el-tag size="small" type="success">{{ groups.length }}</el-tag>
        </div>

        <div class="group-list">
          <button
            v-for="group in groups"
            :key="group.id"
            class="group-card"
            :class="{ active: group.id === activeGroup?.id }"
            @click="selectGroup(group.id)"
          >
            <div class="group-avatar">
              <el-icon>
                <ChatDotRound />
              </el-icon>
            </div>
            <div class="group-main">
              <strong>{{ group.name }}</strong>
              <span>{{ group.members.length }} 个群友 · {{ group.messages.length }} 条消息</span>
            </div>
          </button>
        </div>
      </section>

      <div class="side-actions">
        <el-button type="primary" :icon="CirclePlus" @click="openCreateGroupDialog">
          新建聊天群
        </el-button>
        <el-button :icon="Setting" @click="settingsOpen = true">
          设置当前群
        </el-button>
      </div>
    </aside>

    <main class="chat-workspace">
      <header class="chat-header">
        <div>
          <p class="eyebrow">Group Conversation</p>
          <h2>{{ activeGroup?.name }}</h2>
          <p class="group-description">{{ activeGroup?.description }}</p>
        </div>
        <div class="header-actions">
          <el-tag type="info">{{ activeMembers.length }} 个启用群友</el-tag>
          <el-button :icon="Setting" @click="settingsOpen = true">设置</el-button>
        </div>
      </header>

      <section ref="messagesPanel" class="messages-panel">
        <article
          v-for="message in activeMessages"
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
        </article>
      </section>

      <footer class="composer">
        <el-input
          v-model="composer"
          type="textarea"
          :autosize="{ minRows: 3, maxRows: 7 }"
          resize="none"
          placeholder="输入消息，Enter 发送，Shift + Enter 换行"
          @keydown.enter.exact.prevent="sendMessage"
        />

        <el-button
          type="primary"
          :loading="sending"
          :disabled="!canSend"
          :icon="Promotion"
          @click="sendMessage"
        >
          发给 {{ activeMembers.length }} 个群友
        </el-button>
      </footer>
    </main>

    <el-dialog v-model="groupDialogOpen" title="新建聊天群" width="720px">
      <el-form label-position="top">
        <el-form-item label="群名称">
          <el-input v-model="newGroupName" />
        </el-form-item>
        <el-form-item label="群简介">
          <el-input v-model="newGroupDescription" />
        </el-form-item>
      </el-form>

      <div class="settings-toolbar">
        <el-button type="primary" :icon="CirclePlus" @click="addDraftMember('openai')">
          添加 ChatGPT 群友
        </el-button>
        <el-button :icon="CirclePlus" @click="addDraftMember('deepseek')">
          添加 DeepSeek 群友
        </el-button>
      </div>

      <div class="settings-stack compact">
        <section v-for="member in newGroupMembers" :key="member.id" class="settings-card">
          <div class="settings-card-head">
            <el-input v-model="member.name" class="model-name-input" />
            <el-switch v-model="member.enabled" />
          </div>

          <div class="member-grid">
            <el-form-item label="API">
              <el-select v-model="member.provider" @change="updateDraftMemberProvider(member)">
                <el-option
                  v-for="option in providerOptions"
                  :key="option.value"
                  :label="option.label"
                  :value="option.value"
                />
              </el-select>
            </el-form-item>
            <el-form-item label="模型">
              <el-select v-model="member.model" filterable allow-create default-first-option>
                <el-option
                  v-for="preset in modelPresets[member.provider]"
                  :key="preset"
                  :label="preset"
                  :value="preset"
                />
              </el-select>
            </el-form-item>
          </div>

          <el-form-item label="核心角色">
            <el-input
              v-model="member.systemPrompt"
              type="textarea"
              :autosize="{ minRows: 2, maxRows: 5 }"
              resize="none"
            />
          </el-form-item>

          <div class="card-actions">
            <el-button type="danger" plain :icon="Delete" @click="removeDraftMember(member.id)">
              删除群友
            </el-button>
          </div>
        </section>
      </div>

      <template #footer>
        <el-button @click="groupDialogOpen = false">取消</el-button>
        <el-button type="primary" @click="createGroup">创建群聊</el-button>
      </template>
    </el-dialog>

    <el-drawer
      v-model="settingsOpen"
      title="群聊设置"
      size="560px"
      direction="rtl"
      class="settings-drawer"
    >
      <el-tabs>
        <el-tab-pane label="Provider Key">
          <div class="settings-stack">
            <section
              v-for="provider in providers"
              :key="provider.id"
              class="settings-card"
            >
              <div class="settings-card-head">
                <strong>{{ provider.name }}</strong>
                <el-tag size="small">{{ provider.id }}</el-tag>
              </div>

              <el-form label-position="top">
                <el-form-item label="API Key">
                  <el-input
                    v-model="provider.apiKey"
                    type="password"
                    show-password
                    placeholder="sk-..."
                  />
                </el-form-item>
                <el-form-item label="Base URL">
                  <el-input v-model="provider.baseUrl" />
                </el-form-item>
                <el-form-item label="默认模型">
                  <el-select
                    v-model="provider.defaultModel"
                    filterable
                    allow-create
                    default-first-option
                  >
                    <el-option
                      v-for="preset in modelPresets[provider.id]"
                      :key="preset"
                      :label="preset"
                      :value="preset"
                    />
                  </el-select>
                </el-form-item>
              </el-form>
            </section>
          </div>
        </el-tab-pane>

        <el-tab-pane label="当前群友">
          <div class="settings-toolbar">
            <el-button type="primary" :icon="CirclePlus" @click="addMember('openai')">
              添加 ChatGPT
            </el-button>
            <el-button :icon="CirclePlus" @click="addMember('deepseek')">
              添加 DeepSeek
            </el-button>
            <el-button type="danger" plain :icon="Delete" @click="removeGroup(activeGroup?.id ?? '')">
              删除群
            </el-button>
          </div>

          <div class="settings-stack">
            <section v-for="member in activeGroupMembers" :key="member.id" class="settings-card">
              <div class="settings-card-head">
                <el-input v-model="member.name" class="model-name-input" />
                <el-switch v-model="member.enabled" />
              </div>

              <el-form label-position="top">
                <el-form-item label="API">
                  <el-select v-model="member.provider" @change="updateMemberProvider(member)">
                    <el-option
                      v-for="option in providerOptions"
                      :key="option.value"
                      :label="option.label"
                      :value="option.value"
                    />
                  </el-select>
                </el-form-item>
                <el-form-item label="模型">
                  <el-select
                    v-model="member.model"
                    filterable
                    allow-create
                    default-first-option
                  >
                    <el-option
                      v-for="preset in modelPresets[member.provider]"
                      :key="preset"
                      :label="preset"
                      :value="preset"
                    />
                  </el-select>
                </el-form-item>
                <el-form-item label="温度">
                  <el-slider
                    v-model="member.temperature"
                    :min="0"
                    :max="2"
                    :step="0.1"
                    show-input
                  />
                </el-form-item>
                <el-form-item label="核心角色">
                  <el-input
                    v-model="member.systemPrompt"
                    type="textarea"
                    :autosize="{ minRows: 3, maxRows: 6 }"
                    resize="none"
                  />
                </el-form-item>
              </el-form>

              <div class="card-actions">
                <el-button :icon="DocumentCopy" @click="duplicateMember(member)">
                  复制
                </el-button>
                <el-button type="danger" plain :icon="Delete" @click="removeMember(member.id)">
                  删除
                </el-button>
              </div>
            </section>
          </div>
        </el-tab-pane>

        <el-tab-pane label="群聊说明">
          <div class="settings-card docs-card">
            <h3>聊天群</h3>
            <p>每个聊天群都是一个独立对话框，保存自己的聊天记录和虚拟群友。</p>
            <h3>虚拟群友</h3>
            <p>每个群友都可以选择 ChatGPT 或 DeepSeek，配置模型、温度和核心角色提示词。</p>
            <h3>上下文</h3>
            <p>发送消息时，当前群里启用的群友共享同一段最近聊天上下文，并分别回复。</p>
          </div>
        </el-tab-pane>
      </el-tabs>
    </el-drawer>
  </div>
</template>

<style>
:root {
  font-family:
    Inter, "Microsoft YaHei", "PingFang SC", "Helvetica Neue", Arial, sans-serif;
  color: #1d2521;
  background: #eef1ed;
  font-synthesis: none;
  text-rendering: optimizeLegibility;
  -webkit-font-smoothing: antialiased;
  -moz-osx-font-smoothing: grayscale;
  -webkit-text-size-adjust: 100%;
}

* {
  box-sizing: border-box;
}

html,
body,
#app {
  min-width: 320px;
  min-height: 100vh;
  margin: 0;
}

button,
textarea,
input {
  font: inherit;
}

.app-shell {
  display: grid;
  grid-template-columns: 300px minmax(0, 1fr);
  gap: 16px;
  min-height: 100vh;
  padding: 16px;
  background:
    linear-gradient(180deg, rgba(255, 255, 255, 0.78), rgba(255, 255, 255, 0)),
    #eef1ed;
}

.side-panel,
.chat-workspace {
  min-width: 0;
  border: 1px solid #d9ded8;
  border-radius: 8px;
  background: #fbfcfb;
  box-shadow: 0 14px 34px rgba(31, 43, 36, 0.08);
}

.side-panel {
  display: flex;
  flex-direction: column;
  gap: 18px;
  padding: 18px;
}

.brand-block {
  display: flex;
  align-items: center;
  gap: 12px;
}

.brand-mark {
  display: grid;
  width: 44px;
  height: 44px;
  place-items: center;
  border-radius: 8px;
  color: #ffffff;
  background: #2e6f5b;
}

.brand-mark .el-icon {
  font-size: 23px;
}

.eyebrow {
  margin: 0 0 4px;
  color: #778279;
  font-size: 12px;
  font-weight: 700;
  letter-spacing: 0;
  text-transform: uppercase;
}

h1,
h2,
h3,
p {
  margin: 0;
}

h1 {
  color: #18221d;
  font-size: 22px;
  line-height: 1.2;
}

h2 {
  color: #18221d;
  font-size: 24px;
  line-height: 1.2;
}

.section-block {
  display: flex;
  flex: 1;
  flex-direction: column;
  gap: 12px;
  min-height: 0;
}

.section-heading,
.settings-card-head,
.message-meta {
  display: flex;
  align-items: center;
  gap: 10px;
}

.section-heading,
.settings-card-head {
  justify-content: space-between;
}

.section-heading {
  color: #2f3833;
  font-size: 14px;
  font-weight: 700;
}

.group-list,
.settings-stack {
  display: grid;
  gap: 10px;
}

.group-list {
  overflow: auto;
}

.group-card {
  display: flex;
  align-items: center;
  gap: 11px;
  min-height: 66px;
  padding: 10px 12px;
  border: 1px solid #e0e5df;
  border-radius: 8px;
  background: #ffffff;
  cursor: pointer;
  text-align: left;
}

.group-card.active {
  border-color: #91bda9;
  background: #f0f8f4;
}

.group-avatar {
  display: grid;
  width: 40px;
  height: 40px;
  flex: 0 0 auto;
  place-items: center;
  border-radius: 8px;
  color: #ffffff;
  background: #3a7c67;
}

.group-main {
  min-width: 0;
}

.group-main strong,
.group-main span {
  display: block;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.group-main strong {
  color: #202b25;
  font-size: 15px;
}

.group-main span {
  margin-top: 4px;
  color: #727c74;
  font-size: 12px;
}

.side-actions {
  display: grid;
  gap: 10px;
}

.chat-workspace {
  display: flex;
  flex-direction: column;
  min-height: calc(100vh - 32px);
  overflow: hidden;
}

.chat-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 16px;
  padding: 18px;
  border-bottom: 1px solid #e2e6e1;
  background: #ffffff;
}

.group-description {
  margin-top: 6px;
  color: #68746d;
  font-size: 13px;
}

.header-actions {
  display: flex;
  align-items: center;
  gap: 10px;
}

.messages-panel {
  display: flex;
  flex: 1;
  flex-direction: column;
  gap: 12px;
  min-height: 300px;
  padding: 18px;
  overflow: auto;
  background: #f6f7f4;
}

.message-row {
  width: min(920px, 100%);
  padding: 14px 14px 12px;
  border: 1px solid #e0e5df;
  border-radius: 8px;
  background: #ffffff;
}

.message-row.user {
  align-self: flex-end;
  background: #f8fbff;
}

.message-row.assistant {
  align-self: flex-start;
}

.message-meta {
  min-height: 28px;
}

.accent-line {
  width: 4px;
  height: 28px;
  flex: 0 0 auto;
  border-radius: 999px;
  background: var(--accent);
}

.message-title {
  display: flex;
  flex: 1;
  align-items: baseline;
  min-width: 0;
  gap: 8px;
}

.message-title strong {
  overflow: hidden;
  color: #1c2821;
  font-size: 15px;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.message-title span,
.message-meta time {
  color: #7a837b;
  font-size: 12px;
}

.status-pill {
  display: inline-flex;
  flex: 0 0 auto;
  align-items: center;
  justify-content: center;
  min-width: 52px;
  min-height: 24px;
  border-radius: 999px;
  padding: 2px 9px;
  font-size: 12px;
  font-weight: 700;
}

.status-pill.done {
  color: #2f705c;
  background: #e8f4ed;
}

.status-pill.thinking {
  color: #8a6020;
  background: #fff1d7;
}

.status-pill.error {
  color: #a13f3f;
  background: #fdecec;
}

.message-body {
  margin-top: 12px;
  overflow-wrap: anywhere;
  color: #202924;
  font-size: 14px;
  line-height: 1.65;
}

.message-body > :first-child {
  margin-top: 0;
}

.message-body > :last-child {
  margin-bottom: 0;
}

.message-body h1,
.message-body h2,
.message-body h3 {
  margin: 12px 0 8px;
  color: #18221d;
  font-size: 16px;
  line-height: 1.35;
}

.message-body p,
.message-body ul,
.message-body ol,
.message-body table,
.message-body pre,
.message-body blockquote {
  margin: 0 0 10px;
}

.message-body ul,
.message-body ol {
  padding-left: 20px;
}

.message-body code,
.docs-card code {
  border-radius: 5px;
  padding: 2px 5px;
  color: #365f51;
  background: #edf4ef;
  font-family: "Cascadia Code", "Fira Code", Consolas, monospace;
  font-size: 13px;
}

.message-body pre {
  overflow: auto;
  border: 1px solid #dbe4dd;
  border-radius: 8px;
  padding: 12px;
  background: #17201c;
}

.message-body pre code {
  padding: 0;
  color: #edf6f0;
  background: transparent;
}

.message-body blockquote {
  border-left: 3px solid #d6b36c;
  padding: 8px 10px;
  color: #555f58;
  background: #fff8e7;
}

.message-body table {
  width: 100%;
  border-collapse: collapse;
  font-size: 13px;
}

.message-body th,
.message-body td {
  border: 1px solid #dde4dd;
  padding: 7px 9px;
  text-align: left;
}

.message-body th {
  background: #eef4ef;
}

.composer {
  display: flex;
  align-items: flex-end;
  gap: 10px;
  padding: 14px 18px;
  border-top: 1px solid #e2e6e1;
  background: #ffffff;
}

.composer .el-textarea {
  flex: 1;
}

.composer .el-textarea__inner {
  border-radius: 8px;
  box-shadow: none;
}

.composer .el-button {
  flex-shrink: 0;
}

.settings-stack {
  padding-bottom: 16px;
}

.settings-stack.compact {
  max-height: 420px;
  overflow: auto;
}

.settings-toolbar {
  display: flex;
  flex-wrap: wrap;
  gap: 10px;
  margin-bottom: 14px;
}

.settings-card {
  padding: 14px;
  border: 1px solid #e0e5df;
  border-radius: 8px;
  background: #ffffff;
}

.settings-card-head {
  margin-bottom: 12px;
}

.model-name-input {
  flex: 1;
}

.member-grid {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: 12px;
}

.card-actions {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
}

.docs-card {
  display: grid;
  gap: 10px;
  color: #4d5851;
  line-height: 1.65;
}

.docs-card h3 {
  color: #18221d;
  font-size: 16px;
}

@media (max-width: 980px) {
  .app-shell {
    grid-template-columns: 1fr;
  }

  .chat-workspace {
    order: -1;
    min-height: 720px;
  }
}

@media (max-width: 700px) {
  .app-shell {
    gap: 12px;
    padding: 12px;
  }

  .chat-header,
  .header-actions,
  .composer,
  .settings-toolbar,
  .member-grid {
    align-items: stretch;
    flex-direction: column;
    grid-template-columns: 1fr;
  }

  .composer .el-button,
  .settings-toolbar .el-button {
    width: 100%;
  }

  .message-meta {
    align-items: flex-start;
    flex-wrap: wrap;
  }

  .message-title {
    flex-basis: calc(100% - 16px);
  }
}
</style>
