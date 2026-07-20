<script setup lang="ts">
import { computed, ref } from "vue";
import { storeToRefs } from "pinia";
import { ElMessage, ElMessageBox } from "element-plus";
import {
  ChevronDown,
  FolderOpen,
  Image,
  ListPlus,
  PlugZap,
  Plus,
  RefreshCcw,
  Settings as SettingsIcon,
  Trash2,
  UserRound,
} from "@lucide/vue";
import { invoke, isTauri } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { useSettingsStore, type ProviderConfig } from "../stores/settings";
import { chooseLocalAvatar, getAvatarSrc } from "../utils/avatar";
import { getReadableTextColor } from "../utils/colorContrast";
import { DEFAULT_CONTEXT_LIMIT, providerPresets } from "../utils/modelCatalog";
import { useI18n } from "vue-i18n";
import type { AppLocale } from "../i18n/locales";

interface CcSwitchOpenAIConfig {
  source: string;
  providerName?: string;
  baseUrl: string;
  apiKey: string;
  model?: string;
  wireApi?: string;
  warning?: string;
}

const settingsStore = useSettingsStore();
const { providers, ownerProfile, locale, cacheDirectory, defaultCacheDirectory } =
  storeToRefs(settingsStore);
const { t } = useI18n();
const syncingCcSwitch = ref(false);
const canSyncCcSwitch = isTauri();

const languageOptions = computed<Array<{ label: string; value: AppLocale }>>(() => [
  { label: t("settings.languageOptions.en"), value: "en" },
  { label: t("settings.languageOptions.zhCN"), value: "zh-CN" },
]);

const providerList = computed<ProviderConfig[]>(() => Object.values(providers.value));

const wireApiOptions = computed(() => [
  { label: t("settings.providers.wireApiAuto"), value: "" },
  { label: t("settings.providers.wireApiChat"), value: "chat" },
  { label: t("settings.providers.wireApiResponses"), value: "responses" },
]);

function addProviderFromMenu(command: string | number | object) {
  settingsStore.addProvider(command === "custom" ? undefined : String(command));
}

function ensureDefaultModelListed(provider: ProviderConfig) {
  const model = provider.defaultModel.trim();

  if (model && !provider.models.includes(model)) {
    provider.models.push(model);
  }
}

interface ProviderProbeState {
  status: "busy" | "ok" | "error";
  latencyMs?: number;
  modelCount?: number;
  modelIds?: string[];
  message?: string;
}

const probeStates = ref<Record<string, ProviderProbeState>>({});

async function probeProvider(provider: ProviderConfig) {
  probeStates.value[provider.id] = { status: "busy" };

  try {
    const result = await invoke<{
      ok: boolean;
      latencyMs: number;
      modelIds: string[];
      error?: string;
    }>("probe_model_provider", {
      request: { baseUrl: provider.baseUrl, apiKey: provider.apiKey },
    });

    if (result.ok) {
      probeStates.value[provider.id] = {
        status: "ok",
        latencyMs: result.latencyMs,
        modelCount: result.modelIds.length,
        modelIds: result.modelIds,
      };
      return;
    }

    probeStates.value[provider.id] = {
      status: "error",
      message: result.error || t("settings.providers.probeError"),
    };
  } catch (error) {
    probeStates.value[provider.id] = { status: "error", message: String(error) };
  }
}

function importProbeModels(provider: ProviderConfig) {
  const probe = probeStates.value[provider.id];
  const ids = probe?.modelIds ?? [];

  if (ids.length === 0) {
    return;
  }

  const merged = new Set([...provider.models, ...ids]);
  const added = merged.size - provider.models.length;
  provider.models = [...merged];

  if (!provider.defaultModel.trim() && provider.models.length > 0) {
    provider.defaultModel = provider.models[0];
  }

  ElMessage.success(t("settings.providers.importOk", { count: added }));
}

async function confirmRemoveProvider(provider: ProviderConfig) {
  try {
    await ElMessageBox.confirm(
      t("settings.providers.confirmRemove.message", { name: provider.name || provider.id }),
      t("settings.providers.confirmRemove.title"),
      {
        confirmButtonText: t("settings.providers.remove"),
        cancelButtonText: t("common.cancel"),
        confirmButtonClass: "el-button--danger",
        type: "warning",
      },
    );
  } catch {
    return;
  }

  const result = settingsStore.removeProvider(provider.id);

  if (result !== true) {
    ElMessage.warning(t(`settings.providers.removeBlocked.${result}`));
  }
}

