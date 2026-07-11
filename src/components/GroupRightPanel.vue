<script setup lang="ts">
import { computed, ref } from "vue";
import { useI18n } from "vue-i18n";
import GroupMemberPanel from "./GroupMemberPanel.vue";
import type {
  AgentModel,
  AgentReasoningEffort,
  OwnerProfile,
  ProviderId,
} from "../stores/settings";

const announcement = defineModel<string>("announcement", { default: "" });
const editingAnnouncement = ref(false);
const { t } = useI18n();

const reasoningEffortOptions = computed<Array<{ label: string; value: AgentReasoningEffort }>>(
  () => [
    { label: t("members.reasoningEffortOptions.off"), value: "off" },
    { label: t("members.reasoningEffortOptions.low"), value: "low" },
    { label: t("members.reasoningEffortOptions.medium"), value: "medium" },
    { label: t("members.reasoningEffortOptions.high"), value: "high" },
  ],
);

defineProps<{
  members: AgentModel[];
  friends: AgentModel[];
  ownerProfile: OwnerProfile;
  getProviderLabel: (provider: ProviderId) => string;
  providerOptions: Array<{ label: string; value: ProviderId }>;
  modelPresets: Record<ProviderId, string[]>;
}>();

const emit = defineEmits<{
  addMember: [provider: ProviderId];
  addFriendMember: [friendId: string];
  removeMember: [memberId: string];
  renameMember: [memberId: string, name: string];
  updateOwnerProfile: [profile: OwnerProfile];
  updateMemberProfile: [member: AgentModel];
  updateMemberProvider: [member: AgentModel];
}>();

function finishAnnouncementEdit() {
  editingAnnouncement.value = false;
}
</script>

<template>
  <aside class="right-panel">
    <section class="announcement-panel">
      <div class="section-heading">
        <span>{{ t("rightPanel.announcement.title") }}</span>
        <el-tag size="small" type="warning">{{ t("rightPanel.announcement.tag") }}</el-tag>
      </div>
      <div
        v-if="!editingAnnouncement"
        class="announcement-view"
        :title="t('rightPanel.announcement.editTitle')"
        @dblclick="editingAnnouncement = true"
      >
        {{ announcement || t("rightPanel.announcement.empty") }}
      </div>
      <el-input
        v-else
        v-model="announcement"
        type="textarea"
        :autosize="{ minRows: 7, maxRows: 12 }"
        resize="none"
        :placeholder="t('rightPanel.announcement.placeholder')"
        @blur="finishAnnouncementEdit"
        @keydown.ctrl.enter.prevent="finishAnnouncementEdit"
      />
    </section>

    <GroupMemberPanel
      :members="members"
      :friends="friends"
      :owner-profile="ownerProfile"
      :get-provider-label="getProviderLabel"
      :provider-options="providerOptions"
      :model-presets="modelPresets"
      :reasoning-effort-options="reasoningEffortOptions"
      @add-member="(provider) => emit('addMember', provider)"
      @add-friend-member="(friendId) => emit('addFriendMember', friendId)"
      @remove-member="(memberId) => emit('removeMember', memberId)"
      @rename-member="(memberId, name) => emit('renameMember', memberId, name)"
      @update-owner-profile="(profile) => emit('updateOwnerProfile', profile)"
      @update-member-profile="(member) => emit('updateMemberProfile', member)"
      @update-member-provider="(member) => emit('updateMemberProvider', member)"
    />
  </aside>
</template>

<style scoped>
.right-panel {
  display: flex;
  min-width: 0;
  height: 100%;
  min-height: 0;
  flex-direction: column;
  gap: 16px;
  overflow: hidden;
  padding: 18px;
  border: 1px solid #d9ded8;
  border-radius: 8px;
  background: #fbfcfb;
  box-shadow: 0 14px 34px rgba(31, 43, 36, 0.08);
}

.announcement-panel,
.agent-panel {
  display: grid;
  gap: 12px;
}

.section-heading {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 10px;
  color: #2f3833;
  font-size: 14px;
  font-weight: 700;
}

