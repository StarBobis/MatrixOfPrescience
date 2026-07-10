import { defineStore } from "pinia";
import { invoke, isTauri } from "@tauri-apps/api/core";
import { getI18nLocale, translate as t } from "../i18n";
import { defaultLocale, normalizeLocale, type AppLocale } from "../i18n/locales";

export type ProviderId = "openai" | "deepseek";
export type ChatRole = "user" | "assistant";
export type MessageStatus = "done" | "thinking" | "error" | "interrupted";
export type ChatMessageActivityKind = "status" | "tool";
export type ChatMessageActivityStatus = "running" | "done" | "error" | "info" | "interrupted";
export type ChatMessageExecutionKind = "status" | "reasoning" | "tool";
export type ChatMessageExecutionStatus = ChatMessageActivityStatus;
export type AgentMode = "chat" | "local-agent" | "architect";
export type AgentWorkflowMode = "ask" | "edit-before-ask" | "code" | "yolo";
export type AgentApprovalMode = "manual" | "confirm-risky" | "auto";
export type AgentSafetyModel = "strict" | "balanced" | "security-analyzer" | "sandbox-yolo";
export type AgentReasoningEffort = "off" | "low" | "medium" | "high";
export type PatchRiskLevel = "low" | "medium" | "high";
export type PatchApprovalStatus = "pending" | "approved" | "rejected" | "discarded";
export type PatchSafetyVerdict = "allow" | "needs-confirmation" | "blocked";

export interface ProviderConfig {
  id: ProviderId;
  name: string;
  baseUrl: string;
  apiKey: string;
  defaultModel: string;
  wireApi?: string;
}

export interface AgentModel {
  id: string;
  libraryId?: string;
  name: string;
  provider: ProviderId;
  model: string;
  reasoningEffort: AgentReasoningEffort;
  systemPrompt: string;
  temperature: number;
  enabled: boolean;
  isAdmin: boolean;
  canWrite: boolean;
  deepSeekLongContext?: boolean;
  color: string;
  avatar?: string;
}

export interface ChatMessageActivityItem {
  id: string;
  kind: ChatMessageActivityKind;
  status: ChatMessageActivityStatus;
  text: string;
  detail?: string;
}

export interface ChatMessageExecutionItem {
  id: string;
  kind: ChatMessageExecutionKind;
  status: ChatMessageExecutionStatus;
  text: string;
  detail?: string;
  createdAt?: number;
}

export interface ChatMessage {
  id: string;
  role: ChatRole;
  modelName: string;
  providerName?: string;
  avatar?: string;
  apiModel?: string;
  reasoningEffort?: AgentReasoningEffort;
  startedAt?: number;
  durationMs?: number;
  contextUsedTokens?: number;
  contextLimitTokens?: number;
  contextPromptTokens?: number;
  contextCompletionTokens?: number;
  contextCacheHitTokens?: number;
  contextCacheMissTokens?: number;
  status: MessageStatus;
  content: string;
  time: string;
  color: string;
  agreeMemberIds?: string[];
  supplementMemberIds?: string[];
  disagreeMemberIds?: string[];
  thoughtSteps?: string[];
  activityItems?: ChatMessageActivityItem[];
  executionItems?: ChatMessageExecutionItem[];
}

export interface ChatGroup {
  id: string;
  name: string;
  description: string;
  announcement: string;
  workspacePath: string;
  agentConfig: AgentCollaborationConfig;
  patchProposals: AgentPatchProposal[];
  members: AgentModel[];
  messages: ChatMessage[];
  updatedAt: string;
}

export interface AgentCollaborationConfig {
  agentMode: AgentMode;
  workflowMode: AgentWorkflowMode;
  approvalMode: AgentApprovalMode;
  safetyModel: AgentSafetyModel;
  editBeforeAsk: boolean;
  yoloMode: boolean;
}

export interface OwnerProfile {
  name: string;
  avatar: string;
  color: string;
}

export interface AgentPatchProposal {
  id: string;
  title: string;
  proposerName: string;
  riskLevel: PatchRiskLevel;
  safetyCheck: PatchSafetyCheck;
  status: PatchApprovalStatus;
  workspacePath: string;
  files: string[];
  summary: string;
  patchText: string;
  createdAt: string;
  appliedFiles?: string[];
  applyStdout?: string;
  applyStderr?: string;
  applyError?: string;
}

export interface PatchSafetyCheck {
  verdict: PatchSafetyVerdict;
  reasons: string[];
  warnings: string[];
}

