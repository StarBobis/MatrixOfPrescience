<script setup lang="ts">
import { computed, nextTick } from "vue";
import { ElMessageBox } from "element-plus";
import { Check, Trash2, X } from "@lucide/vue";
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

function hasPatchRuntime(proposal: AgentPatchProposal) {
  return Boolean(
    proposal.applyError?.trim() ||
      proposal.applyStdout?.trim() ||
      proposal.applyStderr?.trim() ||
      (proposal.appliedFiles?.length ?? 0) > 0 ||
      proposal.status === "approved",
  );
}

async function confirmDiscard(proposal: AgentPatchProposal) {
  try {
    const proposalIndex = props.patchProposals.findIndex((item) => item.id === proposal.id);
    await ElMessageBox.confirm(
      t("patch.confirmDiscard.message", { title: proposal.title }),
      t("patch.confirmDiscard.title"),
      {
        confirmButtonText: t("common.discard"),
        cancelButtonText: t("common.cancel"),
        confirmButtonClass: "el-button--danger",
        type: "warning",
      },
    );
    emit("removePatchProposal", proposal.id);
    void nextTick(() => {
      const cards = document.querySelectorAll<HTMLElement>(".patch-card");
      const nextCard = cards[Math.min(proposalIndex, cards.length - 1)];

      (nextCard?.querySelector<HTMLElement>("button, summary, [tabindex='0']") ??
        document.querySelector<HTMLElement>(".composer textarea"))?.focus();
    });
  } catch {
    // Canceling keeps the proposal in the approval queue.
  }
}
</script>

<template>
  <section
    v-if="patchProposals.length > 0"
    class="patch-panel"
    aria-labelledby="patch-panel-title"
  >
    <div class="patch-panel-head">
      <h2 id="patch-panel-title">{{ t("patch.panelTitle") }}</h2>
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
        :aria-labelledby="`patch-title-${proposal.id}`"
      >
        <div class="patch-card-head">
          <div>
            <strong :id="`patch-title-${proposal.id}`">{{ proposal.title }}</strong>
            <span>{{ proposal.proposerName }} · {{ proposal.createdAt }}</span>
          </div>
          <div class="patch-tags">
            <el-tag size="small" :type="patchRiskType[proposal.riskLevel]">
              {{ t(`patch.risk.${proposal.riskLevel}`) }}
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

        <details v-if="proposal.patchText" class="patch-disclosure">
          <summary>{{ t("patch.preview") }}</summary>
          <pre class="patch-preview">{{ proposal.patchText }}</pre>
        </details>

        <div
          v-if="hasPatchRuntime(proposal)"
          class="patch-runtime"
          :class="{ error: Boolean(proposal.applyError?.trim()) }"
        >
          <strong>
            {{
              proposal.applyError?.trim()
                ? t("patchRuntime.failedContent", {
                    title: proposal.title,
                    error: proposal.applyError,
                  })
                : t("patchRuntime.appliedContent", { title: proposal.title })
            }}
          </strong>
          <p v-if="proposal.appliedFiles?.length">
            {{ t("patchRuntime.appliedFiles", { files: proposal.appliedFiles.join(", ") }) }}
          </p>
          <pre v-if="proposal.applyStdout?.trim()">{{ t("patchRuntime.output", { output: proposal.applyStdout }) }}</pre>
          <pre v-if="proposal.applyStderr?.trim()">{{ t("patchRuntime.stderr", { stderr: proposal.applyStderr }) }}</pre>
        </div>

        <div class="patch-actions">
          <template v-if="proposal.status === 'pending'">
            <el-button
              size="small"
              type="primary"
              :icon="Check"
              :disabled="proposal.safetyCheck.verdict === 'blocked' || !proposal.patchText"
              @click="emit('updatePatchStatus', proposal.id, 'approved')"
            >
              {{ t("patch.approveAndApply") }}
            </el-button>
            <el-button
              size="small"
              type="danger"
              plain
              :icon="X"
              @click="emit('updatePatchStatus', proposal.id, 'rejected')"
            >
              {{ t("common.reject") }}
            </el-button>
          </template>
          <el-button
            size="small"
            plain
            :icon="Trash2"
            @click="confirmDiscard(proposal)"
          >
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
  width: min(920px, 100%);
  margin: 0;
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

