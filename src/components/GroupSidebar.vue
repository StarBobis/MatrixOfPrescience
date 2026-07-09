<script setup lang="ts">
import { ChatDotRound, CirclePlus, MagicStick } from "@element-plus/icons-vue";
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
        <el-icon>
          <MagicStick />
        </el-icon>
      </div>
      <div>
        <p class="eyebrow">{{ t("common.appName") }}</p>
        <div class="brand-title-row">
          <h1>{{ t("groups.brandTitle") }}</h1>
          <el-button
            class="group-create-icon"
            circle
            type="primary"
            :icon="CirclePlus"
            :title="t('groups.createTitle')"
            @click="emit('createGroup')"
          />
        </div>
      </div>
    </div>

    <section class="section-block">
      <div class="section-heading">
        <span>{{ t("groups.listTitle") }}</span>
        <el-tag size="small" type="success">{{ groups.length }}</el-tag>
      </div>

      <div class="group-list">
        <button
          v-for="group in groups"
          :key="group.id"
          class="group-card"
          :class="{ active: group.id === activeGroupId }"
          @click="emit('selectGroup', group.id)"
        >
          <div class="group-avatar">
            <el-icon>
              <ChatDotRound />
            </el-icon>
          </div>
          <div class="group-main">
            <strong>{{ group.name }}</strong>
            <span>
              {{ t("groups.meta", { members: group.members.length, messages: group.messages.length }) }}
            </span>
          </div>
        </button>
      </div>
    </section>

  </aside>
</template>
