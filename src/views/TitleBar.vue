<script setup lang="ts">
import { getCurrentWindow } from "@tauri-apps/api/window";
import { BrainCircuit, Maximize2, Minus, Settings, Users, X } from "@lucide/vue";
import { useI18n } from "vue-i18n";

defineProps<{
  chatActive: boolean;
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
    <button
      class="titlebar-brand"
      :class="{ active: chatActive }"
      type="button"
      :title="t('navigation.chats')"
      :aria-label="t('navigation.chats')"
      :aria-current="chatActive ? 'page' : undefined"
      @click="emit('home')"
    >
      <span class="titlebar-mark">
        <BrainCircuit aria-hidden="true" />
      </span>
      <span class="titlebar-name">Matrix Of Prescience</span>
    </button>

    <nav class="titlebar-navigation" :aria-label="t('navigation.primary')">
      <button
        class="titlebar-tool"
        :class="{ active: friendsActive }"
        type="button"
        :title="t('common.friendLibrary')"
        :aria-label="t('common.friendLibrary')"
        :aria-pressed="friendsActive"
        @click="emit('toggleFriends')"
      >
        <Users aria-hidden="true" />
      </button>
      <button
        class="titlebar-tool"
        :class="{ active: settingsActive }"
        type="button"
        :title="t('common.settings')"
        :aria-label="t('common.settings')"
        :aria-pressed="settingsActive"
        @click="emit('toggleSettings')"
      >
        <Settings aria-hidden="true" />
      </button>
    </nav>

    <div class="window-controls" role="group" :aria-label="t('windowControls.group')">
      <button
        class="window-button"
        type="button"
        :title="t('windowControls.minimize')"
        :aria-label="t('windowControls.minimize')"
        @click="minimizeWindow"
      >
        <Minus aria-hidden="true" />
      </button>
      <button
        class="window-button"
        type="button"
        :title="t('windowControls.maximize')"
        :aria-label="t('windowControls.maximize')"
        @click="toggleMaximizeWindow"
      >
        <Maximize2 aria-hidden="true" />
      </button>
      <button
        class="window-button close"
        type="button"
        :title="t('windowControls.close')"
        :aria-label="t('windowControls.close')"
        @click="closeWindow"
      >
        <X aria-hidden="true" />
      </button>
    </div>
  </header>
</template>

<style scoped>
.app-titlebar {
  display: flex;
  z-index: 50;
  height: 48px;
  flex: 0 0 48px;
  align-items: center;
  border-bottom: 1px solid var(--separator);
  background: color-mix(in srgb, var(--surface-elevated) 92%, transparent);
  backdrop-filter: saturate(150%) blur(18px);
  user-select: none;
}

.titlebar-brand {
  display: flex;
  height: 100%;
  align-items: center;
  gap: 10px;
  min-width: 0;
  max-width: 320px;
  padding: 0 16px;
  border: 0;
  border-right: 1px solid var(--separator);
  color: var(--text-primary);
  font-size: 13px;
  font-weight: 700;
  background: transparent;
  cursor: pointer;
}

.titlebar-brand:hover,
.titlebar-brand.active {
  background: var(--control-bg);
}

.titlebar-brand.active {
  box-shadow: inset 0 -2px var(--accent);
}

.titlebar-mark {
  display: grid;
  width: 28px;
  height: 28px;
  flex: 0 0 auto;
  place-items: center;
  border-radius: 7px;
  color: #ffffff;
  background: var(--accent);
}

.titlebar-mark svg {
  width: 17px;
  height: 17px;
  stroke-width: 1.9;
}

.titlebar-name {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.titlebar-navigation {
  display: flex;
  height: 100%;
  align-items: center;
  gap: 4px;
  margin-left: auto;
  padding: 0 10px;
  border-right: 1px solid var(--separator);
}

.titlebar-tool,
.window-button {
  display: grid;
  place-items: center;
  border: 0;
  color: var(--text-secondary);
  background: transparent;
  cursor: pointer;
}

.titlebar-tool {
  width: 34px;
  height: 34px;
  border-radius: 7px;
}

.titlebar-tool:hover,
.titlebar-tool.active {
  color: var(--accent-text);
  background: var(--accent-soft);
}

.titlebar-tool svg,
.window-button svg {
  width: 16px;
  height: 16px;
  stroke-width: 1.8;
}

.window-controls {
  display: flex;
  height: 100%;
}

.window-button {
  width: 44px;
  height: 100%;
}

.window-button:hover {
  color: var(--text-primary);
  background: var(--control-hover);
}

.window-button.close:hover {
  color: #ffffff;
  background: var(--danger-fill);
}

@media (prefers-reduced-transparency: reduce) {
  .app-titlebar {
    background: var(--surface);
    backdrop-filter: none;
  }
}

@media (max-width: 620px) {
  .titlebar-name {
    display: none;
  }

  .titlebar-brand {
    padding-inline: 10px;
  }

  .titlebar-navigation {
    padding-inline: 6px;
  }

  .window-button {
    width: 40px;
  }
}
</style>