interface PersistedSettings {
  locale?: unknown;
  cacheDirectory?: string;
  providers?: Partial<Record<ProviderId, ProviderConfig>>;
  agentModels?: AgentModel[];
  memberLibrary?: AgentModel[];
  groups?: ChatGroup[];
  activeGroupId?: string;
  ownerProfile?: Partial<OwnerProfile>;
}

const STORAGE_KEY = "matrix-of-prescience-settings";
const MAX_PERSISTED_GROUPS = 12;
const NORMAL_MESSAGE_LIMIT = 80;
const COMPACT_MESSAGE_LIMIT = 20;
const NORMAL_MESSAGE_CONTENT_LIMIT = 12000;
const COMPACT_MESSAGE_CONTENT_LIMIT = 4000;
const NORMAL_PATCH_LIMIT = 8;
const NORMAL_PATCH_TEXT_LIMIT = 20000;
const COMPACT_PATCH_LIMIT = 3;
const THOUGHT_STEP_LIMIT = 96;
const THOUGHT_STEP_TEXT_LIMIT = 1200;
const ACTIVITY_ITEM_LIMIT = 36;
const ACTIVITY_ITEM_TEXT_LIMIT = 360;
const ACTIVITY_ITEM_DETAIL_LIMIT = 6000;
const EXECUTION_ITEM_LIMIT = 160;
const EXECUTION_ITEM_TEXT_LIMIT = 1200;
const EXECUTION_ITEM_DETAIL_LIMIT = 6000;
const PERSIST_DEBOUNCE_MS = 800;
let pendingPersistTimer: number | undefined;

type PersistenceMode = "normal" | "compact" | "minimal";

interface AppCacheState {
  defaultCacheDirectory: string;
  cacheDirectory: string;
  settings?: PersistedSettings;
  memberLibrary?: AgentModel[];
}

const providerDefaults: Record<ProviderId, ProviderConfig> = {
  openai: {
    id: "openai",
    name: "ChatGPT",
    baseUrl: "https://api.openai.com/v1",
    apiKey: "",
    defaultModel: "gpt-4.1-mini",
  },
  deepseek: {
    id: "deepseek",
    name: "DeepSeek",
    baseUrl: "https://api.deepseek.com",
    apiKey: "",
    defaultModel: "deepseek-v4-flash",
  },
};

function getDefaultAnnouncement() {
  return t("defaults.group.announcement");
}

function createDefaultOwnerProfile(): OwnerProfile {
  return {
    name: t("common.ownerName"),
    avatar: "",
    color: "#4d5a61",
  };
}

const defaultAgentConfig: AgentCollaborationConfig = {
  agentMode: "chat",
  workflowMode: "ask",
  approvalMode: "manual",
  safetyModel: "balanced",
  editBeforeAsk: false,
  yoloMode: false,
};

function normalizeAgentConfig(
  config?: Partial<AgentCollaborationConfig>,
): AgentCollaborationConfig {
  const merged = {
    ...structuredClone(defaultAgentConfig),
    ...config,
  };

  if (merged.yoloMode) {
    merged.workflowMode = "yolo";
    merged.approvalMode = "auto";
    merged.safetyModel = "sandbox-yolo";
  }

  if (merged.editBeforeAsk) {
    merged.workflowMode = "edit-before-ask";
  }

  return merged;
}

function normalizeGroup(group: ChatGroup): ChatGroup {
  const workspacePath = group.workspacePath ?? "";

  return {
    ...group,
    announcement: group.announcement ?? getDefaultAnnouncement(),
    workspacePath,
    agentConfig: normalizeAgentConfig(group.agentConfig),
    patchProposals: (group.patchProposals ?? []).map((proposal) => ({
      ...proposal,
      workspacePath: proposal.workspacePath ?? workspacePath,
      appliedFiles: proposal.appliedFiles ?? [],
      applyStdout: proposal.applyStdout ?? "",
      applyStderr: proposal.applyStderr ?? "",
      applyError: proposal.applyError ?? "",
      safetyCheck: proposal.safetyCheck ?? {
        verdict: proposal.riskLevel === "high" ? "blocked" : "needs-confirmation",
        reasons:
          proposal.riskLevel === "high"
            ? [t("patchSafety.legacyBlocked")]
            : [],
        warnings: [t("patchSafety.legacyWarning")],
      },
    })),
    members: group.members.map(normalizeMember),
    messages: group.messages.map((message) => ({
      ...message,
      status: normalizeMessageStatus(message.status),
      avatar: message.avatar ?? "",
      apiModel: message.apiModel ?? "",
      reasoningEffort: normalizeReasoningEffort(message.reasoningEffort),
      startedAt: Number.isFinite(message.startedAt) ? message.startedAt : undefined,
      durationMs: Number.isFinite(message.durationMs) ? message.durationMs : undefined,
      contextUsedTokens: Number.isFinite(message.contextUsedTokens) ? message.contextUsedTokens : undefined,
      contextLimitTokens: Number.isFinite(message.contextLimitTokens) ? message.contextLimitTokens : undefined,
      contextPromptTokens: Number.isFinite(message.contextPromptTokens) ? message.contextPromptTokens : undefined,
      contextCompletionTokens: Number.isFinite(message.contextCompletionTokens)
        ? message.contextCompletionTokens
        : undefined,
      contextCacheHitTokens: Number.isFinite(message.contextCacheHitTokens)
        ? message.contextCacheHitTokens
        : undefined,
      contextCacheMissTokens: Number.isFinite(message.contextCacheMissTokens)
        ? message.contextCacheMissTokens
        : undefined,
      agreeMemberIds: message.agreeMemberIds ?? [],
      supplementMemberIds: message.supplementMemberIds ?? [],
      disagreeMemberIds: message.disagreeMemberIds ?? [],
      thoughtSteps: message.thoughtSteps ?? [],
      activityItems: message.activityItems ?? [],
      executionItems: message.executionItems ?? [],
    })),
  };
}

