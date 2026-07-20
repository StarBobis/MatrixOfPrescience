<script setup lang="ts">
import { computed, nextTick, ref } from "vue";
import { Check, Pencil } from "@lucide/vue";
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
const announcementInput = ref<{ focus: () => void } | null>(null);
const announcementView = ref<HTMLButtonElement | null>(null);
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
  deepSeekProviderIds?: string[];
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

function startAnnouncementEdit() {
  editingAnnouncement.value = true;
  void nextTick(() => announcementInput.value?.focus());
}

function finishAnnouncementEdit() {
  editingAnnouncement.value = false;
  void nextTick(() => announcementView.value?.focus());
}
</script>

<template>
  <aside class="right-panel" :aria-label="t('rightPanel.inspectorLabel')">
    <section class="announcement-panel" aria-labelledby="announcement-heading">
      <div class="section-heading">
        <span id="announcement-heading">{{ t("rightPanel.announcement.title") }}</span>
        <div class="announcement-actions">
          <el-tag size="small" type="warning">{{ t("rightPanel.announcement.tag") }}</el-tag>
          <el-button
            circle
            plain
            :icon="editingAnnouncement ? Check : Pencil"
            :title="
              editingAnnouncement
                ? t('rightPanel.announcement.saveTitle')
                : t('rightPanel.announcement.editTitle')
            "
            :aria-label="
              editingAnnouncement
                ? t('rightPanel.announcement.saveTitle')
                : t('rightPanel.announcement.editTitle')
            "
            @click="editingAnnouncement ? finishAnnouncementEdit() : startAnnouncementEdit()"
          />
        </div>
      </div>
      <button
        v-if="!editingAnnouncement"
        ref="announcementView"
        class="announcement-view"
        type="button"
        :title="t('rightPanel.announcement.editTitle')"
        :aria-label="t('rightPanel.announcement.editTitle')"
        @click="startAnnouncementEdit"
      >
        {{ announcement || t("rightPanel.announcement.empty") }}
      </button>
      <el-input
        v-else
        ref="announcementInput"
        v-model="announcement"
        type="textarea"
        :autosize="{ minRows: 5, maxRows: 10 }"
        resize="none"
        :aria-label="t('rightPanel.announcement.title')"
        :placeholder="t('rightPanel.announcement.placeholder')"
        @keydown.ctrl.enter.prevent="finishAnnouncementEdit"
        @keydown.meta.enter.prevent="finishAnnouncementEdit"
        @keydown.esc.prevent="finishAnnouncementEdit"
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
      :deep-seek-provider-ids="deepSeekProviderIds"
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
  width: 100%;
  height: 100%;
  min-width: 0;
  min-height: 0;
  flex-direction: column;
  gap: 14px;
  overflow: hidden;
  padding: 16px 14px;
  background: var(--inspector-bg);
}

.announcement-panel {
  display: grid;
  flex: 0 0 auto;
  gap: 9px;
  padding-bottom: 14px;
  border-bottom: 1px solid var(--separator);
}

.section-heading,
.announcement-actions {
  display: flex;
  align-items: center;
}

.section-heading {
  min-height: 28px;
  justify-content: space-between;
  gap: 10px;
  color: var(--text-secondary);
  font-size: 12px;
  font-weight: 700;
}

.announcement-actions {
  flex: 0 0 auto;
  gap: 6px;
}

.announcement-actions :deep(.el-button) {
  width: var(--control-height-small);
  min-width: var(--control-height-small);
  height: var(--control-height-small);
}

.announcement-view {
  display: block;
  width: 100%;
  min-height: 96px;
  max-height: 160px;
  overflow: auto;
  padding: 10px 11px;
  border: 1px solid var(--separator-strong);
  border-radius: 7px;
  color: var(--text-primary);
  background: var(--surface);
  cursor: text;
  font-size: 13px;
  line-height: 1.55;
  text-align: left;
  white-space: pre-wrap;
}

.announcement-view:hover {
  border-color: var(--accent);
  background: color-mix(in srgb, var(--accent-soft) 36%, var(--surface));
}

.announcement-panel :deep(.el-textarea__inner) {
  line-height: 1.55;
}

@media (max-width: 980px) {
  .right-panel {
    height: auto;
    min-height: 320px;
    overflow: visible;
  }
}
</style>
