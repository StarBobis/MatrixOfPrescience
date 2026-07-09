<script setup lang="ts">
import { getCurrentWindow } from "@tauri-apps/api/window";
import { Close, FullScreen, MagicStick, Minus, Setting, UserFilled } from "@element-plus/icons-vue";
import { useI18n } from "vue-i18n";

defineProps<{
  settingsActive: boolean;
  friendsActive: boolean;
}>();

const emit = defineEmits<{
  home: [];
  toggleFriends: [];
  toggleSettings: [];
}>();

const { t } = useI18n();

async function minimizeWindow() {
  await getCurrentWindow().minimize();
}

async function toggleMaximizeWindow() {
  await getCurrentWindow().toggleMaximize();
}

async function closeWindow() {
  await getCurrentWindow().close();
}
</script>

<template>
  <header class="app-titlebar" data-tauri-drag-region>
    <button class="titlebar-brand" type="button" @click="emit('home')">
      <span class="titlebar-mark">
        <el-icon>
          <MagicStick />
        </el-icon>
      </span>
      <span>Matrix Of Prescience</span>
    </button>

    <div class="window-controls">
      <button
        class="window-button"
        :class="{ active: friendsActive }"
        :title="t('common.friendLibrary')"
        :aria-label="t('common.friendLibrary')"
        @click="emit('toggleFriends')"
      >
        <el-icon>
          <UserFilled />
        </el-icon>
      </button>
      <button
        class="window-button"
        :class="{ active: settingsActive }"
        :title="t('common.settings')"
        :aria-label="t('common.settings')"
        @click="emit('toggleSettings')"
      >
        <el-icon>
          <Setting />
        </el-icon>
      </button>
      <button class="window-button" @click="minimizeWindow">
        <el-icon>
          <Minus />
        </el-icon>
      </button>
      <button class="window-button" @click="toggleMaximizeWindow">
        <el-icon>
          <FullScreen />
        </el-icon>
      </button>
      <button class="window-button close" @click="closeWindow">
        <el-icon>
          <Close />
        </el-icon>
      </button>
    </div>
  </header>
</template>

<style scoped>
.app-titlebar {
  display: flex;
  height: 42px;
  flex: 0 0 42px;
  align-items: center;
  justify-content: space-between;
  border-bottom: 1px solid #dbe1db;
  background: #f8faf7;
  user-select: none;
}

.titlebar-brand {
  display: flex;
  align-items: center;
  gap: 9px;
  min-width: 0;
  padding-left: 14px;
  border: 0;
  color: #26332d;
  font-size: 13px;
  font-weight: 700;
  background: transparent;
  cursor: pointer;
}

.titlebar-mark {
  display: grid;
  width: 24px;
  height: 24px;
  place-items: center;
  border-radius: 6px;
  color: #ffffff;
  background: #2e6f5b;
}

.window-controls {
  display: flex;
  height: 100%;
}

.window-button {
  display: grid;
  width: 46px;
  height: 100%;
  place-items: center;
  border: 0;
  color: #46544d;
  background: transparent;
  cursor: pointer;
}

.window-button:hover {
  background: #e9eeea;
}

.window-button.active {
  color: #ffffff;
  background: #2e6f5b;
}

.window-button.close:hover {
  color: #ffffff;
  background: #c84242;
}
</style>
