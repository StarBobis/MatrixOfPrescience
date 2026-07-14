<script setup lang="ts">
import { BrainCircuit, MessageSquare, Plus } from "@lucide/vue";
import { useI18n } from "vue-i18n";
import type { ChatGroup } from "../stores/settings";

defineProps<{
  groups: ChatGroup[];
  activeGroupId?: string;
}>();

const emit = defineEmits<{
  selectGroup: [groupId: string];
  createGroup: [];
}>();

const { t } = useI18n();
</script>

<template>
  <aside class="side-panel">
    <div class="brand-block">
      <div class="brand-mark">
        <BrainCircuit aria-hidden="true" />
      </div>
      <div>
        <p class="eyebrow">{{ t("common.appName") }}</p>
        <div class="brand-title-row">
          <h1>{{ t("groups.brandTitle") }}</h1>
          <el-button
            class="group-create-icon"
            circle
            type="primary"
            :icon="Plus"
            :title="t('groups.createTitle')"
            :aria-label="t('groups.createTitle')"
            @click="emit('createGroup')"
          />
        </div>
      </div>
    </div>

    <section class="section-block" aria-labelledby="group-list-heading">
      <div class="section-heading">
        <span id="group-list-heading">{{ t("groups.listTitle") }}</span>
        <el-tag size="small" type="success">{{ groups.length }}</el-tag>
      </div>

      <div v-if="groups.length" class="group-list">
        <button
          v-for="group in groups"
          :key="group.id"
          class="group-card"
          :class="{ active: group.id === activeGroupId }"
          type="button"
          :aria-current="group.id === activeGroupId ? 'page' : undefined"
          @click="emit('selectGroup', group.id)"
        >
          <div class="group-avatar">
            <MessageSquare aria-hidden="true" />
          </div>
          <div class="group-main">
            <strong>{{ group.name }}</strong>
            <span>
              {{ t("groups.meta", { members: group.members.length, messages: group.messages.length }) }}
            </span>
          </div>
        </button>
      </div>
      <div v-else class="group-list-empty">
        <MessageSquare aria-hidden="true" />
        <strong>{{ t("groups.emptyTitle") }}</strong>
        <span>{{ t("groups.emptyDescription") }}</span>
        <el-button type="primary" :icon="Plus" @click="emit('createGroup')">
          {{ t("groups.createTitle") }}
        </el-button>
      </div>
    </section>
  </aside>
</template>