.member-heading-actions {
  display: flex;
  align-items: center;
  gap: 8px;
}

.member-add-btn {
  width: 28px;
  height: 28px;
  padding: 0;
}

.add-member-card {
  display: grid;
  gap: 10px;
}

.add-member-card strong {
  color: #202b25;
  font-size: 14px;
}

.add-member-card .el-button {
  width: 100%;
  margin-left: 0;
}

.announcement-panel :deep(.el-textarea__inner) {
  border-radius: 8px;
  box-shadow: none;
  line-height: 1.55;
}

.announcement-view {
  min-height: 132px;
  overflow: auto;
  padding: 10px 11px;
  border: 1px solid #dcdfe6;
  border-radius: 8px;
  color: #303a34;
  background: #ffffff;
  cursor: text;
  font-size: 13px;
  line-height: 1.55;
  white-space: pre-wrap;
}

.announcement-view:hover {
  border-color: #a8c7b8;
  background: #f8fbf9;
}

.agent-panel {
  padding-bottom: 2px;
}

.agent-settings-form {
  display: grid;
  gap: 10px;
}

.agent-settings-form :deep(.el-form-item) {
  margin-bottom: 0;
}

.agent-settings-grid {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: 10px;
}

.agent-mode-toggles {
  display: flex;
  flex-wrap: wrap;
  gap: 8px 12px;
}

.patch-empty {
  padding: 12px;
  border: 1px dashed #d8dfd7;
  border-radius: 8px;
  color: #7b857e;
  background: #ffffff;
  font-size: 13px;
  text-align: center;
}

.patch-list {
  display: grid;
  max-height: 340px;
  gap: 10px;
  overflow: auto;
}

.patch-card {
  display: grid;
  gap: 10px;
  padding: 12px;
  border: 1px solid #e0e5df;
  border-radius: 8px;
  background: #ffffff;
}

.patch-card.approved {
  border-color: #b8d3c5;
}

.patch-card.rejected {
  border-color: #efc4c4;
}

.patch-card-head {
  display: grid;
  gap: 8px;
}

.patch-card-head strong,
.patch-card-head span {
  display: block;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.patch-card-head strong {
  color: #202b25;
  font-size: 13px;
}

.patch-card-head span {
  margin-top: 3px;
  color: #7a837d;
  font-size: 12px;
}

.patch-tags,
.patch-actions {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
}

.patch-summary {
  display: -webkit-box;
  margin: 0;
  overflow: hidden;
  color: #37423b;
  font-size: 12px;
  line-height: 1.55;
  -webkit-box-orient: vertical;
  -webkit-line-clamp: 3;
}

.patch-safety {
  display: grid;
  gap: 6px;
  padding: 8px;
  border-radius: 8px;
  background: #f8faf8;
}

.patch-safety strong {
  color: #526057;
  font-size: 12px;
}

.patch-safety ul {
  display: grid;
  gap: 4px;
  margin: 0;
  padding-left: 16px;
  color: #435047;
  font-size: 12px;
  line-height: 1.45;
}

.patch-safety li.warning {
  color: #9a6a12;
}

.patch-files {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
}

.patch-files span,
.patch-files.muted {
  max-width: 100%;
  overflow: hidden;
  padding: 3px 7px;
  border-radius: 6px;
  color: #526057;
  background: #eef4ef;
  font-size: 12px;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.patch-files.muted {
  display: block;
  color: #879089;
}

.patch-preview {
  max-height: 160px;
  margin: 0;
  overflow: auto;
  padding: 10px;
  border-radius: 8px;
  color: #26312b;
  background: #f3f6f3;
  font-size: 11px;
  line-height: 1.5;
  white-space: pre-wrap;
}

.patch-actions {
  padding-top: 2px;
}

.member-panel :deep(.el-textarea__inner) {
  border-radius: 8px;
  box-shadow: none;
  line-height: 1.55;
}

@media (max-width: 980px) {
  .right-panel {
    min-height: auto;
  }
}
</style>
