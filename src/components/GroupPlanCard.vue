<script setup lang="ts">
import { computed } from "vue";
import { Check, ChevronDown, ClipboardList, Ellipsis, PenLine, Play } from "@lucide/vue";
import { useI18n } from "vue-i18n";
import { getAvatarSrc } from "../utils/avatar";
import { getReadableTextColor } from "../utils/colorContrast";
import type { AgentModel, GroupPlan } from "../stores/settings";

const props = defineProps<{
  plan: GroupPlan;
  members: AgentModel[];
  renderMarkdown: (source: string) => string;
}>();

const emit = defineEmits<{
  execute: [planId: string, memberId: string];
}>();

const { t } = useI18n();

const latestVersion = computed(() => props.plan.versions[props.plan.versions.length - 1]);

const statusTagType = computed(() => {
  switch (props.plan.status) {
    case "approved":
      return "success";
    case "executing":
      return "primary";
    case "done":
      return "info";
    default:
      return "warning";
  }
});

type VoteState = "author" | "agree" | "revise" | "pending";

const voteRows = computed(() => {
  const version = latestVersion.value;

  if (!version) {
    return [] as Array<{ member: AgentModel; state: VoteState }>;
  }

  const agreeIds = new Set(version.agreeMemberIds);
  const reviseIds = new Set(version.reviseMemberIds);

  return props.members.map((member) => {
    let state: VoteState = "pending";

    if (member.id === version.authorId) {
      state = "author";
    } else if (agreeIds.has(member.id)) {
      state = "agree";
    } else if (reviseIds.has(member.id)) {
      state = "revise";
    }

    return { member, state };
  });
});

const writableMembers = computed(() => props.members.filter((member) => member.canWrite));

function getInitial(name: string) {
  return name.trim().slice(0, 1) || "?";
}

function getVoteTitle(state: VoteState, name: string) {
  if (state === "author") {
    return t("plan.voterAuthor", { name });
  }

  if (state === "agree") {
    return t("plan.voterAgree", { name });
  }

  if (state === "revise") {
    return t("plan.voterRevise", { name });
  }

  return t("plan.voterPending", { name });
}

function executeWith(memberId: unknown) {
  if (typeof memberId === "string") {
    emit("execute", props.plan.id, memberId);
  }
}
</script>

<template>
  <section
    v-if="latestVersion"
    class="group-plan-card"
    :data-plan-status="plan.status"
    :aria-label="t('plan.cardLabel', { title: plan.title })"
  >
    <header class="group-plan-head">
      <span class="group-plan-icon" aria-hidden="true">
        <ClipboardList />
      </span>
      <strong class="group-plan-title">{{ plan.title }}</strong>
      <el-tag size="small" :type="statusTagType">{{ t(`plan.status.${plan.status}`) }}</el-tag>
      <span class="group-plan-version">v{{ plan.versions.length }}</span>
    </header>

    <div class="group-plan-body markdown-body" v-html="renderMarkdown(latestVersion.content)"></div>

    <div class="group-plan-votes">
      <span class="group-plan-votes-caption">{{ t("plan.votesCaption") }}</span>
      <span
        v-for="row in voteRows"
        :key="row.member.id"
        class="group-plan-voter"
        :data-vote="row.state"
        :title="getVoteTitle(row.state, row.member.name)"
      >
        <span
          class="group-plan-voter-avatar"
          :style="{
            background: row.member.color,
            color: getReadableTextColor(row.member.color),
          }"
        >
          <img v-if="row.member.avatar" :src="getAvatarSrc(row.member.avatar)" alt="" />
          <span v-else>{{ getInitial(row.member.name) }}</span>
        </span>
        <span class="group-plan-voter-name">{{ row.member.name }}</span>
        <Check v-if="row.state === 'agree' || row.state === 'author'" class="vote-icon agree" aria-hidden="true" />
        <PenLine v-else-if="row.state === 'revise'" class="vote-icon revise" aria-hidden="true" />
        <Ellipsis v-else class="vote-icon pending" aria-hidden="true" />
      </span>
    </div>

    <div v-if="plan.versions.length > 1" class="group-plan-versions">
      <span
        v-for="(version, index) in plan.versions"
        :key="version.id"
        class="group-plan-version-item"
        :class="{ current: index === plan.versions.length - 1 }"
      >
        v{{ index + 1 }} · {{ version.authorName }}
      </span>
    </div>

    <footer v-if="plan.status !== 'voting'" class="group-plan-actions">
      <template v-if="plan.status === 'approved'">
        <el-dropdown trigger="click" @command="executeWith">
          <el-button size="small" type="primary" :icon="Play" :disabled="writableMembers.length === 0">
            {{ t("plan.execute") }}
            <ChevronDown class="dropdown-chevron" aria-hidden="true" />
          </el-button>
          <template #dropdown>
            <el-dropdown-menu>
              <el-dropdown-item
                v-for="member in writableMembers"
                :key="member.id"
                :command="member.id"
              >
                {{ member.name }}
              </el-dropdown-item>
            </el-dropdown-menu>
          </template>
        </el-dropdown>
        <span v-if="writableMembers.length === 0" class="group-plan-hint">
          {{ t("plan.noWritableMembers") }}
        </span>
      </template>
      <span v-else-if="plan.status === 'executing'" class="group-plan-hint">
        {{ t("plan.executing", { name: plan.executorName ?? "" }) }}
      </span>
      <span v-else class="group-plan-hint">
        {{ t("plan.done", { name: plan.executorName ?? "" }) }}
      </span>
    </footer>
  </section>
