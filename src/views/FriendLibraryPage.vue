<script setup lang="ts">
import { computed, nextTick } from "vue";
import { ElMessage, ElMessageBox } from "element-plus";
import { ChevronDown, Pencil, Plus, Trash2, Users } from "@lucide/vue";
import { storeToRefs } from "pinia";
import { useI18n } from "vue-i18n";
import {
  type AgentModel,
  type AgentReasoningEffort,
  type ProviderId,
  useSettingsStore,
} from "../stores/settings";
import { chooseLocalAvatar, getAvatarSrc } from "../utils/avatar";
import { getReadableTextColor } from "../utils/colorContrast";
import { isDeepSeekProvider } from "../utils/modelCatalog";

const settingsStore = useSettingsStore();
const { friends, groups, providers } = storeToRefs(settingsStore);
const { t } = useI18n();

const providerOptions = computed<Array<{ label: string; value: ProviderId }>>(() =>
  (Object.values(providers.value) as Array<{ id: ProviderId; name: string }>).map((provider) => ({
    label: provider.name,
    value: provider.id,
  })),
);

const reasoningEffortOptions = computed<Array<{ label: string; value: AgentReasoningEffort }>>(
  () => [
    { label: t("members.reasoningEffortOptions.off"), value: "off" },
    { label: t("members.reasoningEffortOptions.low"), value: "low" },
    { label: t("members.reasoningEffortOptions.medium"), value: "medium" },
    { label: t("members.reasoningEffortOptions.high"), value: "high" },
  ],
);

const friendUsage = computed(() => {
  const usage = new Map<string, number>();

  for (const group of groups.value) {
    const usedInGroup = new Set<string>();

    for (const member of group.members) {
      if (member.libraryId) {
        usedInGroup.add(member.libraryId);
      }
    }

    for (const libraryId of usedInGroup) {
      usage.set(libraryId, (usage.get(libraryId) ?? 0) + 1);
    }
  }

  return usage;
});

function getInitial(name: string) {
  return name.trim().slice(0, 1) || "?";
}

function getFriendUsage(friend: AgentModel) {
  return friendUsage.value.get(friend.libraryId ?? friend.id) ?? 0;
}

function getProviderLabel(provider: ProviderId) {
  return providers.value[provider]?.name ?? provider;
}

async function assignFriendAvatar(friend: AgentModel) {
  const avatar = await chooseLocalAvatar();

  if (avatar) {
    friend.avatar = avatar;
    settingsStore.updateFriendProfile(friend);
  }
}

function addFriend(provider: ProviderId) {
  settingsStore.addFriend(provider);
}

function addFriendFromMenu(command: unknown) {
  if (typeof command === "string" && providers.value[command]) {
    addFriend(command);
  }
}

function renameFriend(friend: AgentModel) {
  settingsStore.renameFriend(friend.id, friend.name);
}

async function removeFriend(friend: AgentModel) {
  if (getFriendUsage(friend) > 0) {
    ElMessage.warning(t("messages.friendInUse"));
    return;
  }

  try {
    const friendIndex = friends.value.findIndex((item) => item.id === friend.id);
    await ElMessageBox.confirm(
      t("friends.confirmRemove.message", { name: friend.name }),
      t("friends.confirmRemove.title"),
      {
        confirmButtonText: t("friends.remove"),
        cancelButtonText: t("common.cancel"),
        confirmButtonClass: "el-button--danger",
        type: "warning",
      },
    );
    settingsStore.removeFriend(friend.id);
    void nextTick(() => {
      const nextFriend = friends.value[Math.min(friendIndex, friends.value.length - 1)];
      const nextRow = Array.from(
        document.querySelectorAll<HTMLElement>("[data-friend-id]"),
      ).find((element) => element.dataset.friendId === nextFriend?.id);

      (nextRow?.querySelector<HTMLElement>(".friend-name-block input") ??
        document.getElementById("friend-add-button"))?.focus();
    });
  } catch {
    // Canceling leaves the friend unchanged.
  }
}
</script>

