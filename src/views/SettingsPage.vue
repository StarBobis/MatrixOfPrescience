<script setup lang="ts">
import { storeToRefs } from "pinia";
import { Setting, UserFilled } from "@element-plus/icons-vue";
import { type ProviderId, useSettingsStore } from "../stores/settings";

const settingsStore = useSettingsStore();
const { providers, ownerProfile } = storeToRefs(settingsStore);

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
</script>

<template>
  <main class="settings-page">
    <section class="settings-hero">
      <div class="settings-hero-icon">
        <el-icon>
          <Setting />
        </el-icon>
      </div>
      <div>
        <p class="settings-eyebrow">Settings</p>
        <h1>全局设置</h1>
      </div>
    </section>

    <div class="settings-layout">
      <section class="settings-panel">
        <div class="settings-panel-head">
          <strong>我的资料</strong>
          <el-tag size="small" type="success">群主</el-tag>
        </div>

        <div class="owner-preview">
          <span class="owner-avatar" :style="{ background: ownerProfile.color }">
            <img v-if="ownerProfile.avatar" :src="ownerProfile.avatar" alt="" />
            <el-icon v-else>
              <UserFilled />
            </el-icon>
          </span>
          <div>
            <strong>{{ ownerProfile.name || "我" }}</strong>
            <span>所有群聊中固定为群主</span>
          </div>
        </div>

        <el-form label-position="top">
          <el-form-item label="名称">
            <el-input v-model="ownerProfile.name" placeholder="我" />
          </el-form-item>
          <el-form-item label="头像 URL">
            <el-input v-model="ownerProfile.avatar" placeholder="https://..." />
          </el-form-item>
          <el-form-item label="头像底色">
            <el-color-picker v-model="ownerProfile.color" />
          </el-form-item>
        </el-form>
      </section>

      <section class="settings-panel">
        <div class="settings-panel-head">
          <strong>Provider Key</strong>
          <el-tag size="small">{{ providerOptions.length }}</el-tag>
        </div>

        <div class="provider-stack">
          <section v-for="provider in providers" :key="provider.id" class="provider-card">
            <div class="provider-card-head">
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
      </section>
    </div>
  </main>
</template>

<style scoped>
.settings-page {
  flex: 1;
  min-height: 0;
  overflow: auto;
  padding: 16px;
}

.settings-hero {
  display: flex;
  align-items: center;
  gap: 12px;
  margin-bottom: 16px;
  padding: 18px;
  border: 1px solid #d9ded8;
  border-radius: 8px;
  background: #fbfcfb;
  box-shadow: 0 14px 34px rgba(31, 43, 36, 0.08);
}

.settings-hero-icon {
  display: grid;
  width: 44px;
  height: 44px;
  place-items: center;
  border-radius: 8px;
  color: #ffffff;
  background: #2e6f5b;
}

.settings-eyebrow,
.owner-preview span {
  margin: 0;
  color: #778279;
  font-size: 12px;
  font-weight: 700;
  letter-spacing: 0;
  text-transform: uppercase;
}

h1 {
  margin: 0;
  color: #18221d;
  font-size: 22px;
  line-height: 1.2;
}

.settings-layout {
  display: grid;
  grid-template-columns: 360px minmax(0, 1fr);
  gap: 16px;
}

.settings-panel,
.provider-card {
  border: 1px solid #e0e5df;
  border-radius: 8px;
  background: #ffffff;
}

.settings-panel {
  display: grid;
  align-content: start;
  gap: 14px;
  padding: 16px;
}

.settings-panel-head,
.provider-card-head,
.owner-preview {
  display: flex;
  align-items: center;
  gap: 10px;
}

.settings-panel-head,
.provider-card-head {
  justify-content: space-between;
}

.owner-preview {
  padding: 12px;
  border: 1px solid #e0e5df;
  border-radius: 8px;
  background: #f7faf7;
}

.owner-avatar {
  display: grid;
  width: 48px;
  height: 48px;
  flex: 0 0 auto;
  place-items: center;
  overflow: hidden;
  border-radius: 50%;
  color: #ffffff;
  font-size: 22px;
}

.owner-avatar img {
  width: 100%;
  height: 100%;
  object-fit: cover;
}

.owner-preview strong,
.owner-preview span {
  display: block;
}

.owner-preview strong {
  color: #202b25;
  font-size: 15px;
}

.owner-preview span {
  margin-top: 4px;
  text-transform: none;
}

.provider-stack {
  display: grid;
  gap: 12px;
}

.provider-card {
  padding: 14px;
}

@media (max-width: 900px) {
  .settings-layout {
    grid-template-columns: 1fr;
  }
}
</style>
