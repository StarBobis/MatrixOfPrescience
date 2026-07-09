<script setup lang="ts">
import { ref } from "vue";
import { CirclePlus, Close } from "@element-plus/icons-vue";
import { useI18n } from "vue-i18n";
import { chooseLocalAvatar, getAvatarSrc } from "../utils/avatar";
import type { AgentModel, OwnerProfile, ProviderId } from "../stores/settings";

defineProps<{
  members: AgentModel[];
  historicalMembers: AgentModel[];
  ownerProfile: OwnerProfile;
  getProviderLabel: (provider: ProviderId) => string;
}>();

const emit = defineEmits<{
  addMember: [provider: ProviderId];
  addHistoricalMember: [memberId: string];
  removeMember: [memberId: string];
  renameMember: [memberId: string, name: string];
  updateMemberProfile: [member: AgentModel];
}>();

const activeMemberCardId = ref("");
const editingMemberCardId = ref("");
const { t } = useI18n();
let memberCardCloseTimer: number | undefined;

function getInitial(name: string) {
  return name.trim().slice(0, 1) || "?";
}

function showMemberCard(memberId: string) {
  if (memberCardCloseTimer) {
    window.clearTimeout(memberCardCloseTimer);
  }

  activeMemberCardId.value = memberId;
}

function scheduleHideMemberCard(memberId: string) {
  if (editingMemberCardId.value === memberId) {
    return;
  }

  if (memberCardCloseTimer) {
    window.clearTimeout(memberCardCloseTimer);
  }

  memberCardCloseTimer = window.setTimeout(() => {
    if (activeMemberCardId.value === memberId && editingMemberCardId.value !== memberId) {
      activeMemberCardId.value = "";
    }
  }, 160);
}

function startMemberCardEdit(memberId: string) {
  editingMemberCardId.value = memberId;
  showMemberCard(memberId);
}

function finishMemberCardEdit(memberId: string) {
  if (editingMemberCardId.value === memberId) {
    editingMemberCardId.value = "";
  }
}

async function assignLocalAvatar(member: AgentModel) {
  const avatar = await chooseLocalAvatar();

  if (avatar) {
    member.avatar = avatar;
    emit("updateMemberProfile", member);
  }
}
</script>

<template>
  <section class="member-panel">
    <div class="section-heading">
      <span>{{ t("members.listTitle") }}</span>
      <div class="member-heading-actions">
        <el-tag size="small" type="success">{{ members.length + 1 }}</el-tag>
        <el-popover trigger="click" placement="left-start" :width="220">
          <template #reference>
            <el-button class="member-add-btn" circle type="primary" :icon="CirclePlus" />
          </template>

          <div class="add-member-card">
            <strong>{{ t("members.addTitle") }}</strong>
            <el-select
              :placeholder="t('members.addFromHistory')"
              filterable
              clearable
              @change="(memberId: string) => memberId && emit('addHistoricalMember', memberId)"
            >
              <el-option
                v-for="member in historicalMembers"
                :key="member.id"
                :label="member.name"
                :value="member.id"
                :disabled="
                  members.some(
                    (item) =>
                      item.name.trim().toLocaleLowerCase() === member.name.trim().toLocaleLowerCase(),
                  )
                "
              />
            </el-select>
            <el-button type="primary" plain @click="emit('addMember', 'openai')">
              {{ t("members.addOpenAIMember") }}
            </el-button>
            <el-button plain @click="emit('addMember', 'deepseek')">
              {{ t("members.addDeepSeekMember") }}
            </el-button>
          </div>
        </el-popover>
      </div>
    </div>

    <div class="right-member-list">
      <article class="owner-card">
        <span class="member-avatar owner-avatar" :style="{ background: ownerProfile.color }">
          <img v-if="ownerProfile.avatar" :src="getAvatarSrc(ownerProfile.avatar)" alt="" />
          <span v-else>{{ getInitial(ownerProfile.name) }}</span>
        </span>
        <div class="member-card-copy">
          <strong>{{ ownerProfile.name || t("common.ownerName") }}</strong>
          <span>{{ t("common.ownerRole") }}</span>
        </div>
      </article>

      <el-popover
        v-for="member in members"
        :key="member.id"
        :visible="activeMemberCardId === member.id"
        trigger="manual"
        placement="left-start"
        :width="292"
        popper-class="member-popover"
      >
        <template #reference>
          <div
            class="right-member-card"
            :class="{ muted: !member.enabled }"
            @mouseenter="showMemberCard(member.id)"
            @mouseleave="scheduleHideMemberCard(member.id)"
          >
            <span class="member-avatar" :style="{ background: member.color }">
              <img v-if="member.avatar" :src="getAvatarSrc(member.avatar)" alt="" />
              <span v-else>{{ getInitial(member.name) }}</span>
            </span>
            <div class="member-card-copy">
              <strong>{{ member.name }}</strong>
              <span v-if="!member.enabled">{{ t("members.muted") }}</span>
            </div>
            <button
              class="member-remove-btn"
              type="button"
              :title="t('members.removeTitle')"
              @click.stop="emit('removeMember', member.id)"
            >
              <el-icon><Close /></el-icon>
            </button>
          </div>
        </template>

        <div
          class="member-profile-card"
          @mouseenter="showMemberCard(member.id)"
          @mouseleave="scheduleHideMemberCard(member.id)"
        >
          <div class="profile-head">
            <span class="profile-avatar" :style="{ background: member.color }">
              <img v-if="member.avatar" :src="getAvatarSrc(member.avatar)" alt="" />
              <span v-else>{{ getInitial(member.name) }}</span>
            </span>
            <div class="profile-title">
            <el-input
              v-model="member.name"
              size="small"
              @focus="startMemberCardEdit(member.id)"
              @input="emit('renameMember', member.id, member.name)"
              @blur="finishMemberCardEdit(member.id)"
            />
              <span v-if="!member.enabled">{{ t("members.muted") }}</span>
            </div>
          </div>

          <dl class="profile-details">
            <div>
              <dt>API</dt>
              <dd>{{ getProviderLabel(member.provider) }}</dd>
            </div>
            <div>
              <dt>{{ t("common.model") }}</dt>
              <dd>{{ member.model }}</dd>
            </div>
            <div>
              <dt>{{ t("common.temperature") }}</dt>
              <dd>{{ member.temperature.toFixed(1) }}</dd>
            </div>
          </dl>

          <div class="profile-muted-row">
            <span>{{ t("members.muteQuestion") }}</span>
            <el-switch
              v-model="member.enabled"
              size="small"
              inline-prompt
              :active-text="t('common.yes')"
              :inactive-text="t('common.no')"
              active-color="#c45656"
              :active-value="false"
              :inactive-value="true"
              @change="emit('updateMemberProfile', member)"
            />
          </div>

          <div class="profile-prompt">
            <span>{{ t("members.roleIdentity") }}</span>
            <el-input
              v-model="member.systemPrompt"
              type="textarea"
              :autosize="{ minRows: 3 }"
              resize="none"
              :placeholder="t('members.rolePlaceholder')"
              @focus="startMemberCardEdit(member.id)"
              @input="emit('updateMemberProfile', member)"
              @blur="finishMemberCardEdit(member.id)"
            />
          </div>

          <div class="profile-actions">
            <el-button size="small" @click="assignLocalAvatar(member)">
              {{ t("members.localAvatar") }}
            </el-button>
            <el-button size="small" type="danger" plain @click="emit('removeMember', member.id)">
              {{ t("members.removeTitle") }}
            </el-button>
          </div>
        </div>
      </el-popover>
    </div>
  </section>