function normalizeReasoningEffort(value: unknown): AgentReasoningEffort {
  return value === "low" || value === "medium" || value === "high" ? value : "off";
}

function normalizeMessageStatus(value: unknown): MessageStatus {
  return value === "done" || value === "thinking" || value === "error" || value === "interrupted"
    ? value
    : "done";
}

function normalizeMember(member: AgentModel): AgentModel {
  return {
    ...member,
    libraryId: member.libraryId,
    reasoningEffort: normalizeReasoningEffort(member.reasoningEffort),
    temperature: Number.isFinite(member.temperature) ? member.temperature : 0.7,
    isAdmin: Boolean(member.isAdmin),
    canWrite: Boolean(member.canWrite),
    deepSeekLongContext: member.provider === "deepseek" ? member.deepSeekLongContext ?? true : false,
  };
}

function nowText() {
  return new Intl.DateTimeFormat(getI18nLocale(), {
    hour: "2-digit",
    minute: "2-digit",
  }).format(new Date());
}

function createSystemMessage(content: string): ChatMessage {
  return {
    id: crypto.randomUUID(),
    role: "assistant",
    modelName: t("common.systemName"),
    avatar: "",
    apiModel: "",
    reasoningEffort: "off",
    status: "done",
    content,
    time: nowText(),
    color: "#6c6f75",
    agreeMemberIds: [],
    supplementMemberIds: [],
    disagreeMemberIds: [],
    thoughtSteps: [],
    activityItems: [],
    executionItems: [],
  };
}

function getModelColor(provider: ProviderId) {
  return provider === "openai" ? "#2f76b7" : "#2f7a61";
}

function createMember(
  provider: ProviderId,
  name: string,
  model: string,
  systemPrompt: string,
): AgentModel {
  return {
    id: crypto.randomUUID(),
    libraryId: crypto.randomUUID(),
    name,
    provider,
    model,
    reasoningEffort: "off",
    systemPrompt,
    temperature: 0.7,
    enabled: true,
    isAdmin: false,
    canWrite: false,
    deepSeekLongContext: provider === "deepseek",
    color: getModelColor(provider),
  };
}

function normalizeName(name: string) {
  return name.trim().toLocaleLowerCase();
}

function makeUniqueMemberName(name: string, members: AgentModel[], exceptId = "") {
  const base = name.trim() || t("common.memberFallback");
  const usedNames = new Set(
    members
      .filter((member) => member.id !== exceptId)
      .map((member) => normalizeName(member.name)),
  );

  if (!usedNames.has(normalizeName(base))) {
    return base;
  }

  let index = 2;
  let nextName = `${base} ${index}`;

  while (usedNames.has(normalizeName(nextName))) {
    index += 1;
    nextName = `${base} ${index}`;
  }

  return nextName;
}

function cloneMember(member: AgentModel, members: AgentModel[] = []) {
  const libraryId = member.libraryId ?? member.id;

  return {
    ...toPlainMember(member),
    id: crypto.randomUUID(),
    libraryId,
    name: makeUniqueMemberName(member.name, members),
  };
}

function toLibraryMember(member: AgentModel, friends: AgentModel[] = []) {
  const libraryId = member.libraryId ?? member.id ?? crypto.randomUUID();

  return {
    ...toPlainMember({
      ...member,
      libraryId,
    }),
    id: libraryId,
    libraryId,
    name: makeUniqueMemberName(member.name, friends, libraryId),
  };
}

