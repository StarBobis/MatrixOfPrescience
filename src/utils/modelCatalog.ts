import type { ProviderId } from "../stores/settings";

export const DEFAULT_CONTEXT_LIMIT = 128_000;
const OPENAI_GPT41_CONTEXT_LIMIT = 1_047_576;
const OPENAI_LONG_CONTEXT_LIMIT = 1_050_000;
const OPENAI_GPT5_CONTEXT_LIMIT = 400_000;
const DEEPSEEK_LONG_CONTEXT_LIMIT = 1_000_000;
const DEEPSEEK_STANDARD_CONTEXT_LIMIT = 128_000;

export const modelPresets: Record<ProviderId, string[]> = {
  openai: [
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
  deepseek: ["deepseek-v4-flash", "deepseek-v4-pro", "deepseek-chat"],
};

export function getProviderModelContextLimit(
  provider: ProviderId,
  modelName: string,
  options: { deepSeekLongContext?: boolean } = {},
) {
  if (provider === "deepseek") {
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
