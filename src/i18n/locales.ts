export const defaultLocale = "en";

export const supportedLocales = ["en", "zh-CN"] as const;

export type AppLocale = (typeof supportedLocales)[number];

export function normalizeLocale(value: unknown): AppLocale {
  return supportedLocales.includes(value as AppLocale) ? (value as AppLocale) : defaultLocale;
}