<template>
  <main class="friend-library-page" aria-labelledby="friend-library-title">
    <div class="friend-library-content">
    <header class="friend-library-head">
      <div class="friend-library-title">
        <span class="friend-library-icon">
          <Users aria-hidden="true" />
        </span>
        <div>
          <p>{{ t("friends.eyebrow") }}</p>
          <h1 id="friend-library-title">{{ t("friends.title") }}</h1>
        </div>
      </div>

      <div class="friend-library-actions">
        <el-tag type="info">{{ t("friends.count", { count: friends.length }) }}</el-tag>
        <el-dropdown trigger="click" @command="addFriendFromMenu">
          <el-button id="friend-add-button" type="primary" :icon="Plus">
            {{ t("friends.add") }}
            <ChevronDown class="dropdown-chevron" aria-hidden="true" />
          </el-button>
          <template #dropdown>
            <el-dropdown-menu>
              <el-dropdown-item
                v-for="option in providerOptions"
                :key="option.value"
                :command="option.value"
              >
                {{ option.label }}
              </el-dropdown-item>
            </el-dropdown-menu>
          </template>
        </el-dropdown>
      </div>
    </header>

    <section v-if="friends.length" class="friend-list">
      <article
        v-for="friend in friends"
        :key="friend.id"
        class="friend-row"
        :data-friend-id="friend.id"
        :aria-label="t('friends.friendSettings', { name: friend.name })"
      >
        <div class="friend-summary">
          <span class="friend-avatar-shell">
            <span
              class="friend-avatar"
              :style="{
                background: friend.color,
                color: getReadableTextColor(friend.color),
              }"
            >
              <img v-if="friend.avatar" :src="getAvatarSrc(friend.avatar)" alt="" />
              <span v-else>{{ getInitial(friend.name) }}</span>
            </span>
            <button
              class="avatar-edit-button"
              type="button"
              :title="t('members.changeAvatar')"
              :aria-label="t('members.changeAvatar')"
              @click="assignFriendAvatar(friend)"
            >
              <Pencil aria-hidden="true" />
            </button>
          </span>

          <div class="friend-name-block">
            <el-input
              v-model="friend.name"
              size="small"
              :placeholder="t('common.memberFallback')"
              :aria-label="t('members.memberName')"
              @blur="renameFriend(friend)"
              @keydown.enter.prevent="renameFriend(friend)"
            />
            <div class="friend-meta">
              <el-tag size="small">{{ getProviderLabel(friend.provider) }}</el-tag>
              <el-tag v-if="friend.isAdmin" size="small" type="success">
                {{ t("members.adminRole") }}
              </el-tag>
              <el-tag size="small" type="info">
                {{ t("friends.usedInGroups", { count: getFriendUsage(friend) }) }}
              </el-tag>
            </div>
          </div>
        </div>

        <div class="friend-fields">
          <label>
            <span>{{ t("common.api") }}</span>
            <el-select
              v-model="friend.provider"
              size="small"
              :aria-label="t('common.api')"
              @change="settingsStore.updateFriendProvider(friend)"
            >
              <el-option
                v-for="option in providerOptions"
                :key="option.value"
                :label="option.label"
                :value="option.value"
              />
            </el-select>
          </label>

          <label>
            <span>{{ t("common.model") }}</span>
            <el-select
              v-model="friend.model"
              size="small"
              filterable
              allow-create
              default-first-option
              :title="friend.model"
              :aria-label="`${t('common.model')}: ${friend.model}`"
              @change="settingsStore.updateFriendProfile(friend)"
            >
              <el-option
                v-for="preset in providers[friend.provider]?.models ?? []"
                :key="preset"
                :label="preset"
                :value="preset"
              />
            </el-select>
          </label>

          <label>
            <span>{{ t("members.reasoningEffort") }}</span>
            <el-select
              v-model="friend.reasoningEffort"
              size="small"
              :aria-label="t('members.reasoningEffort')"
              @change="settingsStore.updateFriendProfile(friend)"
            >
              <el-option
                v-for="option in reasoningEffortOptions"
                :key="option.value"
                :label="option.label"
                :value="option.value"
              />
            </el-select>
          </label>

          <label>
            <span>{{ t("common.temperature") }}</span>
            <el-input-number
              v-model="friend.temperature"
              size="small"
              :min="0"
              :max="2"
              :step="0.1"
              :controls="false"
              :aria-label="t('common.temperature')"
              @change="settingsStore.updateFriendProfile(friend)"
            />
          </label>
        </div>

        <div class="friend-switches">
          <span>
            {{ t("members.adminQuestion") }}
            <el-switch
              v-model="friend.isAdmin"
              size="small"
              :aria-label="t('members.adminQuestion')"
              @change="settingsStore.updateFriendProfile(friend)"
            />
          </span>
          <span>
            {{ t("members.writePermission") }}
            <el-switch
              v-model="friend.canWrite"
              size="small"
              :aria-label="t('members.writePermission')"
              @change="settingsStore.updateFriendProfile(friend)"
            />
          </span>
          <span>
            {{ t("members.muteQuestion") }}
            <el-switch
              v-model="friend.enabled"
              size="small"
              :active-value="false"
              :inactive-value="true"
              :aria-label="t('members.muteQuestion')"
              @change="settingsStore.updateFriendProfile(friend)"
            />
          </span>
          <span v-if="isDeepSeekProvider(providers[friend.provider])">
            {{ t("members.deepSeekLongContext") }}
            <el-switch
              v-model="friend.deepSeekLongContext"
              size="small"
              :aria-label="t('members.deepSeekLongContext')"
              @change="settingsStore.updateFriendProfile(friend)"
            />
          </span>
        </div>

        <label class="friend-role">
          <span>{{ t("members.roleIdentity") }}</span>
          <el-input
            v-model="friend.systemPrompt"
            type="textarea"
            :autosize="{ minRows: 2, maxRows: 5 }"
            resize="none"
            :placeholder="t('members.rolePlaceholder')"
            :aria-label="t('members.roleIdentity')"
            @change="settingsStore.updateFriendProfile(friend)"
          />
        </label>

        <div class="friend-actions">
          <el-button type="danger" plain :icon="Trash2" @click="removeFriend(friend)">
            {{ t("friends.remove") }}
          </el-button>
        </div>
      </article>
    </section>

    <section v-else class="friend-empty">
      <Users aria-hidden="true" />
      <strong>{{ t("friends.emptyTitle") }}</strong>
      <span>{{ t("friends.emptyDescription") }}</span>
      <el-button type="primary" :icon="Plus" @click="addFriend('openai')">
        {{ t("friends.addOpenAI") }}
      </el-button>
    </section>
    </div>
  </main>
