<script setup lang="ts">
import { ChatDotRound, CirclePlus, MagicStick } from "@element-plus/icons-vue";
import type { ChatGroup } from "../stores/settings";

defineProps<{
  groups: ChatGroup[];
  activeGroupId?: string;
}>();

const emit = defineEmits<{
  selectGroup: [groupId: string];
  createGroup: [];
}>();
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
        <p class="eyebrow">Matrix Of Prescience</p>
        <div class="brand-title-row">
          <h1>Agent 群聊</h1>
          <el-button
            class="group-create-icon"
            circle
            type="primary"
            :icon="CirclePlus"
            title="新建聊天群"
            @click="emit('createGroup')"
          />
        </div>
      </div>
    </div>

    <section class="section-block">
      <div class="section-heading">
        <span>聊天群</span>
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
            <span>{{ group.members.length }} 个群友 · {{ group.messages.length }} 条消息</span>
          </div>
        </button>
      </div>
    </section>

  </aside>
</template>
