import { defineStore } from "pinia";

export type ProviderId = "openai" | "deepseek";
export type ChatRole = "user" | "assistant";
export type MessageStatus = "done" | "thinking" | "error";
export type AgentMode = "chat" | "local-agent" | "architect";
export type AgentWorkflowMode = "ask" | "edit-before-ask" | "code" | "yolo";
export type AgentApprovalMode = "manual" | "confirm-risky" | "auto";
export type AgentSafetyModel = "strict" | "balanced" | "security-analyzer" | "sandbox-yolo";
export type PatchRiskLevel = "low" | "medium" | "high";
export type PatchApprovalStatus = "pending" | "approved" | "rejected" | "discarded";
export type PatchSafetyVerdict = "allow" | "needs-confirmation" | "blocked";

export interface ProviderConfig {
  id: ProviderId;
  name: string;
  baseUrl: string;
  apiKey: string;
  defaultModel: string;
}

export interface AgentModel {
  id: string;
  name: string;
  provider: ProviderId;
  model: string;
  systemPrompt: string;
  temperature: number;
  enabled: boolean;
  color: string;
  avatar?: string;
}

export interface ChatMessage {
  id: string;
  role: ChatRole;
  modelName: string;
  providerName?: string;
  status: MessageStatus;
  content: string;
  time: string;
  color: string;
  agreeMemberIds?: string[];
  disagreeMemberIds?: string[];
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
}

export interface PatchSafetyCheck {
  verdict: PatchSafetyVerdict;
  reasons: string[];
  warnings: string[];
}

interface PersistedSettings {
  providers?: Partial<Record<ProviderId, ProviderConfig>>;
  agentModels?: AgentModel[];
  groups?: ChatGroup[];
  activeGroupId?: string;
  ownerProfile?: Partial<OwnerProfile>;
}

const STORAGE_KEY = "matrix-of-prescience-settings";

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

const defaultAnnouncement =
  "群公告：所有群友需要基于事实、清晰表达；先说明判断，再给出可执行建议。不同群友可以保留分歧，但必须指出依据。";

const defaultOwnerProfile: OwnerProfile = {
  name: "我",
  avatar: "",
  color: "#4d5a61",
};

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
    announcement: group.announcement ?? defaultAnnouncement,
    workspacePath,
    agentConfig: normalizeAgentConfig(group.agentConfig),
    patchProposals: (group.patchProposals ?? []).map((proposal) => ({
      ...proposal,
      workspacePath: proposal.workspacePath ?? workspacePath,
      safetyCheck: proposal.safetyCheck ?? {
        verdict: proposal.riskLevel === "high" ? "blocked" : "needs-confirmation",
        reasons:
          proposal.riskLevel === "high"
            ? ["旧提案缺少安全校验结果，已按高风险阻止。"]
            : [],
        warnings: ["旧提案缺少安全校验结果，需要重新人工复核。"],
      },
    })),
    messages: group.messages.map((message) => ({
      ...message,
      agreeMemberIds: message.agreeMemberIds ?? [],
      disagreeMemberIds: message.disagreeMemberIds ?? [],
    })),
  };
}

function nowText() {
  return new Intl.DateTimeFormat("zh-CN", {
    hour: "2-digit",
    minute: "2-digit",
  }).format(new Date());
}

function createSystemMessage(content: string): ChatMessage {
  return {
    id: crypto.randomUUID(),
    role: "assistant",
    modelName: "系统",
    status: "done",
    content,
    time: nowText(),
    color: "#6c6f75",
    agreeMemberIds: [],
    disagreeMemberIds: [],
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
    name,
    provider,
    model,
    systemPrompt,
    temperature: 0.7,
    enabled: true,
    color: getModelColor(provider),
  };
}

function createDefaultMembers(): AgentModel[] {
  return [
    createMember(
      "openai",
      "产品经理",
      "gpt-4.1-mini",
      "你是这个群里的产品经理，负责澄清目标、拆解需求和约束范围。",
    ),
    createMember(
      "deepseek",
      "技术顾问",
      "deepseek-v4-flash",
      "你是这个群里的技术顾问，负责分析实现路径、风险和工程细节。",
    ),
  ];
}