</template>

<style scoped>
.friend-library-page {
  width: 100%;
  flex: 1;
  min-width: 0;
  min-height: 0;
  overflow: auto;
  padding: 16px;
}

.friend-library-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 16px;
  min-width: 0;
  margin-bottom: 14px;
  padding: 16px;
  border: 1px solid #d9ded8;
  border-radius: 8px;
  background: #fbfcfb;
  box-shadow: 0 14px 34px rgba(31, 43, 36, 0.08);
}

.friend-library-title,
.friend-library-actions,
.friend-summary,
.friend-meta,
.friend-switches,
.friend-switches span,
.friend-actions {
  display: flex;
  align-items: center;
}

.friend-library-title {
  min-width: 0;
  gap: 12px;
}

.friend-library-icon {
  display: grid;
  width: 44px;
  height: 44px;
  flex: 0 0 auto;
  place-items: center;
  border-radius: 8px;
  color: #ffffff;
  background: #2e6f5b;
  font-size: 20px;
}

.friend-library-title p {
  margin: 0 0 4px;
  color: #778279;
  font-size: 12px;
  font-weight: 700;
  letter-spacing: 0;
  text-transform: uppercase;
}

.friend-library-title h1 {
  margin: 0;
  color: #18221d;
  font-size: 22px;
  line-height: 1.2;
}