function toPlainMember(member: AgentModel): AgentModel {
  return {
    id: member.id,
    libraryId: member.libraryId,
    name: member.name,
    provider: member.provider,
    model: member.model,
    reasoningEffort: normalizeReasoningEffort(member.reasoningEffort),
    systemPrompt: member.systemPrompt,
    temperature: member.temperature,
    enabled: member.enabled,
    isAdmin: Boolean(member.isAdmin),
    canWrite: Boolean(member.canWrite),
    deepSeekLongContext: member.provider === "deepseek" ? member.deepSeekLongContext ?? true : false,
    color: member.color,
    avatar: member.avatar,
  };
}

function truncateText(value: string, maxLength: number) {
  return value.length > maxLength ? `${value.slice(0, maxLength)}...` : value;
}

function isCommandOutputExecutionItem(item: ChatMessageExecutionItem) {
  const detail = item.detail ?? "";
  return item.kind === "tool" && (detail.includes("[stdout]") || detail.includes("[stderr]"));
}

function toPersistedMessage(message: ChatMessage, mode: Exclude<PersistenceMode, "minimal">) {
  const contentLimit =
    mode === "normal" ? NORMAL_MESSAGE_CONTENT_LIMIT : COMPACT_MESSAGE_CONTENT_LIMIT;

  return {
    ...message,
    content: truncateText(message.content, contentLimit),
    thoughtSteps: (message.thoughtSteps ?? [])
      .slice(-THOUGHT_STEP_LIMIT)
      .map((step) => truncateText(step, THOUGHT_STEP_TEXT_LIMIT)),
    activityItems: (message.activityItems ?? []).slice(-ACTIVITY_ITEM_LIMIT).map((item) => ({
      ...item,
      text: truncateText(item.text, ACTIVITY_ITEM_TEXT_LIMIT),
      detail: item.detail ? truncateText(item.detail, ACTIVITY_ITEM_DETAIL_LIMIT) : undefined,
    })),
    executionItems: (message.executionItems ?? []).slice(-EXECUTION_ITEM_LIMIT).map((item) => ({
      ...item,
      text: truncateText(item.text, EXECUTION_ITEM_TEXT_LIMIT),
      detail: item.detail
        ? mode === "normal" && isCommandOutputExecutionItem(item)
          ? item.detail
          : truncateText(item.detail, EXECUTION_ITEM_DETAIL_LIMIT)
        : undefined,
    })),
  };
}

function toPersistedPatchProposal(
  proposal: AgentPatchProposal,
  mode: Exclude<PersistenceMode, "minimal">,
) {
  return {
    ...proposal,
    summary: truncateText(proposal.summary, mode === "normal" ? 2000 : 500),
    appliedFiles: (proposal.appliedFiles ?? []).slice(0, 40),
    applyStdout: proposal.applyStdout
      ? truncateText(proposal.applyStdout, mode === "normal" ? 4000 : 1000)
      : "",
    applyStderr: proposal.applyStderr
      ? truncateText(proposal.applyStderr, mode === "normal" ? 4000 : 1000)
      : "",
    applyError: proposal.applyError
      ? truncateText(proposal.applyError, mode === "normal" ? 2000 : 500)
      : "",
    patchText:
      mode === "normal" ? truncateText(proposal.patchText, NORMAL_PATCH_TEXT_LIMIT) : "",
  };
}

function createDefaultMembers(): AgentModel[] {
  return [
    createMember(
      "openai",
      t("defaults.member.productManagerName"),
      "gpt-4.1-mini",
      t("defaults.member.productManagerPrompt"),
    ),
    createMember(
      "deepseek",
      t("defaults.member.technicalAdvisorName"),
      "deepseek-v4-flash",
      t("defaults.member.technicalAdvisorPrompt"),
    ),
  ];
}

function createDefaultGroup(): ChatGroup {
  return {
    id: crypto.randomUUID(),
    name: t("defaults.group.name"),
    description: t("defaults.group.description"),
    announcement: getDefaultAnnouncement(),
    workspacePath: "",
    agentConfig: structuredClone(defaultAgentConfig),
    patchProposals: [],
    members: createDefaultMembers(),
    messages: [
      createSystemMessage(
        t("defaults.group.welcome"),
      ),
    ],
    updatedAt: new Date().toISOString(),
  };
}