function createDefaultGroup(): ChatGroup {
  return {
    id: crypto.randomUUID(),
    name: "默认 Agent 群",
    description: "多模型协作讨论群",
    announcement: defaultAnnouncement,
    workspacePath: "",
    agentConfig: structuredClone(defaultAgentConfig),
    patchProposals: [],
    members: createDefaultMembers(),
    messages: [
      createSystemMessage(
        "这是一个 Agent 聊天群。每个虚拟群友都有自己的 API、模型和核心角色；你发出一条消息后，未禁言的群友会共享同一个上下文并分别回复。",
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
      activeGroupId: defaultGroup.id,
      ownerProfile: structuredClone(defaultOwnerProfile),
    };
  },

  getters: {
    activeGroup: (state) =>
      state.groups.find((group) => group.id === state.activeGroupId) ?? state.groups[0],

    activeMembers(): AgentModel[] {
      return this.activeGroup?.members.filter((member) => member.enabled) ?? [];
    },
  },

  actions: {
    hydrate() {
      const raw = localStorage.getItem(STORAGE_KEY);

      if (!raw) {
        return;
      }

      const parsed = JSON.parse(raw) as PersistedSettings;

      if (parsed.providers) {
        this.providers = {
          ...structuredClone(providerDefaults),
          ...parsed.providers,
        };
      }

      if (parsed.ownerProfile) {
        this.ownerProfile = {
          ...structuredClone(defaultOwnerProfile),
          ...parsed.ownerProfile,
        };
      }

      if (Array.isArray(parsed.groups) && parsed.groups.length > 0) {
        this.groups = parsed.groups.map((group) => normalizeGroup(group));
        this.activeGroupId = parsed.activeGroupId ?? parsed.groups[0].id;
        return;
      }

      if (Array.isArray(parsed.agentModels) && parsed.agentModels.length > 0) {
        const migratedGroup = createDefaultGroup();
        migratedGroup.name = "迁移的模型群";
        migratedGroup.members = parsed.agentModels;
        this.groups = [migratedGroup];
        this.activeGroupId = migratedGroup.id;
      }
    },

    persist() {
      localStorage.setItem(
        STORAGE_KEY,
        JSON.stringify({
          providers: this.providers,
          groups: this.groups,
          activeGroupId: this.activeGroupId,
          ownerProfile: this.ownerProfile,
        }),
      );
    },

    startPersistence() {
      this.$subscribe(
        () => {
          this.persist();
        },
        { detached: true },
      );
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
        announcement: announcement.trim() || defaultAnnouncement,
        workspacePath: "",
        agentConfig: structuredClone(defaultAgentConfig),
        patchProposals: [],
        members,
        messages: [
          createSystemMessage(
            `群「${name}」已创建。当前有 ${members.length} 个虚拟群友，可以开始对话。`,
          ),
        ],
        updatedAt: new Date().toISOString(),
      };

      this.groups.unshift(group);
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

      group.members.push(
        createMember(
          provider,
          `${providerConfig.name} 群友 ${nextIndex}`,
          providerConfig.defaultModel,
          "你是这个群里的虚拟群友，请基于自己的核心角色独立回答用户问题。",
        ),
      );
      group.updatedAt = new Date().toISOString();
    },

    duplicateMember(member: AgentModel) {
      const group = this.activeGroup;

      group.members.push({
        ...member,
        id: crypto.randomUUID(),
        name: `${member.name} 副本`,
      });
      group.updatedAt = new Date().toISOString();
    },

    removeMember(memberId: string) {
      const group = this.activeGroup;
      group.members = group.members.filter((member) => member.id !== memberId);
      group.updatedAt = new Date().toISOString();
    },

    updateMemberProvider(member: AgentModel) {
      member.model = this.providers[member.provider].defaultModel;
      member.color = getModelColor(member.provider);
      this.activeGroup.updatedAt = new Date().toISOString();
    },

    appendMessage(message: Omit<ChatMessage, "id" | "time">) {
      const group = this.activeGroup;

      group.messages.push({
        ...message,
        id: crypto.randomUUID(),
        time: nowText(),
        agreeMemberIds: message.agreeMemberIds ?? [],
        disagreeMemberIds: message.disagreeMemberIds ?? [],
      });
      group.updatedAt = new Date().toISOString();
    },

    appendPendingMessage(message: Omit<ChatMessage, "time">) {
      const group = this.activeGroup;

      group.messages.push({
        ...message,
        time: nowText(),
        agreeMemberIds: message.agreeMemberIds ?? [],
        disagreeMemberIds: message.disagreeMemberIds ?? [],
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

    addPatchProposal(proposal: Omit<AgentPatchProposal, "id" | "createdAt" | "status">) {
      const group = this.activeGroup;

      group.patchProposals.unshift({
        ...proposal,
        id: crypto.randomUUID(),
        status: "pending",
        createdAt: nowText(),
      });
      group.updatedAt = new Date().toISOString();
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
        `${providerConfig.name} 群友`,
        providerConfig.defaultModel,
        "你是这个群里的虚拟群友，请基于自己的核心角色独立回答用户问题。",
      );
    },
  },
});
