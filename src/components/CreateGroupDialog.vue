<script setup lang="ts">
import { CirclePlus, Delete } from "@element-plus/icons-vue";
import type { AgentModel, ProviderId } from "../stores/settings";

const open = defineModel<boolean>("open", { required: true });

defineProps<{
  name: string;
  description: string;
  announcement: string;
  members: AgentModel[];
  providerOptions: Array<{ label: string; value: ProviderId }>;
  modelPresets: Record<ProviderId, string[]>;
}>();

const emit = defineEmits<{
  "update:name": [value: string];
  "update:description": [value: string];
  "update:announcement": [value: string];
  addDraftMember: [provider: ProviderId];
  removeDraftMember: [memberId: string];
  updateDraftMemberProvider: [member: AgentModel];
  createGroup: [];
}>();
</script>

<template>
  <el-dialog v-model="open" title="新建聊天群" width="720px">
    <el-form label-position="top">
      <el-form-item label="群名称">
        <el-input :model-value="name" @update:model-value="emit('update:name', String($event))" />
      </el-form-item>
      <el-form-item label="群简介">
        <el-input
          :model-value="description"
          @update:model-value="emit('update:description', String($event))"
        />
      </el-form-item>
      <el-form-item label="群公告">
        <el-input
          :model-value="announcement"
          type="textarea"
          :autosize="{ minRows: 3, maxRows: 6 }"
          resize="none"
          @update:model-value="emit('update:announcement', String($event))"
        />
      </el-form-item>
    </el-form>

    <div class="settings-toolbar">
      <el-button type="primary" :icon="CirclePlus" @click="emit('addDraftMember', 'openai')">
        添加 ChatGPT 群友
      </el-button>
      <el-button :icon="CirclePlus" @click="emit('addDraftMember', 'deepseek')">
        添加 DeepSeek 群友
      </el-button>
    </div>

    <div class="settings-stack compact">
      <section v-for="member in members" :key="member.id" class="settings-card">
        <div class="settings-card-head">
          <el-input v-model="member.name" class="model-name-input" />
          <el-switch v-model="member.enabled" />
        </div>

        <div class="member-grid">
          <el-form-item label="API">
            <el-select v-model="member.provider" @change="emit('updateDraftMemberProvider', member)">
              <el-option
                v-for="option in providerOptions"
                :key="option.value"
                :label="option.label"
                :value="option.value"
              />
            </el-select>
          </el-form-item>
          <el-form-item label="模型">
            <el-select v-model="member.model" filterable allow-create default-first-option>
              <el-option
                v-for="preset in modelPresets[member.provider]"
                :key="preset"
                :label="preset"
                :value="preset"
              />
            </el-select>
          </el-form-item>
        </div>

        <el-form-item label="核心角色">
          <el-input
            v-model="member.systemPrompt"
            type="textarea"
            :autosize="{ minRows: 2, maxRows: 5 }"
            resize="none"
          />
        </el-form-item>

        <div class="card-actions">
          <el-button type="danger" plain :icon="Delete" @click="emit('removeDraftMember', member.id)">
            删除群友
          </el-button>
        </div>
      </section>
    </div>

    <template #footer>
      <el-button @click="open = false">取消</el-button>
      <el-button type="primary" @click="emit('createGroup')">创建群聊</el-button>
    </template>
  </el-dialog>
</template>