async function chooseOwnerAvatar() {
  const avatar = await chooseLocalAvatar();

  if (avatar) {
    ownerProfile.value.avatar = avatar;
  }
}

async function chooseCacheDirectory() {
  const selected = await open({
    directory: true,
    multiple: false,
    title: t("settings.cache.chooseTitle"),
  });

  if (typeof selected === "string") {
    await settingsStore.setCacheDirectory(selected);
  }
}

async function syncOpenAIFromCcSwitch() {
  syncingCcSwitch.value = true;

  try {
    const config = await invoke<CcSwitchOpenAIConfig>("load_ccswitch_openai_config");
    const provider = providers.value.openai;

    if (!provider) {
      ElMessage.warning(t("settings.providers.ccswitchNoOpenAI"));
      return;
    }

    provider.baseUrl = config.baseUrl;
    provider.apiKey = config.apiKey;
    provider.wireApi = config.wireApi;

    if (config.model?.trim()) {
      provider.defaultModel = config.model.trim();
    }

    ElMessage.success(t("settings.providers.ccswitchSuccess", { source: config.source }));

    if (config.warning) {
      const warningKey = `settings.providers.ccswitchWarnings.${config.warning}`;
      const warningText = t(warningKey);
      ElMessage.warning(warningText === warningKey ? config.warning : warningText);
    }
  } catch (error) {
    ElMessage.error(t("settings.providers.ccswitchFailed", { error: String(error) }));
  } finally {
    syncingCcSwitch.value = false;
  }
}
</script>

