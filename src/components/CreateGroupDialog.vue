<script setup lang="ts">
import { nextTick } from "vue";
import { Plus, Trash2 } from "@lucide/vue";
import { useI18n } from "vue-i18n";
import type { AgentModel, ChatGroupMode, ProviderId } from "../stores/settings";

const open = defineModel<boolean>("open", { required: true });

const props = defineProps<{
  name: string;
  description: string;
  announcement: string;
  mode: ChatGroupMode;
  members: AgentModel[];
  friends: AgentModel[];
  providerOptions: Array<{ label: string; value: ProviderId }>;
  modelPresets: Record<ProviderId, string[]>;
}>();

const emit = defineEmits<{
  "update:name": [value: string];
  "update:description": [value: string];
  "update:announcement": [value: string];
  "update:mode": [value: ChatGroupMode];
  addDraftMember: [provider: ProviderId];
  addDraftMemberFromFriend: [friendId: string];
  removeDraftMember: [memberId: string];
  updateDraftMemberProvider: [member: AgentModel];
  setDraftAdmin: [memberId: string];
  createGroup: [];
}>();

const { t } = useI18n();

function removeDraftMemberAndRestoreFocus(memberId: string) {
  const memberIndex = props.members.findIndex((member) => member.id === memberId);
  emit("removeDraftMember", memberId);
  void nextTick(() => {
    const cards = document.querySelectorAll<HTMLElement>(
      ".create-group-dialog .settings-card",
    );
    const nextCard = cards[Math.min(memberIndex, cards.length - 1)];

    (nextCard?.querySelector<HTMLElement>(".model-name-input input") ??
      document.getElementById("create-group-add-openai"))?.focus();
  });
}
</script>