</template>

<style scoped>
.group-plan-card {
  display: grid;
  min-width: 0;
  gap: 12px;
  margin: 0 0 12px;
  padding: 14px 16px;
  border: 1px solid var(--separator);
  border-radius: 8px;
  background: var(--surface);
}

.group-plan-head {
  display: flex;
  min-width: 0;
  align-items: center;
  gap: 8px;
}

.group-plan-icon {
  display: grid;
  width: 28px;
  height: 28px;
  flex: 0 0 auto;
  place-items: center;
  border-radius: 7px;
  color: var(--accent-text, var(--accent));
  background: var(--accent-soft);
}

.group-plan-icon svg {
  width: 16px;
  height: 16px;
  stroke-width: 1.8;
}

.group-plan-title {
  min-width: 0;
  overflow: hidden;
  color: var(--text-primary);
  font-size: 14px;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.group-plan-version {
  flex: 0 0 auto;
  color: var(--text-secondary);
  font-size: 11px;
  font-weight: 700;
}

.group-plan-head :deep(.el-tag) {
  flex: 0 0 auto;
  margin-left: auto;
}

.group-plan-body {
  min-width: 0;
  padding: 10px 12px;
  border: 1px solid var(--separator);
  border-radius: 7px;
  background: var(--surface-secondary);
  color: var(--text-primary);
  font-size: 13px;
  line-height: 1.65;
  overflow-wrap: anywhere;
}

.group-plan-body :deep(p:first-child),
.group-plan-body :deep(h1:first-child),
.group-plan-body :deep(h2:first-child),
.group-plan-body :deep(h3:first-child) {
  margin-top: 0;
}

.group-plan-body :deep(p:last-child),
.group-plan-body :deep(ul:last-child),
.group-plan-body :deep(ol:last-child) {
  margin-bottom: 0;
}

.group-plan-votes {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 6px;
}

.group-plan-votes-caption {
  margin-right: 2px;
  color: var(--text-secondary);
  font-size: 12px;
  font-weight: 700;
}

.group-plan-voter {
  display: inline-flex;
  min-width: 0;
  max-width: 100%;
  align-items: center;
  gap: 6px;
  border: 1px solid var(--separator);
  border-radius: 999px;
  padding: 2px 10px 2px 2px;
  background: var(--surface);
}

.group-plan-voter[data-vote="agree"],
.group-plan-voter[data-vote="author"] {
  border-color: color-mix(in srgb, var(--success) 45%, transparent);
}

.group-plan-voter[data-vote="revise"] {
  border-color: color-mix(in srgb, var(--warning) 55%, transparent);
}

.group-plan-voter-avatar {
  display: grid;
  width: 20px;
  height: 20px;
  flex: 0 0 auto;
  place-items: center;
  overflow: hidden;
  border-radius: 50%;
  font-size: 11px;
  font-weight: 700;
}

.group-plan-voter-avatar img {
  width: 100%;
  height: 100%;
  object-fit: cover;
}

.group-plan-voter-name {
  min-width: 0;
  overflow: hidden;
  color: var(--text-secondary);
  font-size: 12px;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.vote-icon {
  width: 13px;
  height: 13px;
  flex: 0 0 auto;
  stroke-width: 2.2;
}

.vote-icon.agree {
  color: var(--success);
}

.vote-icon.revise {
  color: var(--warning);
}

.vote-icon.pending {
  color: var(--text-secondary);
  opacity: 0.6;
}

.group-plan-versions {
  display: flex;
  flex-wrap: wrap;
  gap: 5px 10px;
  color: var(--text-secondary);
  font-size: 11px;
}

.group-plan-version-item.current {
  color: var(--text-primary);
  font-weight: 700;
}

.group-plan-actions {
  display: flex;
  align-items: center;
  gap: 10px;
  padding-top: 2px;
}

.group-plan-hint {
  color: var(--text-secondary);
  font-size: 12px;
}

.dropdown-chevron {
  width: 14px;
  height: 14px;
  margin-left: 4px;
}
</style>
