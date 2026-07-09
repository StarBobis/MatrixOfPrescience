<script setup lang="ts">
import { computed, ref } from "vue";
import GroupMemberPanel from "./GroupMemberPanel.vue";
import type {
  AgentCollaborationConfig,
  AgentMode,
  AgentWorkflowMode,
  AgentApprovalMode,
  AgentSafetyModel,
  AgentModel,
  AgentPatchProposal,
  OwnerProfile,
  PatchApprovalStatus,
  ProviderId,
} from "../stores/settings";

const announcement = defineModel<string>("announcement", { default: "" });
const agentConfig = defineModel<AgentCollaborationConfig>("agentConfig", { required: true });
const editingAnnouncement = ref(false);

const agentModeOptions: Array<{ label: string; value: AgentMode }> = [
  { label: "聊天讨论", value: "chat" },
  { label: "本地 Agent", value: "local-agent" },
  { label: "Architect", value: "architect" },
];

const workflowModeOptions: Array<{ label: string; value: AgentWorkflowMode }> = [
  { label: "Ask", value: "ask" },
  { label: "EditBeforeAsk", value: "edit-before-ask" },
  { label: "Code", value: "code" },
  { label: "YOLO", value: "yolo" },
];

const approvalModeOptions: Array<{ label: string; value: AgentApprovalMode }> = [
  { label: "手动确认", value: "manual" },
  { label: "高风险确认", value: "confirm-risky" },
  { label: "自动批准", value: "auto" },
];

const safetyModelOptions: Array<{ label: string; value: AgentSafetyModel }> = [
  { label: "Strict", value: "strict" },
  { label: "Balanced", value: "balanced" },
  { label: "Security Analyzer", value: "security-analyzer" },
  { label: "Sandbox YOLO", value: "sandbox-yolo" },
];

const props = defineProps<{
  members: AgentModel[];
  ownerProfile: OwnerProfile;
  patchProposals: AgentPatchProposal[];
  getProviderLabel: (provider: ProviderId) => string;
}>();

const emit = defineEmits<{
  addMember: [provider: ProviderId];
  removeMember: [memberId: string];
  updatePatchStatus: [proposalId: string, status: PatchApprovalStatus];
  removePatchProposal: [proposalId: string];
}>();

const pendingPatchCount = computed(
  () => props.patchProposals.filter((proposal) => proposal.status === "pending").length,
);

const patchRiskType: Record<AgentPatchProposal["riskLevel"], "success" | "warning" | "danger"> = {
  low: "success",
  medium: "warning",
  high: "danger",
};

const patchStatusType: Record<PatchApprovalStatus, "info" | "success" | "danger" | "warning"> = {
  pending: "warning",
  approved: "success",
  rejected: "danger",
  discarded: "info",
};

const patchStatusText: Record<PatchApprovalStatus, string> = {
  pending: "待审批",
  approved: "已应用",
  rejected: "已拒绝",
  discarded: "已丢弃",
};

const safetyVerdictType: Record<
  AgentPatchProposal["safetyCheck"]["verdict"],
  "success" | "warning" | "danger"
> = {
  allow: "success",
  "needs-confirmation": "warning",
  blocked: "danger",
};

const safetyVerdictText: Record<AgentPatchProposal["safetyCheck"]["verdict"], string> = {
  allow: "允许",
  "needs-confirmation": "需确认",
  blocked: "已阻止",
};

function syncEditBeforeAsk(value: boolean) {
  if (value) {
    agentConfig.value.workflowMode = "edit-before-ask";
    agentConfig.value.yoloMode = false;
  } else if (agentConfig.value.workflowMode === "edit-before-ask") {
    agentConfig.value.workflowMode = "ask";
  }
}

