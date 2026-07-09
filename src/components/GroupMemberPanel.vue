<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { CirclePlus, EditPen } from "@element-plus/icons-vue";
import { useI18n } from "vue-i18n";
import { chooseLocalAvatar, getAvatarSrc } from "../utils/avatar";
import type { AgentModel, AgentReasoningEffort, OwnerProfile, ProviderId } from "../stores/settings";

const props = defineProps<{
  members: AgentModel[];
  friends: AgentModel[];
  ownerProfile: OwnerProfile;
  getProviderLabel: (provider: ProviderId) => string;
  providerOptions: Array<{ label: string; value: ProviderId }>;
  modelPresets: Record<ProviderId, string[]>;
  reasoningEffortOptions: Array<{ label: string; value: AgentReasoningEffort }>;
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

const activeMemberCardId = ref("");
const editingMemberCardId = ref("");
const memberNameDrafts = ref<Record<string, string>>({});
const { t } = useI18n();
let memberCardCloseTimer: number | undefined;
let memberCardWatchTimer: number | undefined;
let lastPointerPosition: { x: number; y: number } | null = null;
const memberCardHideDelayMs = 140;
const memberCardWatchIntervalMs = 360;

const sortedMembers = computed(() =>
  props.members
    .map((member, index) => ({ member, index }))
    .sort((left, right) => {
      const adminPriority = Number(right.member.isAdmin) - Number(left.member.isAdmin);
      return adminPriority || left.index - right.index;
    })
    .map(({ member }) => member),
);

function getInitial(name: string) {
  return name.trim().slice(0, 1) || "?";
}

function showMemberCard(memberId: string) {
  if (memberCardCloseTimer) {
    window.clearTimeout(memberCardCloseTimer);
    memberCardCloseTimer = undefined;
  }

  activeMemberCardId.value = memberId;
  startMemberCardWatchdog();
}

function hideMemberCard(memberId = activeMemberCardId.value, force = false) {
  if (!memberId || (!force && editingMemberCardId.value === memberId)) {
    return;
  }

  if (memberCardCloseTimer) {
    window.clearTimeout(memberCardCloseTimer);
    memberCardCloseTimer = undefined;
  }

  if (activeMemberCardId.value === memberId) {
    activeMemberCardId.value = "";
  }

  if (editingMemberCardId.value === memberId) {
    editingMemberCardId.value = "";
  }

  hideActiveMemberCardIfPointerAway();
}

function isInsideActiveMemberCardTarget(target: EventTarget | null) {
  const activeId = activeMemberCardId.value;

  if (!activeId || !(target instanceof Element)) {
    return false;
  }

  const card = target.closest<HTMLElement>("[data-member-card-id]");
  const popover = target.closest<HTMLElement>("[data-member-popover-id]");

  return card?.dataset.memberCardId === activeId || popover?.dataset.memberPopoverId === activeId;
}

function isPointerInsideActiveMemberCard() {
  if (!lastPointerPosition) {
    return false;
  }

  return isInsideActiveMemberCardTarget(
    document.elementFromPoint(lastPointerPosition.x, lastPointerPosition.y),
  );
}

function hideActiveMemberCardIfPointerAway() {
  const activeId = activeMemberCardId.value;

  if (!activeId || editingMemberCardId.value === activeId || isPointerInsideActiveMemberCard()) {
    return;
  }

  scheduleHideMemberCard(activeId);
}

function startMemberCardWatchdog() {
  if (memberCardWatchTimer) {
    return;
  }

  memberCardWatchTimer = window.setInterval(
    hideActiveMemberCardIfPointerAway,
    memberCardWatchIntervalMs,
  );
}

function stopMemberCardWatchdog() {
  if (memberCardWatchTimer) {
    window.clearInterval(memberCardWatchTimer);
    memberCardWatchTimer = undefined;
  }
}

function scheduleHideMemberCard(memberId: string) {
  if (editingMemberCardId.value === memberId) {
    return;
  }

  if (memberCardCloseTimer) {
    window.clearTimeout(memberCardCloseTimer);
    memberCardCloseTimer = undefined;
  }

  memberCardCloseTimer = window.setTimeout(() => {
    if (activeMemberCardId.value === memberId && editingMemberCardId.value !== memberId) {
      hideMemberCard(memberId);
    }
  }, memberCardHideDelayMs);
}

function startMemberCardEdit(memberId: string) {
  editingMemberCardId.value = memberId;
  const member = props.members.find((item) => item.id === memberId);

  if (member && !(memberId in memberNameDrafts.value)) {
    memberNameDrafts.value[memberId] = member.name;
  }

  showMemberCard(memberId);
}

function finishMemberCardEdit(memberId: string) {
  const member = props.members.find((item) => item.id === memberId);
  const draft = memberNameDrafts.value[memberId];

  if (member && draft !== undefined) {
    const nextName = draft.trim();

    if (nextName) {
      emit("renameMember", member.id, nextName);
      memberNameDrafts.value[memberId] = member.name;
    } else {
      memberNameDrafts.value[memberId] = member.name;
    }
  }

  if (editingMemberCardId.value === memberId) {
    editingMemberCardId.value = "";
  }
}

function getMemberNameDraft(member: AgentModel) {
  return memberNameDrafts.value[member.id] ?? member.name;
}

function updateMemberNameDraft(memberId: string, value: string | number) {
  memberNameDrafts.value[memberId] = String(value);
}

function updateMemberSetting(member: AgentModel) {
  emit("updateMemberProfile", member);
}

async function assignLocalAvatar(member: AgentModel) {
  const avatar = await chooseLocalAvatar();

  if (avatar) {
    member.avatar = avatar;
    emit("updateMemberProfile", member);
  }
}

async function assignOwnerAvatar() {
  const avatar = await chooseLocalAvatar();

  if (avatar) {
    emit("updateOwnerProfile", {
      ...props.ownerProfile,
      avatar,
    });
  }
}

function handleGlobalPointerMove(event: PointerEvent) {
  lastPointerPosition = {
    x: event.clientX,
    y: event.clientY,
  };

  if (activeMemberCardId.value && !isInsideActiveMemberCardTarget(event.target)) {
    hideActiveMemberCardIfPointerAway();
  }
}

function handleGlobalPointerLeave() {
  hideMemberCard(activeMemberCardId.value, true);
}

function handleGlobalVisibilityChange() {
  if (document.hidden) {
    hideMemberCard(activeMemberCardId.value, true);
  }
}

watch(
  () => props.members.map((member) => member.id).join("|"),
  () => {
    if (activeMemberCardId.value && !props.members.some((member) => member.id === activeMemberCardId.value)) {
      hideMemberCard(activeMemberCardId.value, true);
    }
  },
);

watch(activeMemberCardId, (memberId) => {
  if (memberId) {
    startMemberCardWatchdog();
    return;
  }

  stopMemberCardWatchdog();
});

onMounted(() => {
  window.addEventListener("pointermove", handleGlobalPointerMove, true);
  window.addEventListener("pointerleave", handleGlobalPointerLeave);
  window.addEventListener("blur", handleGlobalPointerLeave);
  window.addEventListener("scroll", hideActiveMemberCardIfPointerAway, true);
  document.addEventListener("visibilitychange", handleGlobalVisibilityChange);
});

onBeforeUnmount(() => {
  if (memberCardCloseTimer) {
    window.clearTimeout(memberCardCloseTimer);
    memberCardCloseTimer = undefined;
  }

  stopMemberCardWatchdog();
  window.removeEventListener("pointermove", handleGlobalPointerMove, true);
  window.removeEventListener("pointerleave", handleGlobalPointerLeave);
  window.removeEventListener("blur", handleGlobalPointerLeave);
  window.removeEventListener("scroll", hideActiveMemberCardIfPointerAway, true);
  document.removeEventListener("visibilitychange", handleGlobalVisibilityChange);
});
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
              :placeholder="t('members.addFromFriends')"
              filterable
              clearable
              @change="(friendId: string) => friendId && emit('addFriendMember', friendId)"
            >
              <el-option
                v-for="friend in friends"
                :key="friend.id"
                :label="friend.name"
                :value="friend.id"
                :disabled="
                  members.some(
                    (item) =>
                      item.libraryId === friend.libraryId || item.libraryId === friend.id,
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
        <span class="member-avatar-shell">
          <span class="member-avatar owner-avatar" :style="{ background: ownerProfile.color }">
            <img v-if="ownerProfile.avatar" :src="getAvatarSrc(ownerProfile.avatar)" alt="" />
            <span v-else>{{ getInitial(ownerProfile.name) }}</span>
          </span>
          <button
            class="avatar-edit-button"
            type="button"
            :title="t('members.changeAvatar')"
            :aria-label="t('members.changeAvatar')"
            @click.stop="assignOwnerAvatar"
          >
            <el-icon><EditPen /></el-icon>
          </button>
        </span>
        <div class="member-card-copy">
          <div class="member-name-row">
            <strong>{{ ownerProfile.name || t("common.ownerName") }}</strong>
            <span class="identity-badge owner">{{ t("common.ownerRole") }}</span>
          </div>
          <span class="member-sub">· {{ t("common.ownerRole") }}</span>
        </div>
      </article>

      <el-popover
        v-for="member in sortedMembers"
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
            :class="{ admin: member.isAdmin, muted: !member.enabled }"
            :style="{ '--member-accent': member.color }"
            :data-member-card-id="member.id"
            @mouseenter="showMemberCard(member.id)"
            @mouseleave="scheduleHideMemberCard(member.id)"
          >
            <span class="member-avatar-shell">
              <span class="member-avatar" :style="{ background: member.color }">
                <img v-if="member.avatar" :src="getAvatarSrc(member.avatar)" alt="" />
                <span v-else>{{ getInitial(member.name) }}</span>
              </span>
              <button
                class="avatar-edit-button"
                type="button"
                :title="t('members.changeAvatar')"
                :aria-label="t('members.changeAvatar')"
                @click.stop="assignLocalAvatar(member)"
              >
                <el-icon><EditPen /></el-icon>
              </button>
            </span>
            <div class="member-card-copy">
              <div class="member-name-row">
                <strong>{{ member.name }}</strong>
                <span v-if="member.isAdmin" class="identity-badge admin">
                  {{ t("members.adminRole") }}
                </span>
              </div>
              <span v-if="!member.enabled" class="member-sub">{{ t("members.muted") }}</span>
              <span v-else class="member-sub">· {{ member.model }}</span>
            </div>
          </div>
        </template>

        <div
          class="member-profile-card"
          :data-member-popover-id="member.id"
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
              :model-value="getMemberNameDraft(member)"
              size="small"
              @focus="startMemberCardEdit(member.id)"
              @update:model-value="updateMemberNameDraft(member.id, $event)"
              @blur="finishMemberCardEdit(member.id)"
              @keydown.enter.prevent="finishMemberCardEdit(member.id)"
            />
              <div class="profile-badges">
                <span v-if="member.isAdmin" class="identity-badge admin">
                  {{ t("members.adminRole") }}
                </span>
                <span v-if="!member.enabled">{{ t("members.muted") }}</span>
              </div>
            </div>
          </div>

          <dl class="profile-details">
            <div>
              <dt>API</dt>
              <dd>{{ getProviderLabel(member.provider) }}</dd>
            </div>
          </dl>

          <div class="profile-settings">
            <label>
              <span>{{ t("common.api") }}</span>
              <el-select
                v-model="member.provider"
                size="small"
                @focus="startMemberCardEdit(member.id)"
                @change="emit('updateMemberProvider', member)"
                @blur="finishMemberCardEdit(member.id)"
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
                v-model="member.model"
                size="small"
                filterable
                allow-create
                default-first-option
                @focus="startMemberCardEdit(member.id)"
                @change="updateMemberSetting(member)"
                @blur="finishMemberCardEdit(member.id)"
              >
                <el-option
                  v-for="preset in modelPresets[member.provider]"
                  :key="preset"
                  :label="preset"
                  :value="preset"
                />
              </el-select>
            </label>

            <label>
              <span>{{ t("members.reasoningEffort") }}</span>
              <el-select
                v-model="member.reasoningEffort"
                size="small"
                @focus="startMemberCardEdit(member.id)"
                @change="updateMemberSetting(member)"
                @blur="finishMemberCardEdit(member.id)"
              >
                <el-option
                  v-for="option in reasoningEffortOptions"
                  :key="option.value"
                  :label="option.label"
                  :value="option.value"
                />
              </el-select>
            </label>

            <div v-if="member.provider === 'deepseek'" class="profile-switch-row">
              <span>{{ t("members.deepSeekLongContext") }}</span>
              <el-switch
                v-model="member.deepSeekLongContext"
                size="small"
                inline-prompt
                :active-text="t('common.yes')"
                :inactive-text="t('common.no')"
                :active-value="true"
                :inactive-value="false"
                active-color="#2f7a61"
                @change="emit('updateMemberProfile', member)"
              />
            </div>

            <label>
              <span>{{ t("common.temperature") }}</span>
              <div class="temperature-control">
                <el-slider
                  v-model="member.temperature"
                  :min="0"
                  :max="2"
                  :step="0.1"
                  @change="updateMemberSetting(member)"
                />
                <el-input-number
                  v-model="member.temperature"
                  size="small"
                  :min="0"
                  :max="2"
                  :step="0.1"
                  :controls="false"
                  @focus="startMemberCardEdit(member.id)"
                  @change="updateMemberSetting(member)"
                  @blur="finishMemberCardEdit(member.id)"
                />
              </div>
            </label>
          </div>

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

          <div class="profile-admin-row">
            <span>{{ t("members.adminQuestion") }}</span>
            <el-switch
              v-model="member.isAdmin"
              size="small"
              inline-prompt
              :active-text="t('common.yes')"
              :inactive-text="t('common.no')"
              active-color="#2f7a61"
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
/* ===== 面板容器 ===== */
.member-panel {
  display: flex;
  flex-direction: column;
  flex: 1;
  min-height: 160px;
  gap: 12px;
}

/* ===== 标题栏 ===== */
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
  width: 30px;
  height: 30px;
  padding: 0;
  box-shadow: 0 2px 8px rgba(46, 111, 91, 0.22);
}

.add-member-card {
  display: flex;
  flex-direction: column;
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

/* ===== 成员列表 ===== */
.right-member-list {
  display: flex;
  flex-direction: column;
  flex: 1;
  min-height: 0;
  gap: 6px;
  overflow-y: auto;
  scrollbar-gutter: stable;
}

.right-member-list :deep(.el-tooltip__trigger) {
  display: block;
  width: 100%;
}

/* ===== 卡片通用 ===== */
.owner-card,
.right-member-card {
  display: flex !important;
  flex-direction: row !important;
  flex-wrap: nowrap !important;
  align-items: center;
  gap: 10px;
  width: 100%;
  box-sizing: border-box;
  padding: 8px 10px;
  border-radius: 8px;
  border: 1px solid #e8ede9;
  background: #ffffff;
  transition: border-color 0.18s, background 0.18s, box-shadow 0.18s;
}

/* ===== 群主卡片 ===== */
.owner-card {
  border-color: #d8b75f;
  background: linear-gradient(135deg, #fffaf0, #fff2c8);
}

/* ===== 成员卡片 ===== */
.right-member-card {
  cursor: pointer;
}

.right-member-card:hover {
  border-color: #b8cdc1;
  background: #f8fbf9;
  box-shadow: 0 2px 8px rgba(31, 43, 36, 0.06);
}

.right-member-card.admin {
  border-color: #8fc6a9;
  background: #f4fbf7;
}

.right-member-card.admin:hover {
  border-color: #5fa382;
  background: #eef8f2;
}

.right-member-card.muted {
  border-color: #e8ede9;
  background: #fafbfa;
}

.right-member-card.muted:hover {
  box-shadow: none;
}

.right-member-card.muted .member-avatar {
  filter: grayscale(0.5);
  opacity: 0.55;
}

.right-member-card.muted .member-card-copy strong {
  color: #a0a9a3;
}

.right-member-card.muted .member-card-copy .member-sub {
  color: #c0c7c2;
}

/* ===== 头像 ===== */
.member-avatar,
.profile-avatar {
  flex-shrink: 0;
  display: flex;
  align-items: center;
  justify-content: center;
  overflow: hidden;
  border-radius: 50%;
  color: #fff;
  font-weight: 800;
}

.member-avatar-shell {
  position: relative;
  flex: 0 0 auto;
  line-height: 0;
}

.member-avatar img,
.profile-avatar img {
  width: 100%;
  height: 100%;
  object-fit: cover;
}

.member-avatar {
  width: 36px;
  height: 36px;
  font-size: 14px;
  box-shadow: 0 1px 3px rgba(0, 0, 0, 0.1);
}

.owner-avatar {
  box-shadow: 0 0 0 2px #fff, 0 0 0 3px rgba(181, 133, 29, 0.55);
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

.avatar-edit-button .el-icon {
  font-size: 12px;
}

.owner-card:hover .avatar-edit-button,
.right-member-card:hover .avatar-edit-button,
.avatar-edit-button:focus-visible {
  opacity: 1;
  transform: translateY(0) scale(1);
}

.avatar-edit-button:hover {
  background: #25664f;
}

/* ===== 文案区域 ===== */
.member-card-copy {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 2px;
  overflow: hidden;
}

.member-name-row {
  display: flex;
  align-items: center;
  gap: 6px;
  min-width: 0;
}

.member-card-copy strong {
  min-width: 0;
  font-size: 13px;
  font-weight: 700;
  color: #1a2620;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.identity-badge {
  display: inline-flex;
  flex: 0 0 auto;
  align-items: center;
  justify-content: center;
  min-height: 20px;
  border: 1px solid currentColor;
  border-radius: 999px;
  padding: 1px 7px;
  font-size: 11px;
  font-weight: 800;
  line-height: 1;
}

.identity-badge.admin {
  color: #2f7a61;
  background: #eef8f2;
}

.identity-badge.owner {
  color: #9a6a13;
  background: #fff6dc;
}

.member-card-copy .member-sub {
  font-size: 12px;
  color: #8a958e;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

/* ===== Popover 弹出卡片 ===== */
:global(.member-popover.el-popover) {
  border-radius: 8px;
  padding: 0;
}

.member-profile-card {
  display: flex;
  flex-direction: column;
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
  flex: 1;
  min-width: 0;
}

.profile-title > span,
.profile-badges > span:not(.identity-badge) {
  display: block;
  overflow: hidden;
  margin-top: 3px;
  color: #6d7871;
  font-size: 12px;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.profile-badges {
  display: flex;
  flex-wrap: wrap;
  gap: 5px;
  margin-top: 5px;
}

.profile-details {
  display: flex;
  flex-direction: column;
  gap: 8px;
  margin: 0;
}

.profile-details div,
.profile-muted-row,
.profile-admin-row,
.profile-switch-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
}

.profile-details dt,
.profile-muted-row span,
.profile-admin-row span,
.profile-switch-row span,
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

.profile-settings {
  display: flex;
  flex-direction: column;
  gap: 10px;
}

.profile-settings label {
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.profile-settings label > span {
  color: #78817a;
  font-size: 12px;
  font-weight: 700;
}

.profile-settings :deep(.el-select),
.profile-settings :deep(.el-input-number) {
  width: 100%;
}

.temperature-control {
  display: flex;
  align-items: center;
  gap: 10px;
}

.temperature-control :deep(.el-slider) {
  flex: 1;
  --el-slider-main-bg-color: #2f7a61;
}

.profile-prompt {
  display: flex;
  flex-direction: column;
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