.friend-library-actions {
  flex-wrap: wrap;
  justify-content: flex-end;
  gap: 8px;
}

.friend-list {
  display: grid;
  gap: 10px;
}

.friend-row {
  display: grid;
  grid-template-columns: minmax(220px, 1.1fr) minmax(360px, 1.8fr) minmax(220px, 0.9fr) 120px;
  gap: 12px;
  align-items: start;
  min-width: 0;
  padding: 12px;
  border: 1px solid #e0e5df;
  border-radius: 8px;
  background: #ffffff;
}

.friend-summary {
  min-width: 0;
  gap: 10px;
}

.friend-avatar-shell {
  position: relative;
  flex: 0 0 auto;
  line-height: 0;
}

.friend-avatar {
  display: flex;
  width: 42px;
  height: 42px;
  align-items: center;
  justify-content: center;
  overflow: hidden;
  border-radius: 50%;
  color: #ffffff;
  font-size: 16px;
  font-weight: 800;
}

.friend-avatar img {
  width: 100%;
  height: 100%;
  object-fit: cover;
}

.avatar-edit-button {
  position: absolute;
  right: -4px;
  bottom: -4px;
  display: grid;
  width: 20px;
  height: 20px;
  place-items: center;
  border: 1px solid rgba(255, 255, 255, 0.92);
  border-radius: 50%;
  color: #ffffff;
  background: #2f7a61;
  box-shadow: 0 4px 10px rgba(31, 43, 36, 0.22);
  cursor: pointer;
  opacity: 0;
  transform: translateY(2px) scale(0.92);
  transition: opacity 0.16s, transform 0.16s, background 0.16s;
}

.friend-summary:hover .avatar-edit-button,
.avatar-edit-button:focus-visible {
  opacity: 1;
  transform: translateY(0) scale(1);
}

.avatar-edit-button .el-icon {
  font-size: 12px;
}

.avatar-edit-button:hover {
  background: #25664f;
}

.friend-name-block {
  display: grid;
  flex: 1;
  min-width: 0;
  gap: 6px;
}

.friend-meta {
  flex-wrap: wrap;
  gap: 5px;
}

.friend-fields {
  display: grid;
  grid-template-columns: repeat(4, minmax(0, 1fr));
  gap: 10px;
  min-width: 0;
}

.friend-fields label,
.friend-role {
  display: grid;
  min-width: 0;
  gap: 6px;
}

.friend-fields label > span,
.friend-role > span,
.friend-switches span {
  color: #778179;
  font-size: 12px;
  font-weight: 700;
}

.friend-switches {
  flex-wrap: wrap;
  gap: 10px 14px;
  padding-top: 22px;
}

.friend-switches span {
  gap: 7px;
}

.friend-role {
  grid-column: 1 / -2;
}

.friend-actions {
  justify-content: flex-end;
  padding-top: 22px;
}

.friend-empty {
  display: grid;
  min-height: 220px;
  place-items: center;
  gap: 8px;
  border: 1px dashed #cfd8d1;
  border-radius: 8px;
  color: #6d7871;
  background: #ffffff;
  text-align: center;
}

.friend-empty .el-icon {
  color: #2e6f5b;
  font-size: 32px;
}

.friend-empty strong {
  color: #26332d;
}

:deep(.el-select),
:deep(.el-input),
:deep(.el-input-number),
:deep(.el-textarea) {
  width: 100%;
  min-width: 0;
}

:deep(.el-input__inner) {
  min-width: 0;
  text-overflow: ellipsis;
}

:deep(.el-textarea__inner),
:deep(.el-input__wrapper) {
  border-radius: 8px;
}

@media (max-width: 1180px) {
  .friend-row {
    grid-template-columns: 1fr;
  }

  .friend-role {
    grid-column: auto;
  }

  .friend-actions,
  .friend-switches {
    padding-top: 0;
  }
}

