<script setup lang="ts">
import { computed, ref } from "vue";
import MarkdownIt from "markdown-it";
import {
  ChatDotRound,
  Connection,
  Cpu,
  DataAnalysis,
  Document,
  MagicStick,
  Operation,
  Promotion,
  Setting,
  Tickets,
} from "@element-plus/icons-vue";

type MessageStatus = "done" | "thinking" | "queued";

interface AgentMessage {
  id: number;
  model: string;
  role: string;
  status: MessageStatus;
  time: string;
  accent: string;
  content: string;
}

interface ModelRoute {
  name: string;
  role: string;
  active: boolean;
  color: string;
}

interface QueueItem {
  title: string;
  owner: string;
  progress: number;
}

interface KnowledgeSlot {
  title: string;
  meta: string;
  tone: "warm" | "cool" | "plain";
}

interface PluginSlot {
  title: string;
  state: string;
}

const markdown = new MarkdownIt({
  breaks: true,
  html: false,
  linkify: true,
  typographer: true,
});

const statusLabelMap: Record<MessageStatus, string> = {
  done: "完成",
  thinking: "推理中",
  queued: "排队",
};

const selectedRoute = ref("auto");
const composer = ref(`整理这个 Agent 的第一轮目标：

- 识别任务类型
- 选择合适模型
- 给出下一步行动`);

const models = ref<ModelRoute[]>([
  {
    name: "Router",
    role: "意图识别与调度",
    active: true,
    color: "#3a7c67",
  },
  {
    name: "Reasoner",
    role: "复杂规划与拆解",
    active: true,
    color: "#b07c2b",
  },
  {
    name: "Coder",
    role: "代码生成与修复",
    active: true,
    color: "#416c9a",
  },
  {
    name: "Reviewer",
    role: "结果复核与风险提示",
    active: false,
    color: "#8b5f77",
  },
]);

const taskQueue = ref<QueueItem[]>([
  {
    title: "分析输入意图",
    owner: "Router",
    progress: 100,
  },
  {
    title: "拆解执行步骤",
    owner: "Reasoner",
    progress: 68,
  },
  {
    title: "等待工具调用",
    owner: "Coder",
    progress: 24,
  },
]);

const knowledgeSlots: KnowledgeSlot[] = [
  {
    title: "项目上下文",
    meta: "Rust / Tauri / Vue",
    tone: "warm",
  },
  {
    title: "会话记忆",
    meta: "最近 12 条消息",
    tone: "cool",
  },
  {
    title: "文件与工具",
    meta: "待接入",
    tone: "plain",
  },
];

const pluginSlots: PluginSlot[] = [
  {
    title: "模型供应商",
    state: "预留",
  },
  {
    title: "工具调用",
    state: "预留",
  },
  {
    title: "任务产物",
    state: "预留",
  },
];

const messages = ref<AgentMessage[]>([
  {
    id: 1,
    model: "Router",
    role: "任务调度",
    status: "done",
    time: "09:18",
    accent: "#3a7c67",
    content:
      "### 路由结论\n\n当前输入属于 **产品原型 + 前端实现** 任务，建议先调用规划模型拆解界面，再交给代码模型实现 Vue 组件。",
  },
  {
    id: 2,
    model: "Reasoner",
    role: "规划模型",
    status: "done",
    time: "09:19",
    accent: "#b07c2b",
    content:
      "我会将工作台拆成三块：\n\n1. 左侧：模型路由、任务队列\n2. 中间：多模型消息流和输入区\n3. 右侧：上下文、工具、产物预留\n\n| 区域 | 目的 |\n| --- | --- |\n| 左侧 | 决策与队列 |\n| 中间 | 协作对话 |\n| 右侧 | 上下文扩展 |",
  },
  {
    id: 3,
    model: "Coder",
    role: "实现模型",
    status: "thinking",
    time: "09:20",
    accent: "#416c9a",
    content:
      "```ts\nconst nextStep = routeTask(input, enabledModels);\nawait dispatch(nextStep.model, nextStep.payload);\n```\n\n界面层已经保留调度入口，后续可以把这段替换成 Tauri command 或本地 Rust 服务调用。",
  },
]);

const activeModelCount = computed(
  () => models.value.filter((model) => model.active).length,
);

function renderMarkdown(source: string) {
  return markdown.render(source);
}

function sendDraft() {
  const draft = composer.value.trim();

  if (!draft) {
    return;
  }

  messages.value.push({
    id: Date.now(),
    model: "Input",
    role: "任务入口",
    status: "queued",
    time: "刚刚",
    accent: "#5f6f52",
    content: `### 新任务\n\n${draft}\n\n> 已进入调度队列。`,
  });

  composer.value = "";
}
</script>