function syncYoloMode(value: boolean) {
  if (value) {
    agentConfig.value.agentMode = "local-agent";
    agentConfig.value.editBeforeAsk = false;
    agentConfig.value.workflowMode = "yolo";
    agentConfig.value.approvalMode = "auto";
    agentConfig.value.safetyModel = "sandbox-yolo";
    return;
  }

  if (agentConfig.value.workflowMode === "yolo") {
    agentConfig.value.workflowMode = "code";
  }

  if (agentConfig.value.approvalMode === "auto") {
    agentConfig.value.approvalMode = "confirm-risky";
  }
}

function syncWorkflowMode(value: AgentWorkflowMode) {
  agentConfig.value.editBeforeAsk = value === "edit-before-ask";
  agentConfig.value.yoloMode = value === "yolo";

  if (value === "yolo") {
    agentConfig.value.agentMode = "local-agent";
    agentConfig.value.approvalMode = "auto";
    agentConfig.value.safetyModel = "sandbox-yolo";
  }
}

function finishAnnouncementEdit() {
  editingAnnouncement.value = false;
}
</script>

<template>
  <aside class="right-panel">
    <section class="announcement-panel">
      <div class="section-heading">
        <span>群公告</span>
        <el-tag size="small" type="warning">基础约定</el-tag>
      </div>
      <div
        v-if="!editingAnnouncement"
        class="announcement-view"
        title="双击编辑群公告"
        @dblclick="editingAnnouncement = true"
      >
        {{ announcement || "双击编辑群公告" }}
      </div>
      <el-input
        v-else
        v-model="announcement"
        type="textarea"
        :autosize="{ minRows: 7, maxRows: 12 }"
        resize="none"
        placeholder="写给所有群友的基础约定提示词"
        @blur="finishAnnouncementEdit"
        @keydown.ctrl.enter.prevent="finishAnnouncementEdit"
      />
	    </section>

    <section class="agent-panel">
      <div class="section-heading">
        <span>Agent 协作</span>
        <el-tag size="small" type="info">{{ agentConfig.agentMode }}</el-tag>
      </div>

      <el-form label-position="top" class="agent-settings-form">
        <div class="agent-settings-grid">
          <el-form-item label="Agent 模式">
            <el-select v-model="agentConfig.agentMode">
              <el-option
                v-for="option in agentModeOptions"
                :key="option.value"
                :label="option.label"
                :value="option.value"
              />
            </el-select>
          </el-form-item>

          <el-form-item label="工作流">
            <el-select v-model="agentConfig.workflowMode" @change="syncWorkflowMode">
              <el-option
                v-for="option in workflowModeOptions"
                :key="option.value"
                :label="option.label"
                :value="option.value"
              />
            </el-select>
          </el-form-item>
        </div>

        <div class="agent-settings-grid">
          <el-form-item label="审批模式">
            <el-select v-model="agentConfig.approvalMode">
              <el-option
                v-for="option in approvalModeOptions"
                :key="option.value"
                :label="option.label"
                :value="option.value"
              />
            </el-select>
          </el-form-item>

          <el-form-item label="安全模型">
            <el-select v-model="agentConfig.safetyModel">
              <el-option
                v-for="option in safetyModelOptions"
                :key="option.value"
                :label="option.label"
                :value="option.value"
              />
            </el-select>
          </el-form-item>
        </div>

        <div class="agent-mode-toggles">
          <el-checkbox
            v-model="agentConfig.editBeforeAsk"
            @change="(value: boolean) => syncEditBeforeAsk(value)"
          >
            EditBeforeAsk
          </el-checkbox>
          <el-checkbox
            v-model="agentConfig.yoloMode"
            @change="(value: boolean) => syncYoloMode(value)"
          >
            YOLO 模式
          </el-checkbox>
        </div>
      </el-form>
    </section>

    <section class="patch-panel">
      <div class="section-heading">
        <span>审批队列</span>
        <el-tag size="small" :type="pendingPatchCount > 0 ? 'warning' : 'info'">
          {{ pendingPatchCount }}
        </el-tag>
      </div>

      <div v-if="patchProposals.length === 0" class="patch-empty">
        还没有补丁提案
      </div>

      <div v-else class="patch-list">
        <article
          v-for="proposal in patchProposals"
          :key="proposal.id"
          class="patch-card"
          :class="proposal.status"
        >
          <div class="patch-card-head">
            <div>
              <strong>{{ proposal.title }}</strong>
              <span>{{ proposal.proposerName }} · {{ proposal.createdAt }}</span>
            </div>
            <div class="patch-tags">
              <el-tag size="small" :type="patchRiskType[proposal.riskLevel]">
                {{ proposal.riskLevel }}
              </el-tag>
              <el-tag size="small" :type="patchStatusType[proposal.status]">
                {{ patchStatusText[proposal.status] }}
              </el-tag>
              <el-tag size="small" :type="safetyVerdictType[proposal.safetyCheck.verdict]">
                {{ safetyVerdictText[proposal.safetyCheck.verdict] }}
              </el-tag>
            </div>
          </div>

          <p class="patch-summary">{{ proposal.summary }}</p>

          <div class="patch-safety">
            <strong>安全校验</strong>
            <ul>
              <li v-for="reason in proposal.safetyCheck.reasons" :key="reason">
                {{ reason }}
              </li>
              <li v-for="warning in proposal.safetyCheck.warnings" :key="warning" class="warning">
                {{ warning }}
              </li>
            </ul>
          </div>

          <div v-if="proposal.files.length > 0" class="patch-files">
            <span v-for="file in proposal.files" :key="file">{{ file }}</span>
          </div>
          <div v-else class="patch-files muted">
            未识别具体文件
          </div>

          <pre v-if="proposal.patchText" class="patch-preview">{{ proposal.patchText }}</pre>

          <div class="patch-actions">
            <template v-if="proposal.status === 'pending'">
              <el-button
                size="small"
                type="primary"
                :disabled="proposal.safetyCheck.verdict === 'blocked' || !proposal.patchText"
                @click="emit('updatePatchStatus', proposal.id, 'approved')"
              >
                批准并应用
              </el-button>
              <el-button
                size="small"
                type="danger"
                plain
                @click="emit('updatePatchStatus', proposal.id, 'rejected')"
              >
                拒绝
              </el-button>
            </template>
            <el-button
              size="small"
              plain
              @click="emit('removePatchProposal', proposal.id)"
            >
              丢弃
            </el-button>
          </div>
        </article>
      </div>
    </section>

    <GroupMemberPanel
      :members="members"
      :owner-profile="ownerProfile"
      :get-provider-label="getProviderLabel"
      @add-member="(provider) => emit('addMember', provider)"
      @remove-member="(memberId) => emit('removeMember', memberId)"
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
.agent-panel,
.patch-panel {
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

.profile-title strong,
.profile-title span {
  display: block;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.profile-title strong {
  color: #1f2b25;
  font-size: 15px;
}

.profile-title span {
  margin-top: 3px;
  color: #6d7871;
  font-size: 12px;
}

.profile-details {
  display: grid;
  gap: 8px;
  margin: 0;
}

.profile-details div,
.profile-muted-row {
  display: grid;
  grid-template-columns: 64px minmax(0, 1fr);
  align-items: center;
  gap: 10px;
}

.profile-details dt,
.profile-muted-row span,
.profile-prompt span {
  color: #78817a;
  font-size: 12px;
  font-weight: 700;
}

.profile-details dd {
  min-width: 0;
  margin: 0;
  overflow: hidden;
  color: #24312a;
  font-size: 13px;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.profile-prompt {
  display: grid;
  gap: 6px;
}

.profile-prompt :deep(.el-textarea__inner) {
  border-radius: 8px;
  box-shadow: none;
  color: #344039;
  font-size: 13px;
  line-height: 1.55;
}

.profile-actions {
  display: flex;
  gap: 8px;
  padding-top: 8px;
  border-top: 1px solid #e8ece7;
}

@media (max-width: 980px) {
  .right-panel {
    min-height: auto;
  }
}
</style>
