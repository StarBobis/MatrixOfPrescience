<script setup lang="ts">
import { computed } from "vue";
import { ElMessage } from "element-plus";
import { CirclePlus, Delete, EditPen, UserFilled } from "@element-plus/icons-vue";
import { storeToRefs } from "pinia";
import { useI18n } from "vue-i18n";
import {
  type AgentModel,
  type AgentReasoningEffort,
  type ProviderId,
  useSettingsStore,
} from "../stores/settings";
import { chooseLocalAvatar, getAvatarSrc } from "../utils/avatar";

const settingsStore = useSettingsStore();
const { friends, groups, providers } = storeToRefs(settingsStore);
const { t } = useI18n();

const providerOptions = computed<Array<{ label: string; value: ProviderId }>>(() =>
  (Object.values(providers.value) as Array<{ id: ProviderId; name: string }>).map((provider) => ({
    label: provider.name,
    value: provider.id,
  })),
);

const modelPresets: Record<ProviderId, string[]> = {
  openai: ["gpt-4.1", "gpt-4.1-mini", "gpt-4o", "gpt-4o-mini"],
  deepseek: ["deepseek-v4-flash", "deepseek-v4-pro", "deepseek-chat"],
};

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

function renameFriend(friend: AgentModel) {
  settingsStore.renameFriend(friend.id, friend.name);
}

function removeFriend(friendId: string) {
  if (!settingsStore.removeFriend(friendId)) {
    ElMessage.warning(t("messages.friendInUse"));
  }
}
</script>

<template>
  <main class="friend-library-page">
    <section class="friend-library-head">
      <div class="friend-library-title">
        <span class="friend-library-icon">
          <el-icon>
            <UserFilled />
          </el-icon>
        </span>
        <div>
          <p>{{ t("friends.eyebrow") }}</p>
          <h1>{{ t("friends.title") }}</h1>
        </div>
      </div>

      <div class="friend-library-actions">
        <el-tag size="large" type="success">{{ t("friends.count", { count: friends.length }) }}</el-tag>
        <el-button type="primary" :icon="CirclePlus" @click="addFriend('openai')">
          {{ t("friends.addOpenAI") }}
        </el-button>
        <el-button :icon="CirclePlus" @click="addFriend('deepseek')">
          {{ t("friends.addDeepSeek") }}
        </el-button>
      </div>
    </section>

    <section v-if="friends.length" class="friend-list">
      <article v-for="friend in friends" :key="friend.id" class="friend-row">
        <div class="friend-summary">
          <span class="friend-avatar-shell">
            <span class="friend-avatar" :style="{ background: friend.color }">
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
              <el-icon><EditPen /></el-icon>
            </button>
          </span>

          <div class="friend-name-block">
            <el-input
              v-model="friend.name"
              size="small"
              :placeholder="t('common.memberFallback')"
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
            <el-select v-model="friend.provider" size="small" @change="settingsStore.updateFriendProvider(friend)">
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
              @change="settingsStore.updateFriendProfile(friend)"
            >
              <el-option
                v-for="preset in modelPresets[friend.provider]"
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
              active-color="#2f7a61"
              @change="settingsStore.updateFriendProfile(friend)"
            />
          </span>
          <span>
            {{ t("members.writePermission") }}
            <el-switch
              v-model="friend.canWrite"
              size="small"
              active-color="#2f7a61"
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
              active-color="#c45656"
              @change="settingsStore.updateFriendProfile(friend)"
            />
          </span>
          <span v-if="friend.provider === 'deepseek'">
            {{ t("members.deepSeekLongContext") }}
            <el-switch
              v-model="friend.deepSeekLongContext"
              size="small"
              active-color="#2f7a61"
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
            @change="settingsStore.updateFriendProfile(friend)"
          />
        </label>

        <div class="friend-actions">
          <el-button type="danger" plain :icon="Delete" @click="removeFriend(friend.id)">
            {{ t("friends.remove") }}
          </el-button>
        </div>
      </article>
    </section>

    <section v-else class="friend-empty">
      <el-icon>
        <UserFilled />
      </el-icon>
      <strong>{{ t("friends.emptyTitle") }}</strong>
      <span>{{ t("friends.emptyDescription") }}</span>
    </section>
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
