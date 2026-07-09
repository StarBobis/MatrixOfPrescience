<script setup lang="ts">
import { ref } from "vue";
import TitleBar from "./views/TitleBar.vue";
import ChatGroupPage from "./views/ChatGroupPage.vue";
import SettingsPage from "./views/SettingsPage.vue";

type PageName = "chat" | "settings";

const currentPage = ref<PageName>("chat");
const previousPage = ref<PageName>("chat");

function openChatPage() {
  currentPage.value = "chat";
}

function toggleSettingsPage() {
  if (currentPage.value === "settings") {
    currentPage.value = previousPage.value;
    return;
  }

  previousPage.value = currentPage.value;
  currentPage.value = "settings";
}

function openSettingsPage() {
  if (currentPage.value !== "settings") {
    previousPage.value = currentPage.value;
  }

  currentPage.value = "settings";
}
</script>

<template>
  <div class="app-frame" @contextmenu.prevent>
    <TitleBar
      :settings-active="currentPage === 'settings'"
      @home="openChatPage"
      @toggle-settings="toggleSettingsPage"
    />
    <ChatGroupPage v-if="currentPage === 'chat'" @open-settings="openSettingsPage" />
    <SettingsPage v-else />
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
  height: 100vh;
  min-height: 100vh;
  margin: 0;
  overflow: hidden;
}

button,
textarea,
input {
  font: inherit;
}

.app-frame {
  display: flex;
  height: 100vh;
  min-height: 0;
  flex-direction: column;
  background:
    linear-gradient(180deg, rgba(255, 255, 255, 0.78), rgba(255, 255, 255, 0)),
    #eef1ed;
}
</style>