@media (max-width: 760px) {
  .friend-library-head {
    align-items: stretch;
    flex-direction: column;
  }

  .friend-library-actions {
    justify-content: flex-start;
  }

  .friend-fields {
    grid-template-columns: 1fr;
  }
}
</style>

<style scoped>
.friend-library-page {
  padding: 0;
  background: var(--app-bg);
  scrollbar-gutter: stable;
}

.friend-library-content {
  width: min(1320px, 100%);
  min-width: 0;
  margin: 0 auto;
  padding: 26px 28px 40px;
}

.friend-library-head {
  margin: 0 0 20px;
  padding: 0 0 20px;
  border: 0;
  border-bottom: 1px solid var(--separator);
  border-radius: 0;
  background: transparent;
  box-shadow: none;
}

.friend-library-icon {
  width: 38px;
  height: 38px;
  border: 1px solid color-mix(in srgb, var(--accent) 28%, transparent);
  border-radius: 8px;
  color: var(--accent-text);
  background: var(--accent-soft);
}

.friend-library-icon svg {
  width: 20px;
  height: 20px;
  stroke-width: 1.8;
}

.friend-library-title p {
  color: var(--text-secondary);
  font-size: 12px;
  text-transform: none;
}

.friend-library-title h1 {
  color: var(--text-primary);
  font-size: 20px;
  font-weight: 700;
}

.dropdown-chevron {
  width: 14px;
  height: 14px;
  margin-left: 4px;
}

.friend-list {
  gap: 8px;
}

.friend-row {
  grid-template-columns: minmax(210px, 1fr) minmax(400px, 1.8fr) minmax(210px, 0.85fr) auto;
  gap: 12px;
  padding: 14px;
  border-color: var(--separator);
  background: var(--surface);
}

.friend-row:focus-within {
  border-color: color-mix(in srgb, var(--accent) 48%, var(--separator));
}

.friend-avatar {
  box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--separator-strong) 70%, transparent);
}

.avatar-edit-button {
  right: -6px;
  bottom: -5px;
  width: var(--control-height-small);
  height: var(--control-height-small);
  border-color: var(--surface);
  color: #ffffff;
  background: var(--accent);
  box-shadow: 0 2px 8px rgba(0, 0, 0, 0.18);
  opacity: 0.86;
  transform: none;
}

.avatar-edit-button svg {
  width: 13px;
  height: 13px;
}

.friend-summary:hover .avatar-edit-button,
.avatar-edit-button:focus-visible {
  opacity: 1;
  transform: none;
}

.avatar-edit-button:hover {
  background: var(--accent-hover);
}

.friend-fields label > span,
.friend-role > span,
.friend-switches span {
  color: var(--text-secondary);
}

.friend-switches {
  align-content: start;
  padding: 8px 10px;
  border-radius: 7px;
  background: var(--surface-secondary);
}

.friend-switches > span {
  width: 100%;
  min-height: 28px;
  justify-content: space-between;
}

.friend-role {
  grid-column: 1 / 4;
}

.friend-actions {
  grid-row: 1 / span 2;
  grid-column: 4;
  align-self: center;
  padding-top: 0;
}

.friend-empty {
  min-height: 360px;
  border: 0;
  border-radius: 0;
  color: var(--text-secondary);
  background: transparent;
}

.friend-empty > svg {
  width: 34px;
  height: 34px;
  color: var(--text-tertiary);
}

.friend-empty strong {
  color: var(--text-primary);
}

@media (max-width: 1180px) {
  .friend-row {
    grid-template-columns: 1fr;
  }

  .friend-role,
  .friend-actions {
    grid-row: auto;
    grid-column: auto;
  }

  .friend-switches {
    padding-top: 8px;
  }

  .friend-actions {
    justify-content: flex-start;
  }
}

@media (max-width: 760px) {
  .friend-library-content {
    padding: 20px 14px 32px;
  }

  .friend-fields {
    grid-template-columns: 1fr;
  }
}
</style>