<template>
  <div class="app-shell">
    <aside class="panel panel-left">
      <div class="brand-block">
        <div class="brand-mark">
          <el-icon>
            <MagicStick />
          </el-icon>
        </div>
        <div>
          <p class="eyebrow">Matrix Of Prescience</p>
          <h1>大模型 Agent</h1>
        </div>
      </div>

      <section class="panel-section">
        <div class="section-heading">
          <span>
            <el-icon>
              <Cpu />
            </el-icon>
            模型路由
          </span>
          <el-tag size="small" type="success">{{ activeModelCount }} 启用</el-tag>
        </div>

        <div class="model-list">
          <div
            v-for="model in models"
            :key="model.name"
            class="model-item"
            :class="{ active: model.active }"
          >
            <div class="model-main">
              <span class="model-dot" :style="{ background: model.color }"></span>
              <div>
                <strong>{{ model.name }}</strong>
                <p>{{ model.role }}</p>
              </div>
            </div>
            <el-switch v-model="model.active" size="small" />
          </div>
        </div>
      </section>

      <section class="panel-section">
        <div class="section-heading">
          <span>
            <el-icon>
              <Operation />
            </el-icon>
            任务队列
          </span>
          <el-button link type="primary" :icon="Tickets">队列</el-button>
        </div>

        <div class="task-list">
          <div v-for="task in taskQueue" :key="task.title" class="task-item">
            <div class="task-copy">
              <strong>{{ task.title }}</strong>
              <span>{{ task.owner }}</span>
            </div>
            <el-progress
              :percentage="task.progress"
              :show-text="false"
              :stroke-width="6"
            />
          </div>
        </div>
      </section>
    </aside>

    <main class="chat-workspace">
      <header class="chat-header">
        <div>
          <p class="eyebrow">Agent Conversation</p>
          <h2>多模型协作会话</h2>
        </div>

        <div class="route-controls">
          <el-select v-model="selectedRoute" size="large" class="route-select">
            <el-option label="自动路由" value="auto" />
            <el-option label="代码优先" value="code" />
            <el-option label="规划优先" value="plan" />
            <el-option label="复核优先" value="review" />
          </el-select>
          <el-button type="primary" :icon="Promotion" @click="sendDraft">
            开始调度
          </el-button>
        </div>
      </header>

      <section class="messages-panel">
        <article
          v-for="message in messages"
          :key="message.id"
          class="message-row"
          :style="{ '--accent': message.accent }"
        >
          <div class="message-meta">
            <span class="accent-line"></span>
            <div class="message-title">
              <strong>{{ message.model }}</strong>
              <span>{{ message.role }}</span>
            </div>
            <span class="status-pill" :class="message.status">
              {{ statusLabelMap[message.status] }}
            </span>
            <time>{{ message.time }}</time>
          </div>

          <div class="message-body" v-html="renderMarkdown(message.content)"></div>
        </article>
      </section>

      <footer class="composer">
        <div class="composer-toolbar">
          <span>
            <el-icon>
              <ChatDotRound />
            </el-icon>
            Markdown 草稿
          </span>
          <el-tag size="small" type="info">.md</el-tag>
        </div>

        <el-input
          v-model="composer"
          type="textarea"
          :autosize="{ minRows: 4, maxRows: 8 }"
          resize="none"
          placeholder="输入任务或 Markdown 内容"
        />

        <div class="composer-actions">
          <span>{{ composer.trim().length }} 字符</span>
          <el-button
            type="primary"
            :disabled="!composer.trim()"
            :icon="Promotion"
            @click="sendDraft"
          >
            发送到调度器
          </el-button>
        </div>
      </footer>
    </main>

    <aside class="panel panel-right">
      <section class="panel-section">
        <div class="section-heading">
          <span>
            <el-icon>
              <Document />
            </el-icon>
            上下文
          </span>
          <el-button link type="primary" :icon="Setting">设置</el-button>
        </div>

        <div class="context-list">
          <div
            v-for="slot in knowledgeSlots"
            :key="slot.title"
            class="context-item"
            :class="slot.tone"
          >
            <strong>{{ slot.title }}</strong>
            <span>{{ slot.meta }}</span>
          </div>
        </div>
      </section>

      <section class="panel-section metrics-section">
        <div class="section-heading">
          <span>
            <el-icon>
              <DataAnalysis />
            </el-icon>
            运行指标
          </span>
        </div>

        <div class="metric-grid">
          <div>
            <strong>3</strong>
            <span>模型消息</span>
          </div>
          <div>
            <strong>2</strong>
            <span>工具槽位</span>
          </div>
          <div>
            <strong>{{ activeModelCount }}</strong>
            <span>启用模型</span>
          </div>
        </div>
      </section>

      <section class="panel-section">
        <div class="section-heading">
          <span>
            <el-icon>
              <Connection />
            </el-icon>
            扩展区域
          </span>
        </div>

        <div class="plugin-list">
          <button v-for="slot in pluginSlots" :key="slot.title" class="plugin-item">
            <span>{{ slot.title }}</span>
            <em>{{ slot.state }}</em>
          </button>
        </div>
      </section>
    </aside>
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
  grid-template-columns: 260px minmax(0, 1fr) 300px;
  gap: 16px;
  min-height: 100vh;
  padding: 16px;
  background:
    linear-gradient(180deg, rgba(255, 255, 255, 0.76), rgba(255, 255, 255, 0)),
    #eef1ed;
}