<template>
  <main class="settings-page" aria-labelledby="settings-page-title">
    <div class="settings-content">
    <header class="settings-hero">
      <div class="settings-hero-icon">
        <SettingsIcon aria-hidden="true" />
      </div>
      <div>
        <p class="settings-eyebrow">{{ t("common.settings") }}</p>
        <h1 id="settings-page-title">{{ t("settings.title") }}</h1>
      </div>
    </header>

    <div class="settings-layout">
      <section class="settings-panel" aria-labelledby="profile-settings-heading">
        <div class="settings-panel-head">
          <h2 id="profile-settings-heading">{{ t("settings.profile.title") }}</h2>
          <el-tag size="small" type="success">{{ t("common.ownerRole") }}</el-tag>
        </div>

        <div class="owner-preview">
          <span
            class="owner-avatar"
            :style="{
              background: ownerProfile.color,
              color: getReadableTextColor(ownerProfile.color),
            }"
          >
            <img v-if="ownerProfile.avatar" :src="getAvatarSrc(ownerProfile.avatar)" alt="" />
            <UserRound v-else aria-hidden="true" />
          </span>
          <div>
            <strong>{{ ownerProfile.name || t("common.ownerName") }}</strong>
            <span>{{ t("settings.profile.ownerScope") }}</span>
          </div>
        </div>

        <el-form label-position="top">
          <el-form-item :label="t('common.language')">
            <el-select v-model="locale" :aria-label="t('common.language')">
              <el-option
                v-for="option in languageOptions"
                :key="option.value"
                :label="option.label"
                :value="option.value"
              />
            </el-select>
          </el-form-item>
          <el-form-item :label="t('settings.cache.title')">
            <el-input
              v-model="cacheDirectory"
              :placeholder="defaultCacheDirectory || t('settings.cache.defaultPlaceholder')"
              :aria-label="t('settings.cache.title')"
              readonly
            >
              <template #append>
                <el-button :icon="FolderOpen" @click="chooseCacheDirectory">
                  {{ t("common.choose") }}
                </el-button>
              </template>
            </el-input>
            <p class="settings-help">
              {{ t("settings.cache.description", { path: defaultCacheDirectory }) }}
            </p>
          </el-form-item>
          <el-form-item :label="t('settings.profile.name')">
            <el-input
              v-model="ownerProfile.name"
              :placeholder="t('common.ownerName')"
              :aria-label="t('settings.profile.name')"
            />
          </el-form-item>
          <el-form-item :label="t('settings.profile.avatar')">
            <el-input
              v-model="ownerProfile.avatar"
              :placeholder="t('settings.profile.avatarPlaceholder')"
              :aria-label="t('settings.profile.avatar')"
            >
              <template #append>
                <el-button :icon="Image" @click="chooseOwnerAvatar">
                  {{ t("common.choose") }}
                </el-button>
              </template>
            </el-input>
          </el-form-item>
          <el-form-item :label="t('settings.profile.avatarColor')">
            <el-color-picker
              v-model="ownerProfile.color"
              :aria-label="t('settings.profile.avatarColor')"
            />
          </el-form-item>
        </el-form>
      </section>

      <section class="settings-panel providers-panel" aria-labelledby="provider-settings-heading">
        <div class="settings-panel-head">
          <h2 id="provider-settings-heading">{{ t("settings.providers.title") }}</h2>
          <div class="provider-head-actions">
            <el-tag size="small">{{ providerList.length }}</el-tag>
            <el-dropdown trigger="click" @command="addProviderFromMenu">
              <el-button size="small" type="primary" :icon="Plus">
                {{ t("settings.providers.add") }}
                <ChevronDown class="dropdown-chevron" aria-hidden="true" />
              </el-button>
              <template #dropdown>
                <el-dropdown-menu>
                  <el-dropdown-item
                    v-for="preset in providerPresets"
                    :key="preset.id"
                    :command="preset.id"
                  >
                    {{ preset.name }}
                  </el-dropdown-item>
                  <el-dropdown-item divided command="custom">
                    {{ t("settings.providers.addCustom") }}
                  </el-dropdown-item>
                </el-dropdown-menu>
              </template>
            </el-dropdown>
          </div>
        </div>

        <div class="provider-stack">
          <section
            v-for="provider in providerList"
            :key="provider.id"
            class="provider-card"
            :aria-labelledby="`provider-${provider.id}-heading`"
          >
            <div class="provider-card-head">
              <h3 :id="`provider-${provider.id}-heading`">{{ provider.name || provider.id }}</h3>
              <div class="provider-card-actions">
                <el-button
                  size="small"
                  plain
                  :icon="PlugZap"
                  :loading="probeStates[provider.id]?.status === 'busy'"
                  :disabled="!provider.baseUrl.trim()"
                  @click="probeProvider(provider)"
                >
                  {{ t("settings.providers.probeTest") }}
                </el-button>
                <el-button
                  v-if="(probeStates[provider.id]?.modelIds?.length ?? 0) > 0"
                  size="small"
                  type="primary"
                  plain
                  :icon="ListPlus"
                  @click="importProbeModels(provider)"
                >
                  {{
                    t("settings.providers.importModels", {
                      count: probeStates[provider.id]?.modelIds?.length ?? 0,
                    })
                  }}
                </el-button>
                <el-button
                  v-if="provider.id === 'openai' && canSyncCcSwitch"
                  size="small"
                  type="primary"
                  plain
                  :icon="RefreshCcw"
                  :loading="syncingCcSwitch"
                  @click="syncOpenAIFromCcSwitch"
                >
                  {{ t("settings.providers.ccswitchSync") }}
                </el-button>
                <el-color-picker
                  v-model="provider.color"
                  size="small"
                  :aria-label="`${provider.name} ${t('settings.providers.color')}`"
                />
                <el-tag size="small">{{ provider.id }}</el-tag>
                <el-button
                  size="small"
                  type="danger"
                  plain
                  :icon="Trash2"
                  :aria-label="t('settings.providers.remove')"
                  @click="confirmRemoveProvider(provider)"
                />
              </div>
            </div>

            <div
              v-if="probeStates[provider.id] && probeStates[provider.id].status !== 'busy'"
              class="provider-probe-result"
            >
              <el-tag
                v-if="probeStates[provider.id].status === 'ok'"
                size="small"
                type="success"
                effect="plain"
              >
                {{
                  t("settings.providers.probeOk", {
                    latency: probeStates[provider.id].latencyMs ?? 0,
                    count: probeStates[provider.id].modelCount ?? 0,
                  })
                }}
              </el-tag>
              <el-tag v-else size="small" type="danger" effect="plain">
                {{ probeStates[provider.id].message || t("settings.providers.probeError") }}
              </el-tag>
            </div>

            <el-form label-position="top">
              <el-form-item :label="t('settings.providers.name')">
                <el-input
                  v-model="provider.name"
                  :placeholder="provider.id"
                  :aria-label="`${provider.id} ${t('settings.providers.name')}`"
                />
              </el-form-item>
              <el-form-item :label="t('common.apiKey')">
                <el-input
                  v-model="provider.apiKey"
                  type="password"
                  show-password
                  placeholder="sk-..."
                  :aria-label="`${provider.name} ${t('common.apiKey')}`"
                />
              </el-form-item>
              <el-form-item :label="t('common.baseUrl')">
                <el-input
                  v-model="provider.baseUrl"
                  placeholder="https://api.example.com/v1"
                  :aria-label="`${provider.name} ${t('common.baseUrl')}`"
                />
              </el-form-item>
              <el-form-item :label="t('settings.providers.wireApi')">
                <el-select
                  v-model="provider.wireApi"
                  :aria-label="`${provider.name} ${t('settings.providers.wireApi')}`"
                >
                  <el-option
                    v-for="option in wireApiOptions"
                    :key="option.value"
                    :label="option.label"
                    :value="option.value"
                  />
                </el-select>
              </el-form-item>
              <el-form-item :label="t('settings.providers.models')">
                <el-select
                  v-model="provider.models"
                  multiple
                  filterable
                  allow-create
                  default-first-option
                  :placeholder="t('settings.providers.modelsPlaceholder')"
                  :aria-label="`${provider.name} ${t('settings.providers.models')}`"
                >
                  <el-option
                    v-for="model in provider.models"
                    :key="model"
                    :label="model"
                    :value="model"
                  />
                </el-select>
              </el-form-item>
              <el-form-item :label="t('settings.providers.defaultModel')">
                <el-select
                  v-model="provider.defaultModel"
                  filterable
                  allow-create
                  default-first-option
                  :aria-label="`${provider.name} ${t('settings.providers.defaultModel')}`"
                  @change="ensureDefaultModelListed(provider)"
                >
                  <el-option
                    v-for="model in provider.models"
                    :key="model"
                    :label="model"
                    :value="model"
                  />
                </el-select>
              </el-form-item>
              <el-form-item :label="t('settings.providers.contextLimit')">
                <el-input-number
                  v-model="provider.contextLimit"
                  :min="1000"
                  :step="10000"
                  :controls="false"
                  :placeholder="String(DEFAULT_CONTEXT_LIMIT)"
                  :aria-label="`${provider.name} ${t('settings.providers.contextLimit')}`"
                />
                <p class="settings-help">{{ t("settings.providers.contextLimitHelp") }}</p>
              </el-form-item>
            </el-form>
          </section>
        </div>
      </section>
    </div>
    </div>
  </main>
