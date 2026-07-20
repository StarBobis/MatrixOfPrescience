export const DEFAULT_CONTEXT_LIMIT = 128_000;
const OPENAI_GPT41_CONTEXT_LIMIT = 1_047_576;
const OPENAI_LONG_CONTEXT_LIMIT = 1_050_000;
const OPENAI_GPT5_CONTEXT_LIMIT = 400_000;
const DEEPSEEK_LONG_CONTEXT_LIMIT = 1_000_000;
const DEEPSEEK_STANDARD_CONTEXT_LIMIT = 128_000;

export interface ProviderPreset {
  id: string;
  name: string;
  baseUrl: string;
  defaultModel: string;
  models: string[];
  wireApi?: string;
  color: string;
}

export const providerPresets: ProviderPreset[] = [
  {
    id: "openai",
    name: "ChatGPT",
    baseUrl: "https://api.openai.com/v1",
    defaultModel: "gpt-4.1-mini",
    models: [
      "gpt-5.6-sol",
      "gpt-5.6-terra",
      "gpt-5.6-luna",
      "gpt-5.5",
      "gpt-5.4",
      "gpt-5",
      "gpt-4.1",
      "gpt-4.1-mini",
      "gpt-4o",
      "gpt-4o-mini",
    ],
    color: "#2f76b7",
  },
  {
    id: "deepseek",
    name: "DeepSeek",
    baseUrl: "https://api.deepseek.com",
    defaultModel: "deepseek-v4-flash",
    models: ["deepseek-v4-flash", "deepseek-v4-pro", "deepseek-chat"],
    color: "#2f7a61",
  },
];

const FALLBACK_PROVIDER_COLORS = [
  "#2f76b7",
  "#2f7a61",
  "#8a5a44",
  "#6c5ce7",
  "#b5872c",
  "#3a8f8a",
  "#a34d7e",
  "#5a6b8c",
];

export function getProviderPreset(id: string) {
  return providerPresets.find((preset) => preset.id === id);
}

export function fallbackProviderColor(id: string) {
  let hash = 0;

  for (const char of id) {
    hash = (hash * 31 + char.charCodeAt(0)) >>> 0;
  }

  return FALLBACK_PROVIDER_COLORS[hash % FALLBACK_PROVIDER_COLORS.length];
}

export function isDeepSeekProvider(provider?: { id?: string; baseUrl?: string } | null) {
  if (!provider) {
    return false;
  }

  return Boolean(
    provider.id?.toLowerCase().includes("deepseek") ||
      provider.baseUrl?.toLowerCase().includes("deepseek"),
  );
}

export function getProviderModelContextLimit(
  provider: string | { id?: string; baseUrl?: string } | undefined,
  modelName: string,
  options: { deepSeekLike?: boolean; deepSeekLongContext?: boolean } = {},
) {
  const deepSeekLike =
    options.deepSeekLike ??
    (typeof provider === "string"
      ? provider.toLowerCase().includes("deepseek")
      : isDeepSeekProvider(provider));

  if (deepSeekLike) {
    return options.deepSeekLongContext ?? true
      ? DEEPSEEK_LONG_CONTEXT_LIMIT
      : DEEPSEEK_STANDARD_CONTEXT_LIMIT;
  }

  return getOpenAIModelContextLimit(modelName);
}

function getOpenAIModelContextLimit(modelName: string) {
  const model = modelName.trim().toLowerCase();

  if (!model) {
    return DEFAULT_CONTEXT_LIMIT;
  }

  if (model.includes("gpt-5.6") || model.includes("gpt-5.5")) {
    return OPENAI_LONG_CONTEXT_LIMIT;
  }

  if (model.includes("gpt-5.4-mini") || model.includes("gpt-5.4-nano")) {
    return OPENAI_GPT5_CONTEXT_LIMIT;
  }

  if (model.includes("gpt-5.4")) {
    return OPENAI_LONG_CONTEXT_LIMIT;
  }

  if (model.includes("gpt-4.1")) {
    return OPENAI_GPT41_CONTEXT_LIMIT;
  }

  if (model.includes("gpt-5")) {
    return OPENAI_GPT5_CONTEXT_LIMIT;
  }

  return DEFAULT_CONTEXT_LIMIT;
}