export const useSettingsStore = defineStore("settings", {
  state: () => {
    const defaultGroup = createDefaultGroup();

    return {
      providers: structuredClone(providerDefaults),
      groups: [defaultGroup] as ChatGroup[],
      memberLibrary: defaultGroup.members.map((member) => toLibraryMember(member)) as AgentModel[],
      activeGroupId: defaultGroup.id,
      ownerProfile: createDefaultOwnerProfile(),
      locale: defaultLocale as AppLocale,
      cacheDirectory: "",
      defaultCacheDirectory: "",
    };
  },

  getters: {
    activeGroup: (state) =>
      state.groups.find((group) => group.id === state.activeGroupId) ?? state.groups[0],

    activeMembers(): AgentModel[] {
      return this.activeGroup?.members.filter((member) => member.enabled) ?? [];
    },

    friends(): AgentModel[] {
      return this.memberLibrary;
    },
  },

  actions: {
    applyPersistedSettings(parsed: PersistedSettings, memberLibrary?: AgentModel[]) {
      this.cacheDirectory = parsed.cacheDirectory ?? this.cacheDirectory;
      this.locale = normalizeLocale(parsed.locale);

      if (parsed.providers) {
        this.providers = {
          ...structuredClone(providerDefaults),
          ...parsed.providers,
        };
      }

      if (parsed.ownerProfile) {
        this.ownerProfile = {
          ...createDefaultOwnerProfile(),
          ...parsed.ownerProfile,
        };
      }

      if (Array.isArray(parsed.groups) && parsed.groups.length > 0) {
        this.groups = parsed.groups.map((group) => normalizeGroup(group));
        this.memberLibrary = this.buildMemberLibrary(memberLibrary ?? parsed.memberLibrary ?? []);
        this.activeGroupId = parsed.activeGroupId ?? parsed.groups[0].id;
        return;
      }

      if (Array.isArray(parsed.agentModels) && parsed.agentModels.length > 0) {
        const migratedGroup = createDefaultGroup();
        migratedGroup.name = t("defaults.group.migratedName");
        migratedGroup.members = parsed.agentModels.map(normalizeMember);
        this.groups = [migratedGroup];
        this.memberLibrary = this.buildMemberLibrary(memberLibrary ?? parsed.agentModels);
        this.activeGroupId = migratedGroup.id;
      }
    },

    async hydrate() {
      let loadedFromCache = false;

      if (isTauri()) {
        try {
          const cacheState = await invoke<AppCacheState>("load_app_cache");
          this.defaultCacheDirectory = cacheState.defaultCacheDirectory;
          this.cacheDirectory = cacheState.cacheDirectory;

          if (cacheState.settings) {
            this.applyPersistedSettings(
              {
                ...cacheState.settings,
                cacheDirectory: cacheState.cacheDirectory,
              },
              cacheState.memberLibrary,
            );
            loadedFromCache = true;
          }
        } catch (error) {
          console.warn("Failed to load cache directory settings.", error);
        }
      }

      if (loadedFromCache) {
        return;
      }

      const raw = localStorage.getItem(STORAGE_KEY);

      if (!raw) {
        if (isTauri()) {
          await this.persist();
        }
        return;
      }

      this.applyPersistedSettings(JSON.parse(raw) as PersistedSettings);

      if (isTauri()) {
        await this.persist();
      }
    },

    buildPersistencePayload(mode: PersistenceMode): PersistedSettings {
      const messageLimit =
        mode === "normal"
          ? NORMAL_MESSAGE_LIMIT
          : mode === "compact"
            ? COMPACT_MESSAGE_LIMIT
            : 0;
      const patchLimit =
        mode === "normal"
          ? NORMAL_PATCH_LIMIT
          : mode === "compact"
            ? COMPACT_PATCH_LIMIT
            : 0;

      return {
        cacheDirectory: this.cacheDirectory,
        providers: this.providers,
        memberLibrary: this.buildMemberLibrary(this.memberLibrary),
        groups: this.groups.slice(0, MAX_PERSISTED_GROUPS).map((group) => ({
          ...group,
          patchProposals:
            mode === "minimal"
              ? []
              : group.patchProposals
                  .slice(0, patchLimit)
                  .map((proposal) => toPersistedPatchProposal(proposal, mode)),
          members: group.members.map((member) => toPlainMember(member)),
          messages:
            mode === "minimal"
              ? []
              : group.messages
                  .slice(-messageLimit)
                  .map((message) => toPersistedMessage(message, mode)),
        })),
        activeGroupId: this.activeGroupId,
        ownerProfile: this.ownerProfile,
        locale: this.locale,
      };
    },

    async persist() {
      if (isTauri()) {
        const payload = this.buildPersistencePayload("normal");

        await invoke("save_app_cache", {
          request: {
            cacheDirectory: this.cacheDirectory,
            settings: payload,
            memberLibrary: payload.memberLibrary ?? [],
          },
        });
        return;
      }

      const modes: PersistenceMode[] = ["normal", "compact", "minimal"];

      for (const mode of modes) {
        try {
          localStorage.setItem(STORAGE_KEY, JSON.stringify(this.buildPersistencePayload(mode)));
          return;
        } catch (error) {
          if (mode === "minimal") {
            try {
              localStorage.removeItem(STORAGE_KEY);
              localStorage.setItem(STORAGE_KEY, JSON.stringify(this.buildPersistencePayload(mode)));
              return;
            } catch (fallbackError) {
              console.warn("Failed to persist settings.", fallbackError);
              return;
            }
          }

          console.warn(`Failed to persist settings in ${mode} mode; retrying smaller.`, error);
        }
      }
    },

    buildMemberLibrary(seed: AgentModel[] = []) {
      const friends: AgentModel[] = [];
      const byLibraryId = new Map<string, AgentModel>();
      const byName = new Map<string, AgentModel>();

      const upsertFriend = (source: AgentModel, sourceIsGroupMember = false) => {
        const sourceLibraryId = source.libraryId ?? "";
        const sourceName = normalizeName(source.name);
        const existing =
          (sourceLibraryId ? byLibraryId.get(sourceLibraryId) : undefined) ??
          byName.get(sourceName);

        if (existing) {
          const libraryId = existing.libraryId ?? existing.id;
          source.libraryId = libraryId;

          if (sourceIsGroupMember) {
            Object.assign(source, toPlainMember(existing), {
              id: source.id,
              libraryId,
            });
          }

          return existing;
        }

        const friend = toLibraryMember(
          {
            ...source,
            libraryId: sourceLibraryId || source.id || crypto.randomUUID(),
          },
          friends,
        );

        friends.push(friend);
        byLibraryId.set(friend.libraryId ?? friend.id, friend);
        byName.set(normalizeName(friend.name), friend);

        if (sourceIsGroupMember) {
          source.libraryId = friend.libraryId;
        }

        return friend;
      };

      for (const member of seed.map(normalizeMember)) {
        upsertFriend(member);
      }

      for (const group of this.groups) {
        for (const member of group.members) {
          upsertFriend(member, true);
        }
      }

      return friends;
    },

    rememberMember(member: AgentModel) {
      const memberLibraryId = member.libraryId ?? "";
      const existingByLibraryId = memberLibraryId
        ? this.memberLibrary.find((item) => (item.libraryId ?? item.id) === memberLibraryId)
        : undefined;
      const existingByName = existingByLibraryId
        ? undefined
        : this.memberLibrary.find((item) => normalizeName(item.name) === normalizeName(member.name));
      const existing = existingByLibraryId ?? existingByName;

      if (existing) {
        const libraryId = existing.libraryId ?? existing.id;
        const name = makeUniqueMemberName(member.name, this.memberLibrary, existing.id);
        const plainMember = toPlainMember({
          ...member,
          name,
          libraryId,
        });
        Object.assign(existing, plainMember, {
          id: libraryId,
          libraryId,
          name,
        });
        member.libraryId = libraryId;
        member.name = name;
        this.syncFriendToGroups(existing);
        return existing;
      }

      const libraryId = member.libraryId ?? crypto.randomUUID();
      const libraryMember = toLibraryMember(
        {
          ...member,
          libraryId,
        },
        this.memberLibrary,
      );

      member.libraryId = libraryMember.libraryId;
      member.name = libraryMember.name;
      this.memberLibrary.push(libraryMember);
      this.syncFriendToGroups(libraryMember);
      return libraryMember;
    },

    syncFriendToGroups(friend: AgentModel) {
      const libraryId = friend.libraryId ?? friend.id;
      const plainFriend = toPlainMember({
        ...friend,
        id: libraryId,
        libraryId,
      });

      for (const group of this.groups) {
        let groupChanged = false;

        for (const member of group.members) {
          if (member.libraryId !== libraryId) {
            continue;
          }

          Object.assign(member, plainFriend, {
            id: member.id,
            libraryId,
          });
          groupChanged = true;
        }

        if (groupChanged) {
          group.updatedAt = new Date().toISOString();
        }
      }
    },

    addFriend(provider: ProviderId = "openai") {
      const providerConfig = this.providers[provider];
      const friend = createMember(
        provider,
        makeUniqueMemberName(
          t("defaults.member.draftName", { provider: providerConfig.name }),
          this.memberLibrary,
        ),
        providerConfig.defaultModel,
        t("defaults.member.defaultPrompt"),
      );
      const libraryFriend = this.rememberMember(friend);
      return libraryFriend;
    },

    renameFriend(friendId: string, name: string) {
      const friend = this.memberLibrary.find((member) => member.id === friendId);

      if (!friend) {
        return;
      }

      friend.name = makeUniqueMemberName(name, this.memberLibrary, friend.id);
      this.syncFriendToGroups(friend);
    },

    updateFriendProfile(friend: AgentModel) {
      const existing = this.memberLibrary.find((member) => member.id === friend.id);

      if (!existing) {
        return;
      }

      Object.assign(existing, toLibraryMember(friend, this.memberLibrary), {
        id: existing.id,
        libraryId: existing.libraryId ?? existing.id,
        name: makeUniqueMemberName(friend.name, this.memberLibrary, existing.id),
      });
      this.syncFriendToGroups(existing);
    },

    updateFriendProvider(friend: AgentModel) {
      friend.model = this.providers[friend.provider].defaultModel;
      friend.color = getModelColor(friend.provider);
      friend.deepSeekLongContext = friend.provider === "deepseek";
      this.updateFriendProfile(friend);
    },

    removeFriend(friendId: string) {
      const friend = this.memberLibrary.find((member) => member.id === friendId);
      const libraryId = friend?.libraryId ?? friendId;
      const isInUse = this.groups.some((group) =>
        group.members.some((member) => member.libraryId === libraryId),
      );

      if (isInUse) {
        return false;
      }

      this.memberLibrary = this.memberLibrary.filter((friend) => friend.id !== friendId);
      return true;
    },

    startPersistence() {
      this.$subscribe(
        () => {
          if (pendingPersistTimer) {
            window.clearTimeout(pendingPersistTimer);
          }

          pendingPersistTimer = window.setTimeout(() => {
            pendingPersistTimer = undefined;
            void this.persist().catch((error) => {
              console.warn("Failed to persist settings.", error);
            });
          }, PERSIST_DEBOUNCE_MS);
        },
        { detached: true },
      );
    },

    async setCacheDirectory(cacheDirectory: string) {
      this.cacheDirectory = cacheDirectory;
      await this.persist();
    },

    selectGroup(groupId: string) {
      this.activeGroupId = groupId;
    },

    createGroup(
      name: string,
      description: string,
      announcement: string,
      members: AgentModel[],
    ) {
      const group: ChatGroup = {
        id: crypto.randomUUID(),
        name,
        description,
        announcement: announcement.trim() || getDefaultAnnouncement(),
        workspacePath: "",
        agentConfig: structuredClone(defaultAgentConfig),
        patchProposals: [],
        members: members.map((member, index, list) => ({
          ...member,
          name: makeUniqueMemberName(member.name, list.slice(0, index)),
        })),
        messages: [
          createSystemMessage(
            t("defaults.group.createdMessage", { name, count: members.length }),
          ),
        ],
        updatedAt: new Date().toISOString(),
      };

      this.groups.unshift(group);
      for (const member of group.members) {
        this.rememberMember(member);
      }
      this.activeGroupId = group.id;
    },

    removeGroup(groupId: string) {
      if (this.groups.length <= 1) {
        return false;
      }

      this.groups = this.groups.filter((group) => group.id !== groupId);

      if (this.activeGroupId === groupId) {
        this.activeGroupId = this.groups[0].id;
      }

      return true;
    },

    addMember(provider: ProviderId = "openai") {
      const group = this.activeGroup;
      const providerConfig = this.providers[provider];
      const nextIndex = group.members.length + 1;
      const member = createMember(
        provider,
        makeUniqueMemberName(
          t("defaults.member.nameWithIndex", {
            provider: providerConfig.name,
            index: nextIndex,
          }),
          [...group.members, ...this.memberLibrary],
        ),
        providerConfig.defaultModel,
        t("defaults.member.defaultPrompt"),
      );

      group.members.push(member);
      this.rememberMember(member);
      group.updatedAt = new Date().toISOString();
    },

    addMemberFromFriend(friendId: string) {
      const source = this.memberLibrary.find((member) => member.id === friendId);

      if (!source) {
        return false;
      }

      const group = this.activeGroup;

      if (
        group.members.some(
          (member) =>
            member.libraryId === source.libraryId ||
            member.libraryId === source.id,
        )
      ) {
        return false;
      }

      group.members.push(cloneMember(source, group.members));
      group.updatedAt = new Date().toISOString();
      return true;
    },

    duplicateMember(member: AgentModel) {
      const group = this.activeGroup;
      const libraryId = crypto.randomUUID();
      const copy = {
        ...toPlainMember(member),
        id: crypto.randomUUID(),
        libraryId,
        name: makeUniqueMemberName(t("defaults.member.copyName", { name: member.name }), group.members),
      };

      group.members.push(copy);
      this.rememberMember(copy);
      group.updatedAt = new Date().toISOString();
    },

    renameMember(memberId: string, name: string) {
      const group = this.activeGroup;
      const member = group.members.find((item) => item.id === memberId);

      if (!member) {
        return;
      }

      member.name = makeUniqueMemberName(name, group.members, memberId);
      this.rememberMember(member);
      group.updatedAt = new Date().toISOString();
    },

    updateMemberProfile(member: AgentModel) {
      this.rememberMember(member);
      this.activeGroup.updatedAt = new Date().toISOString();
    },

    removeMember(memberId: string) {
      const group = this.activeGroup;
      group.members = group.members.filter((member) => member.id !== memberId);
      group.updatedAt = new Date().toISOString();
    },

    updateMemberProvider(member: AgentModel) {
      member.model = this.providers[member.provider].defaultModel;
      member.color = getModelColor(member.provider);
      member.deepSeekLongContext = member.provider === "deepseek";
      this.rememberMember(member);
      this.activeGroup.updatedAt = new Date().toISOString();
    },

    appendMessage(message: Omit<ChatMessage, "id" | "time">) {
      const group = this.activeGroup;

      group.messages.push({
        ...message,
        id: crypto.randomUUID(),
        time: nowText(),
        agreeMemberIds: message.agreeMemberIds ?? [],
        supplementMemberIds: message.supplementMemberIds ?? [],
        disagreeMemberIds: message.disagreeMemberIds ?? [],
        thoughtSteps: message.thoughtSteps ?? [],
        activityItems: message.activityItems ?? [],
        executionItems: message.executionItems ?? [],
      });
      group.updatedAt = new Date().toISOString();
    },

    appendPendingMessage(message: Omit<ChatMessage, "time">) {
      const group = this.activeGroup;

      group.messages.push({
        ...message,
        time: nowText(),
        agreeMemberIds: message.agreeMemberIds ?? [],
        supplementMemberIds: message.supplementMemberIds ?? [],
        disagreeMemberIds: message.disagreeMemberIds ?? [],
        thoughtSteps: message.thoughtSteps ?? [],
        activityItems: message.activityItems ?? [],
        executionItems: message.executionItems ?? [],
      });
      group.updatedAt = new Date().toISOString();
    },

    updateMessage(messageId: string, patch: Partial<ChatMessage>) {
      const message = this.activeGroup.messages.find((item) => item.id === messageId);

      if (message) {
        Object.assign(message, patch);
        this.activeGroup.updatedAt = new Date().toISOString();
      }
    },

    clearActiveGroupMessages() {
      this.activeGroup.messages = [];
      this.activeGroup.updatedAt = new Date().toISOString();
    },

    addPatchProposal(proposal: Omit<AgentPatchProposal, "id" | "createdAt" | "status">) {
      const group = this.activeGroup;

      group.patchProposals.unshift({
        ...proposal,
        id: crypto.randomUUID(),
        status: "pending",
        createdAt: nowText(),
        appliedFiles: proposal.appliedFiles ?? [],
        applyStdout: proposal.applyStdout ?? "",
        applyStderr: proposal.applyStderr ?? "",
        applyError: proposal.applyError ?? "",
      });
      group.updatedAt = new Date().toISOString();
    },

    updatePatchProposal(proposalId: string, patch: Partial<AgentPatchProposal>) {
      const group = this.activeGroup;
      if (!group) return;

      const proposal = group.patchProposals.find((item) => item.id === proposalId);

      if (proposal) {
        Object.assign(proposal, patch);
        group.updatedAt = new Date().toISOString();
      }
    },

    updatePatchProposalStatus(proposalId: string, status: PatchApprovalStatus) {
      const proposal = this.activeGroup.patchProposals.find((item) => item.id === proposalId);

      if (proposal) {
        proposal.status = status;
        this.activeGroup.updatedAt = new Date().toISOString();
      }
    },

    removePatchProposal(proposalId: string) {
      this.activeGroup.patchProposals = this.activeGroup.patchProposals.filter(
        (item) => item.id !== proposalId,
      );
      this.activeGroup.updatedAt = new Date().toISOString();
    },

    createMemberDraft(provider: ProviderId = "openai") {
      const providerConfig = this.providers[provider];

      return createMember(
        provider,
        makeUniqueMemberName(
          t("defaults.member.draftName", { provider: providerConfig.name }),
          this.memberLibrary,
        ),
        providerConfig.defaultModel,
        t("defaults.member.defaultPrompt"),
      );
    },

    cloneFriend(friendId: string, members: AgentModel[] = []) {
      const source = this.memberLibrary.find((member) => member.id === friendId);
      return source ? cloneMember(source, members) : null;
    },

  },
});