</template>

<style scoped>
.settings-page {
  width: 100%;
  flex: 1;
  min-width: 0;
  min-height: 0;
  overflow: auto;
  padding: 0;
  background: var(--app-bg);
  scrollbar-gutter: stable;
}

.settings-content {
  width: min(1120px, 100%);
  min-width: 0;
  margin: 0 auto;
  padding: 28px 30px 40px;
}

.settings-hero {
  display: flex;
  align-items: center;
  gap: 12px;
  min-width: 0;
  margin: 0 0 24px;
  padding: 0 0 20px;
  border: 0;
  border-bottom: 1px solid var(--separator);
  border-radius: 0;
  background: transparent;
  box-shadow: none;
}

.settings-hero > div {
  min-width: 0;
}

.settings-hero-icon {
  display: grid;
  width: 38px;
  height: 38px;
  place-items: center;
  border: 1px solid color-mix(in srgb, var(--accent) 28%, transparent);
  border-radius: 8px;
  color: var(--accent-text);
  background: var(--accent-soft);
}

.settings-hero-icon svg {
  width: 20px;
  height: 20px;
  stroke-width: 1.8;
}

.settings-eyebrow,
.owner-preview span {
  margin: 0;
  color: var(--text-secondary);
  font-size: 12px;
  font-weight: 700;
  letter-spacing: 0;
  text-transform: none;
}

h1 {
  margin: 0;
  color: var(--text-primary);
  font-size: 20px;
  font-weight: 700;
  line-height: 1.2;
}