.panel,
.chat-workspace {
  min-width: 0;
  border: 1px solid #d9ded8;
  border-radius: 8px;
  background: #fbfcfb;
  box-shadow: 0 14px 34px rgba(31, 43, 36, 0.08);
}

.panel {
  display: flex;
  flex-direction: column;
  gap: 22px;
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

.panel-section {
  display: flex;
  flex-direction: column;
  gap: 12px;
}

.section-heading {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  color: #2f3833;
  font-size: 14px;
  font-weight: 700;
}

.section-heading span {
  display: inline-flex;
  align-items: center;
  min-width: 0;
  gap: 7px;
}

.section-heading .el-icon {
  color: #3a7c67;
}

.model-list,
.task-list,
.context-list,
.plugin-list {
  display: grid;
  gap: 10px;
}

.model-item,
.task-item,
.context-item,
.plugin-item,
.message-row {
  border: 1px solid #e0e5df;
  border-radius: 8px;
  background: #ffffff;
}

.model-item {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 10px;
  min-height: 68px;
  padding: 12px;
  opacity: 0.62;
}

.model-item.active {
  opacity: 1;
  border-color: #c9d9d0;
  background: #f9fbf8;
}

.model-main {
  display: flex;
  align-items: center;
  min-width: 0;
  gap: 10px;
}

.model-dot {
  width: 10px;
  height: 10px;
  flex: 0 0 auto;
  border-radius: 999px;
}

.model-main strong,
.task-copy strong,
.context-item strong,
.plugin-item span {
  display: block;
  overflow: hidden;
  color: #202b25;
  font-size: 14px;
  line-height: 1.35;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.model-main p,
.task-copy span,
.context-item span,
.plugin-item em {
  display: block;
  overflow: hidden;
  margin-top: 3px;
  color: #727c74;
  font-size: 12px;
  font-style: normal;
  line-height: 1.35;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.task-item {
  display: grid;
  gap: 10px;
  padding: 12px;
}

.task-copy {
  display: flex;
  align-items: center;
  justify-content: space-between;
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

.route-controls {
  display: flex;
  align-items: center;
  gap: 10px;
}

.route-select {
  width: 154px;
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
  padding: 14px 14px 12px;
}

.message-meta {
  display: flex;
  align-items: center;
  gap: 10px;
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

.status-pill.queued {
  color: #50606a;
  background: #eef3f4;
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

.message-body code {
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
  display: grid;
  gap: 10px;
  padding: 14px 18px 18px;
  border-top: 1px solid #e2e6e1;
  background: #ffffff;
}

.composer-toolbar,
.composer-actions {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 10px;
  color: #6f7a72;
  font-size: 13px;
}

.composer-toolbar span {
  display: inline-flex;
  align-items: center;
  gap: 7px;
  font-weight: 700;
}

.composer .el-textarea__inner {
  border-radius: 8px;
  box-shadow: none;
}

.context-item {
  min-height: 62px;
  padding: 12px;
}

.context-item.warm {
  border-color: #ead9b6;
  background: #fffaf0;
}

.context-item.cool {
  border-color: #cbdce2;
  background: #f1f7f8;
}

.metrics-section {
  padding: 14px;
  border: 1px solid #e1e6df;
  border-radius: 8px;
  background: #f7faf6;
}

.metric-grid {
  display: grid;
  grid-template-columns: repeat(3, minmax(0, 1fr));
  gap: 8px;
}

.metric-grid div {
  display: grid;
  gap: 2px;
  min-height: 58px;
  place-items: center;
  border: 1px solid #dfe8df;
  border-radius: 8px;
  background: #ffffff;
}

.metric-grid strong {
  color: #2e6f5b;
  font-size: 22px;
  line-height: 1;
}

.metric-grid span {
  color: #718078;
  font-size: 12px;
}

.plugin-item {
  display: flex;
  width: 100%;
  align-items: center;
  justify-content: space-between;
  gap: 10px;
  min-height: 44px;
  padding: 10px 12px;
  cursor: pointer;
  text-align: left;
}

.plugin-item:hover {
  border-color: #b9cfc2;
  background: #f8fbf8;
}

@media (max-width: 1180px) {
  .app-shell {
    grid-template-columns: 236px minmax(0, 1fr) 260px;
  }
}

@media (max-width: 980px) {
  .app-shell {
    grid-template-columns: 1fr;
  }

  .chat-workspace {
    order: -1;
    min-height: 720px;
  }

  .panel {
    min-height: auto;
  }
}

@media (max-width: 700px) {
  .app-shell {
    gap: 12px;
    padding: 12px;
  }

  .chat-header,
  .route-controls,
  .composer-actions {
    align-items: stretch;
    flex-direction: column;
  }

  .route-select,
  .route-controls .el-button,
  .composer-actions .el-button {
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
