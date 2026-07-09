import { watch } from "vue";
import { createI18n } from "vue-i18n";
import { defaultLocale, normalizeLocale, type AppLocale } from "./locales";
import en from "./locales/en.json";
import zhCN from "./locales/zh-CN.json";

type SettingsWithLocale = {
  locale: AppLocale;
};

type MessageSchema = typeof en;

export const i18n = createI18n<[MessageSchema], AppLocale>({
  legacy: false,
  locale: defaultLocale,
  fallbackLocale: defaultLocale,
  messages: {
    en,
    "zh-CN": zhCN,
  },
});

export function setI18nLocale(locale?: unknown): AppLocale {
  const normalized = normalizeLocale(locale);
  const globalLocale = i18n.global.locale as unknown;

  if (typeof globalLocale === "string") {
    (i18n.global as unknown as { locale: AppLocale }).locale = normalized;
  } else {
    (globalLocale as { value: AppLocale }).value = normalized;
  }

  document.documentElement.lang = normalized;
  return normalized;
}

export function getI18nLocale(): AppLocale {
  const globalLocale = i18n.global.locale as unknown;

  return normalizeLocale(
    typeof globalLocale === "string" ? globalLocale : (globalLocale as { value?: unknown }).value,
  );
}

export function bindI18nLocaleToSettings(settings: SettingsWithLocale): void {
  watch(
    () => settings.locale,
    (nextLocale) => {
      const normalized = setI18nLocale(nextLocale);

      if (settings.locale !== normalized) {
        settings.locale = normalized;
      }
    },
    { flush: "sync", immediate: true },
  );
}

export function translate(key: string, named?: Record<string, unknown>): string {
  return i18n.global.t(key as never, named ?? {});
}