.settings-layout {
  display: grid;
  width: 100%;
  min-width: 0;
  grid-template-columns: minmax(280px, 340px) minmax(0, 1fr);
  gap: 30px;
  align-items: start;
}

.settings-panel,
.provider-card {
  border-radius: 0;
}

.settings-panel {
  display: grid;
  align-content: start;
  min-width: 0;
  gap: 16px;
  padding: 0;
  border: 0;
  background: transparent;
}

.settings-panel:first-child {
  padding-right: 30px;
  border-right: 1px solid var(--separator);
}

.settings-panel-head,
.provider-card-head,
.owner-preview {
  display: flex;
  align-items: center;
  gap: 10px;
}

.settings-panel-head,
.provider-card-head {
  justify-content: space-between;
}

.settings-panel-head {
  min-height: 28px;
}

.settings-panel-head h2,
.provider-card-head h3 {
  min-width: 0;
  margin: 0;
  overflow: hidden;
  color: var(--text-primary);
  text-overflow: ellipsis;
  white-space: nowrap;
}

.settings-panel-head h2 {
  font-size: 14px;
}

.provider-card-head h3 {
  font-size: 13px;
}

.provider-head-actions {
  display: flex;
  align-items: center;
  flex: 0 0 auto;
  gap: 8px;
}

.dropdown-chevron {
  width: 14px;
  height: 14px;
  margin-left: 4px;
}

.owner-preview {
  border-color: var(--separator);
  background: var(--surface-secondary);
}

.owner-preview strong,
.owner-preview span {
  display: block;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
}

.owner-preview strong {
  color: var(--text-primary);
  font-size: 15px;
}

.owner-preview span {
  margin-top: 4px;
}

.owner-avatar {
  display: grid;
  width: 48px;
  height: 48px;
  flex: 0 0 auto;
  place-items: center;
  overflow: hidden;
  border-radius: 50%;
  color: #ffffff;
  font-size: 22px;
}

.owner-avatar svg {
  width: 22px;
  height: 22px;
}

.owner-avatar img {
  width: 100%;
  height: 100%;
  object-fit: cover;
}

.settings-help {
  margin: 6px 0 0;
  overflow-wrap: anywhere;
  color: var(--text-secondary);
  font-size: 12px;
  line-height: 1.45;
}

.provider-stack {
  display: grid;
  min-width: 0;
  gap: 10px;
}

.provider-card {
  min-width: 0;
  padding: 14px 16px;
  border: 1px solid var(--separator);
  border-radius: 8px;
  background: var(--surface);
}

.provider-card + .provider-card {
  margin-top: 0;
}

.provider-card-head {
  min-width: 0;
  padding-bottom: 12px;
  border-bottom: 1px solid var(--separator);
}

.provider-card-actions {
  display: inline-flex;
  flex: 0 0 auto;
  align-items: center;
  gap: 8px;
}

.provider-probe-result {
  margin-top: 10px;
}

.provider-probe-result .el-tag {
  max-width: 100%;
  white-space: normal;
  height: auto;
  line-height: 1.4;
  padding-top: 4px;
  padding-bottom: 4px;
}

.provider-card-head strong {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

:deep(.el-form),
:deep(.el-form-item),
:deep(.el-select),
:deep(.el-input) {
  min-width: 0;
  max-width: 100%;
}

:deep(.el-select),
:deep(.el-input) {
  width: 100%;
}

:deep(.el-input__inner) {
  min-width: 0;
  text-overflow: ellipsis;
}

.provider-card :deep(.el-form) {
  padding-top: 14px;
}

.provider-card :deep(.el-form-item:last-child),
.settings-panel :deep(.el-form-item:last-child) {
  margin-bottom: 0;
}

@media (max-width: 900px) {
  .settings-content {
    padding: 22px 20px 36px;
  }

  .settings-layout {
    grid-template-columns: 1fr;
    gap: 28px;
  }

  .settings-panel:first-child {
    padding-right: 0;
    padding-bottom: 28px;
    border-right: 0;
    border-bottom: 1px solid var(--separator);
  }
}

@media (max-width: 520px) {
  .settings-content {
    padding: 18px 14px 30px;
  }

  .provider-card-head,
  .provider-card-actions {
    align-items: flex-start;
    flex-direction: column;
  }
}
</style>
