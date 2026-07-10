<script setup lang="ts">
import { CirclePlus, Delete } from "@element-plus/icons-vue";
import { useI18n } from "vue-i18n";
import type { AgentModel, ChatGroupMode, ProviderId } from "../stores/settings";

const open = defineModel<boolean>("open", { required: true });

defineProps<{
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
</script>

<template>
  <el-dialog v-model="open" :title="t('createGroup.title')" width="720px">
    <el-form label-position="top">
      <el-form-item :label="t('createGroup.name')">
        <el-input :model-value="name" @update:model-value="emit('update:name', String($event))" />
      </el-form-item>
      <el-form-item :label="t('createGroup.description')">
        <el-input
          :model-value="description"
          @update:model-value="emit('update:description', String($event))"
        />
      </el-form-item>
      <el-form-item :label="t('createGroup.mode')">
        <el-radio-group
          :model-value="mode"
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
          @update:model-value="emit('update:announcement', String($event))"
        />
      </el-form-item>
    </el-form>

    <div class="settings-toolbar">
      <el-select
        class="friend-member-select"
        :placeholder="t('members.addFromFriends')"
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
      <el-button type="primary" :icon="CirclePlus" @click="emit('addDraftMember', 'openai')">
        {{ t("createGroup.addOpenAIMember") }}
      </el-button>
      <el-button :icon="CirclePlus" @click="emit('addDraftMember', 'deepseek')">
        {{ t("createGroup.addDeepSeekMember") }}
      </el-button>
    </div>

    <div class="settings-stack compact">
      <section v-for="member in members" :key="member.id" class="settings-card">
        <div class="settings-card-head">
          <el-input v-model="member.name" class="model-name-input" />
          <div class="draft-member-toggles">
            <span class="draft-toggle">
              <span>{{ t("members.adminRole") }}</span>
              <el-radio
                v-if="mode === 'task'"
                :model-value="member.isAdmin ? member.id : ''"
                :label="member.id"
                @change="emit('setDraftAdmin', member.id)"
              >
                {{ t("createGroup.taskAdmin") }}
              </el-radio>
              <el-switch v-else v-model="member.isAdmin" active-color="#2f7a61" />
            </span>
            <span class="draft-toggle">
              <span>{{ t("members.writePermission") }}</span>
              <el-switch v-model="member.canWrite" active-color="#2f7a61" />
            </span>
            <el-switch v-model="member.enabled" />
          </div>
        </div>

        <div class="member-grid">
          <el-form-item :label="t('common.api')">
            <el-select v-model="member.provider" @change="emit('updateDraftMemberProvider', member)">
              <el-option
                v-for="option in providerOptions"
                :key="option.value"
                :label="option.label"
                :value="option.value"
              />
            </el-select>
          </el-form-item>
          <el-form-item :label="t('common.model')">
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

        <el-form-item :label="t('common.coreRole')">
          <el-input
            v-model="member.systemPrompt"
            type="textarea"
            :autosize="{ minRows: 2 }"
            resize="none"
          />
        </el-form-item>

        <div class="card-actions">
          <el-button type="danger" plain :icon="Delete" @click="emit('removeDraftMember', member.id)">
            {{ t("common.deleteMember") }}
          </el-button>
        </div>
      </section>
    </div>

    <template #footer>
      <el-button @click="open = false">{{ t("common.cancel") }}</el-button>
      <el-button type="primary" @click="emit('createGroup')">{{ t("createGroup.create") }}</el-button>
    </template>
  </el-dialog>
</template>

<style scoped>
.settings-toolbar {
  display: flex;
  flex-wrap: wrap;
  gap: 10px;
  margin-bottom: 14px;
}

.friend-member-select {
  width: 220px;
}

.draft-member-toggles,
.draft-toggle {
  display: inline-flex;
  flex: 0 0 auto;
  align-items: center;
  gap: 8px;
}

.draft-toggle > span {
  color: #2f7a61;
  font-size: 12px;
  font-weight: 800;
}
</style>
