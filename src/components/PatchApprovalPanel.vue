<script setup lang="ts">
import { computed } from "vue";
import { useI18n } from "vue-i18n";
import type { AgentPatchProposal, PatchApprovalStatus } from "../stores/settings";

const props = defineProps<{
  patchProposals: AgentPatchProposal[];
}>();

const emit = defineEmits<{
  updatePatchStatus: [proposalId: string, status: PatchApprovalStatus];
  removePatchProposal: [proposalId: string];
}>();

const { t } = useI18n();

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

const patchStatusTextKey: Record<PatchApprovalStatus, string> = {
  pending: "patch.status.pending",
  approved: "patch.status.approved",
  rejected: "patch.status.rejected",
  discarded: "patch.status.discarded",
};

const safetyVerdictType: Record<
  AgentPatchProposal["safetyCheck"]["verdict"],
  "success" | "warning" | "danger"
> = {
  allow: "success",
  "needs-confirmation": "warning",
  blocked: "danger",
};

const safetyVerdictTextKey: Record<AgentPatchProposal["safetyCheck"]["verdict"], string> = {
  allow: "patch.verdict.allow",
  "needs-confirmation": "patch.verdict.needsConfirmation",
  blocked: "patch.verdict.blocked",
};
</script>

<template>
  <section v-if="patchProposals.length > 0" class="patch-panel">
    <div class="patch-panel-head">
      <strong>{{ t("patch.panelTitle") }}</strong>
      <el-tag size="small" :type="pendingPatchCount > 0 ? 'warning' : 'info'">
        {{ t("patch.pendingCount", { count: pendingPatchCount }) }}
      </el-tag>
    </div>

    <div class="patch-list">
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
              {{ t(patchStatusTextKey[proposal.status]) }}
            </el-tag>
            <el-tag size="small" :type="safetyVerdictType[proposal.safetyCheck.verdict]">
              {{ t(safetyVerdictTextKey[proposal.safetyCheck.verdict]) }}
            </el-tag>
          </div>
        </div>

        <p class="patch-summary">{{ proposal.summary }}</p>

        <div class="patch-safety">
          <strong>{{ t("patch.safetyTitle") }}</strong>
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
        <div v-else class="patch-files muted">{{ t("patch.noFiles") }}</div>

        <pre v-if="proposal.patchText" class="patch-preview">{{ proposal.patchText }}</pre>

        <div class="patch-actions">
          <template v-if="proposal.status === 'pending'">
            <el-button
              size="small"
              type="primary"
              :disabled="proposal.safetyCheck.verdict === 'blocked' || !proposal.patchText"
              @click="emit('updatePatchStatus', proposal.id, 'approved')"
            >
              {{ t("patch.approveAndApply") }}
            </el-button>
            <el-button
              size="small"
              type="danger"
              plain
              @click="emit('updatePatchStatus', proposal.id, 'rejected')"
            >
              {{ t("common.reject") }}
            </el-button>
          </template>
          <el-button size="small" plain @click="emit('removePatchProposal', proposal.id)">
            {{ t("common.discard") }}
          </el-button>
        </div>
      </article>
    </div>
  </section>
</template>

<style scoped>
.patch-panel {
  display: grid;
  gap: 10px;
  margin: 12px 0;
  padding: 12px;
  border: 1px solid #dbe6de;
  border-radius: 8px;
  background: #f8fbf8;
}

.patch-panel-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 10px;
}

.patch-panel-head strong {
  color: #24312a;
  font-size: 14px;
}

.patch-list {
  display: grid;
  gap: 10px;
}

.patch-card {
  display: grid;
  gap: 10px;
  padding: 12px;
  border: 1px solid #d9e2dc;
  border-radius: 8px;
  background: #ffffff;
}

.patch-card.approved {
  border-color: #a7d5bd;
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
  margin: 0;
  color: #37423b;
  font-size: 12px;
  line-height: 1.55;
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
  max-height: 220px;
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
</style>