<template>
  <el-dialog
    v-model="open"
    class="create-group-dialog"
    :title="t('createGroup.title')"
    width="720px"
  >
    <el-form label-position="top">
      <el-form-item :label="t('createGroup.name')">
        <el-input
          :model-value="name"
          :aria-label="t('createGroup.name')"
          @update:model-value="emit('update:name', String($event))"
        />
      </el-form-item>
      <el-form-item :label="t('createGroup.description')">
        <el-input
          :model-value="description"
          :aria-label="t('createGroup.description')"
          @update:model-value="emit('update:description', String($event))"
        />
      </el-form-item>
      <el-form-item :label="t('createGroup.mode')">
        <el-radio-group
          :model-value="mode"
          :aria-label="t('createGroup.mode')"
          @update:model-value="emit('update:mode', $event as ChatGroupMode)"
        >
          <el-radio-button value="discussion">{{ t("createGroup.modes.discussion") }}</el-radio-button>
          <el-radio-button value="task">{{ t("createGroup.modes.task") }}</el-radio-button>
        </el-radio-group>
      </el-form-item>
      <el-form-item :label="t('createGroup.announcement')">
        <el-input
          :model-value="announcement"
          type="textarea"
          :autosize="{ minRows: 3, maxRows: 6 }"
          resize="none"
          :aria-label="t('createGroup.announcement')"
          @update:model-value="emit('update:announcement', String($event))"
        />
      </el-form-item>
    </el-form>

    <div class="settings-toolbar">
      <el-select
        class="friend-member-select"
        :placeholder="t('members.addFromFriends')"
        :aria-label="t('members.addFromFriends')"
        filterable
        clearable
        @change="(friendId: string) => friendId && emit('addDraftMemberFromFriend', friendId)"
      >
        <el-option
          v-for="friend in friends"
          :key="friend.id"
          :label="friend.name"
          :value="friend.id"
          :disabled="
            members.some(
              (item) => item.libraryId === friend.libraryId || item.libraryId === friend.id,
            )
          "
        />
      </el-select>
      <el-button
        id="create-group-add-openai"
        type="primary"
        :icon="Plus"
        @click="emit('addDraftMember', 'openai')"
      >
        {{ t("createGroup.addOpenAIMember") }}
      </el-button>
      <el-button :icon="Plus" @click="emit('addDraftMember', 'deepseek')">
        {{ t("createGroup.addDeepSeekMember") }}
      </el-button>
    </div>

    <div class="settings-stack compact">
      <section v-for="member in members" :key="member.id" class="settings-card">
        <div class="settings-card-head">
          <el-input
            v-model="member.name"
            class="model-name-input"
            :aria-label="t('members.memberName')"
          />
          <div class="draft-member-toggles">
            <span class="draft-toggle">
              <span>{{ t("members.adminRole") }}</span>
              <el-radio
                v-if="mode === 'task'"
                :model-value="member.isAdmin ? member.id : ''"
                :label="member.id"
                :aria-label="t('members.adminRole')"
                @change="emit('setDraftAdmin', member.id)"
              >
                {{ t("createGroup.taskAdmin") }}
              </el-radio>
              <el-switch
                v-else
                v-model="member.isAdmin"
                :aria-label="t('members.adminRole')"
              />
            </span>
            <span class="draft-toggle">
              <span>{{ t("members.writePermission") }}</span>
              <el-switch
                v-model="member.canWrite"
                :aria-label="t('members.writePermission')"
              />
            </span>
            <span class="draft-toggle">
              <span>{{ t("members.active") }}</span>
              <el-switch v-model="member.enabled" :aria-label="t('members.active')" />
            </span>
          </div>
        </div>

        <div class="member-grid">
          <el-form-item :label="t('common.api')">
            <el-select
              v-model="member.provider"
              :aria-label="t('common.api')"
              @change="emit('updateDraftMemberProvider', member)"
            >
              <el-option
                v-for="option in providerOptions"
                :key="option.value"
                :label="option.label"
                :value="option.value"
              />
            </el-select>
          </el-form-item>
          <el-form-item :label="t('common.model')">
            <el-select
              v-model="member.model"
              filterable
              allow-create
              default-first-option
              :aria-label="`${t('common.model')}: ${member.model}`"
            >
              <el-option
                v-for="preset in modelPresets[member.provider]"
                :key="preset"
                :label="preset"
                :value="preset"
              />
            </el-select>
          </el-form-item>
        </div>

        <el-form-item :label="t('common.coreRole')">
          <el-input
            v-model="member.systemPrompt"
            type="textarea"
            :autosize="{ minRows: 2 }"
            resize="none"
            :aria-label="t('common.coreRole')"
          />
        </el-form-item>

        <div class="card-actions">
          <el-button
            type="danger"
            plain
            :icon="Trash2"
            @click="removeDraftMemberAndRestoreFocus(member.id)"
          >
            {{ t("common.deleteMember") }}
          </el-button>
        </div>
      </section>
    </div>

    <template #footer>
      <el-button @click="open = false">{{ t("common.cancel") }}</el-button>
      <el-button
        type="primary"
        :disabled="!name.trim() || members.length === 0"
        @click="emit('createGroup')"
      >
        {{ t("createGroup.create") }}
      </el-button>
    </template>
  </el-dialog>
</template>

<style scoped>
.settings-toolbar {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
  margin-bottom: 14px;
}

.friend-member-select {
  width: min(240px, 100%);
}

.draft-member-toggles,
.draft-toggle {
  display: inline-flex;
  flex: 0 0 auto;
  align-items: center;
  gap: 8px;
}

.draft-toggle > span {
  color: var(--text-secondary);
  font-size: 12px;
  font-weight: 600;
}

.settings-stack {
  display: grid;
  gap: 10px;
  padding-right: 2px;
}

.settings-card {
  padding: 14px;
  border: 1px solid var(--separator);
  border-radius: 8px;
  background: var(--surface-secondary);
}

.settings-card-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  margin-bottom: 12px;
}

.model-name-input {
  min-width: 160px;
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
}

:deep(.el-form-item) {
  min-width: 0;
}

:deep(.el-select),
:deep(.el-input) {
  width: 100%;
}

@media (max-width: 620px) {
  .settings-toolbar,
  .settings-card-head,
  .draft-member-toggles {
    align-items: stretch;
    flex-direction: column;
  }

  .friend-member-select,
  .member-grid {
    width: 100%;
    grid-template-columns: 1fr;
  }

  .draft-toggle {
    min-height: 32px;
    justify-content: space-between;
  }
}
</style>