</template>

<style scoped>
.member-panel {
  display: grid;
  flex: 1;
  min-height: 160px;
  gap: 12px;
  grid-template-rows: auto minmax(0, 1fr);
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

.right-member-list {
  display: grid;
  min-height: 0;
  gap: 10px;
  overflow-y: auto;
  scrollbar-gutter: stable;
}

.right-member-list :deep(.el-tooltip__trigger) {
  width: 100%;
}

.owner-card,
.right-member-card {
  display: flex;
  width: 100%;
  min-height: 60px;
  align-items: center;
  gap: 10px;
  padding: 9px 10px;
  border: 1px solid #e0e5df;
  border-radius: 8px;
  color: inherit;
  background: #ffffff;
  text-align: left;
}

.owner-card {
  border-color: #b8d3c5;
  background: #f3faf6;
}

.right-member-card {
  cursor: pointer;
  position: relative;
}

.right-member-card:hover {
  border-color: #a8c7b8;
  background: #f5faf7;
}

.right-member-card.muted {
  opacity: 0.62;
}

.member-remove-btn {
  display: grid;
  flex: 0 0 auto;
  width: 24px;
  height: 24px;
  place-items: center;
  border: none;
  border-radius: 6px;
  color: #a0a8a2;
  background: transparent;
  cursor: pointer;
  opacity: 0;
  transition: opacity 0.15s, color 0.15s, background 0.15s;
}

.right-member-card:hover .member-remove-btn {
  opacity: 1;
}

.member-remove-btn:hover {
  color: #c45656;
  background: #fef0f0;
}

.member-avatar,
.profile-avatar {
  display: grid;
  flex: 0 0 auto;
  place-items: center;
  overflow: hidden;
  border-radius: 50%;
  color: #ffffff;
  font-weight: 800;
}

.member-avatar img,
.profile-avatar img {
  width: 100%;
  height: 100%;
  object-fit: cover;
}

.member-avatar {
  width: 38px;
  height: 38px;
  font-size: 16px;
}

.owner-avatar {
  box-shadow: 0 0 0 2px #ffffff, 0 0 0 3px #8fbda8;
}

.member-card-copy {
  min-width: 0;
  flex: 1;
}

.member-card-copy strong,
.member-card-copy span {
  display: block;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.member-card-copy strong {
  color: #202b25;
  font-size: 14px;
}

.member-card-copy span {
  margin-top: 4px;
  color: #727c74;
  font-size: 12px;
}

:global(.member-popover.el-popover) {
  border-radius: 8px;
  padding: 0;
}

.member-profile-card {
  display: grid;
  gap: 12px;
  padding: 14px;
}

.profile-head {
  display: flex;
  align-items: center;
  gap: 10px;
}

.profile-avatar {
  width: 42px;
  height: 42px;
  font-size: 18px;
}

.profile-title {
  min-width: 0;
}

.profile-title span {
  display: block;
  overflow: hidden;
  margin-top: 3px;
  color: #6d7871;
  font-size: 12px;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.profile-details {
  display: grid;
  gap: 8px;
  margin: 0;
}

.profile-details div,
.profile-muted-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
}

.profile-details dt,
.profile-muted-row span,
.profile-prompt span {
  color: #778179;
  font-size: 12px;
}

.profile-details dd {
  min-width: 0;
  margin: 0;
  overflow: hidden;
  color: #27332c;
  font-size: 12px;
  font-weight: 700;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.profile-prompt {
  display: grid;
  gap: 6px;
}

.profile-prompt :deep(.el-textarea__inner),
.profile-title :deep(.el-input__wrapper) {
  border-radius: 8px;
  box-shadow: none;
}

.profile-actions {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
}
</style>