.patch-runtime {
  display: grid;
  gap: 6px;
  padding: 9px;
  border: 1px solid #cfe4d7;
  border-radius: 8px;
  color: #24513f;
  background: #f1fbf5;
}

.patch-runtime.error {
  border-color: #efc4c4;
  color: #963f3f;
  background: #fff4f4;
}

.patch-runtime strong {
  font-size: 12px;
  line-height: 1.45;
}

.patch-runtime p,
.patch-runtime pre {
  margin: 0;
  font-size: 12px;
  line-height: 1.5;
}

.patch-runtime pre {
  max-height: 160px;
  overflow: auto;
  padding: 8px;
  border-radius: 6px;
  color: #26312b;
  background: rgba(255, 255, 255, 0.72);
  white-space: pre-wrap;
}
</style>

<style scoped>
.patch-panel {
  display: flex;
  width: 100%;
  max-height: min(42vh, 420px);
  flex: 0 1 auto;
  flex-direction: column;
  gap: 10px;
  overflow: hidden;
  margin: 0;
  padding: 10px 14px 12px;
  border: 0;
  border-bottom: 1px solid var(--separator);
  border-radius: 0;
  background: var(--warning-soft);
}

.patch-panel-head {
  flex: 0 0 auto;
}

.patch-panel-head h2 {
  margin: 0;
  color: var(--text-primary);
  font-size: 13px;
  font-weight: 700;
}

.patch-list {
  min-height: 0;
  overflow: auto;
  padding-right: 2px;
}

.patch-card {
  border-color: var(--separator);
  background: var(--surface);
}

.patch-card.approved {
  border-color: color-mix(in srgb, var(--success) 42%, var(--separator));
}

.patch-card.rejected {
  border-color: color-mix(in srgb, var(--danger) 42%, var(--separator));
}

.patch-card-head strong {
  color: var(--text-primary);
}

.patch-card-head span,
.patch-summary {
  color: var(--text-secondary);
}

.patch-safety {
  border: 1px solid var(--separator);
  background: var(--surface-secondary);
}

.patch-safety strong,
.patch-safety ul {
  color: var(--text-secondary);
}

.patch-safety li.warning {
  color: var(--warning);
}

.patch-files span,
.patch-files.muted {
  color: var(--text-secondary);
  background: var(--surface-tertiary);
}

.patch-files.muted {
  color: var(--text-tertiary);
}

.patch-disclosure {
  overflow: hidden;
  border: 1px solid var(--separator);
  border-radius: 8px;
  background: var(--surface-secondary);
}

.patch-disclosure summary {
  min-height: 32px;
  padding: 7px 10px;
  color: var(--accent-text);
  cursor: pointer;
  font-size: 12px;
  font-weight: 700;
}

.patch-disclosure[open] summary {
  border-bottom: 1px solid var(--separator);
}

.patch-preview {
  max-height: 240px;
  border-radius: 0;
  color: var(--code-text);
  background: var(--code-bg);
  font-family: "Cascadia Code", "SFMono-Regular", Consolas, monospace;
}

.patch-runtime {
  border-color: color-mix(in srgb, var(--success) 34%, var(--separator));
  color: var(--success);
  background: var(--success-soft);
}

.patch-runtime.error {
  border-color: color-mix(in srgb, var(--danger) 36%, var(--separator));
  color: var(--danger);
  background: var(--danger-soft);
}

.patch-runtime pre {
  color: var(--code-text);
  background: var(--code-bg);
  font-family: "Cascadia Code", "SFMono-Regular", Consolas, monospace;
}

.patch-actions :deep(.el-button) {
  margin-left: 0;
}

@media (max-height: 720px) {
  .patch-panel {
    max-height: 34vh;
  }
}
</style>
