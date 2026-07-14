<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref } from "vue";
import TitleBar from "./views/TitleBar.vue";
import ChatGroupPage from "./views/ChatGroupPage.vue";
import SettingsPage from "./views/SettingsPage.vue";
import FriendLibraryPage from "./views/FriendLibraryPage.vue";

type PageName = "chat" | "settings" | "friends";

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

function toggleFriendsPage() {
  if (currentPage.value === "friends") {
    currentPage.value = previousPage.value;
    return;
  }

  previousPage.value = currentPage.value;
  currentPage.value = "friends";
}

function handleAppShortcut(event: KeyboardEvent) {
  const modalOpen = Array.from(
    document.querySelectorAll<HTMLElement>('[aria-modal="true"]'),
  ).some((element) => element.getClientRects().length > 0);

  if (
    !event.defaultPrevented &&
    !modalOpen &&
    (event.metaKey || event.ctrlKey) &&
    event.key === ","
  ) {
    event.preventDefault();
    openSettingsPage();
  }
}

onMounted(() => window.addEventListener("keydown", handleAppShortcut));
onBeforeUnmount(() => window.removeEventListener("keydown", handleAppShortcut));
</script>

<template>
  <div class="app-frame">
    <TitleBar
      :chat-active="currentPage === 'chat'"
      :settings-active="currentPage === 'settings'"
      :friends-active="currentPage === 'friends'"
      @home="openChatPage"
      @toggle-friends="toggleFriendsPage"
      @toggle-settings="toggleSettingsPage"
    />
    <ChatGroupPage v-show="currentPage === 'chat'" @open-settings="openSettingsPage" />
    <SettingsPage v-if="currentPage === 'settings'" />
    <FriendLibraryPage v-else-if="currentPage === 'friends'" />
  </div>
</template>

<style>
.app-frame {
  display: flex;
  height: 100vh;
  min-height: 0;
  flex-direction: column;
  color: var(--text-primary);
  background: var(--app-bg);
}
</style>
