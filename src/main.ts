import { createApp } from "vue";
import { createPinia } from "pinia";
import ElementPlus from "element-plus";
import "element-plus/dist/index.css";
import App from "./App.vue";
import { bindI18nLocaleToSettings, i18n } from "./i18n";
import { useSettingsStore } from "./stores/settings";

const pinia = createPinia();
const app = createApp(App);

app.use(pinia);

const settingsStore = useSettingsStore();

bindI18nLocaleToSettings(settingsStore);

async function bootstrap() {
  try {
    await settingsStore.hydrate();
  } catch (error) {
    console.warn("Failed to hydrate settings; defaults will be used.", error);
  }

  settingsStore.startPersistence();

  app.use(i18n).use(ElementPlus).mount("#app");
}

void bootstrap();
