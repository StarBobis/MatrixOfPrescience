<script setup lang="ts">
import { nextTick, ref } from "vue";
import { PanelLeft, PanelRight } from "@lucide/vue";
import { useI18n } from "vue-i18n";
import { useResizableColumns } from "../composables/useResizableColumns";

const { leftWidth, layoutStyle, resizing, resizeWithKeyboard, rightWidth, startResize } =
  useResizableColumns();
const { t } = useI18n();
const compactPane = ref<"left" | "right" | null>(null);
const leftPaneButton = ref<HTMLButtonElement | null>(null);
const rightPaneButton = ref<HTMLButtonElement | null>(null);

function focusCompactPane(pane: "left" | "right") {
  void nextTick(() => {
    const panel = document.getElementById(`group-${pane}-pane`);
    panel
      ?.querySelector<HTMLElement>(
        ".group-card.active, button, input, textarea, select, [tabindex='0']",
      )
      ?.focus();
  });
}

function closeCompactPane() {
  const pane = compactPane.value;
  if (!pane) {
    return;
  }

  compactPane.value = null;
  void nextTick(() =>
    (pane === "left" ? leftPaneButton.value : rightPaneButton.value)?.focus(),
  );
}

function toggleCompactPane(pane: "left" | "right") {
  if (compactPane.value === pane) {
    closeCompactPane();
    return;
  }

  compactPane.value = pane;
  focusCompactPane(pane);
}

function handleCompactPaneEscape(event: KeyboardEvent) {
  if (event.defaultPrevented || !compactPane.value) {
    return;
  }

  closeCompactPane();
}

function handleLeftPaneClick(event: MouseEvent) {
  if (event.target instanceof Element && event.target.closest(".group-card")) {
    closeCompactPane();
  }
}
</script>

<template>
  <div
    class="resizable-group-layout"
    :class="{ resizing: Boolean(resizing) }"
    :style="layoutStyle"
    @keydown.esc="handleCompactPaneEscape"
  >
    <div class="compact-pane-controls" role="toolbar" :aria-label="t('resizable.controlsLabel')">
      <button
        ref="leftPaneButton"
        class="compact-pane-button"
        :class="{ active: compactPane === 'left' }"
        type="button"
        aria-controls="group-left-pane"
        :aria-expanded="compactPane === 'left'"
        :title="t('resizable.showGroups')"
        :aria-label="t('resizable.showGroups')"
        @click="toggleCompactPane('left')"
      >
        <PanelLeft aria-hidden="true" />
      </button>
      <button
        ref="rightPaneButton"
        class="compact-pane-button"
        :class="{ active: compactPane === 'right' }"
        type="button"
        aria-controls="group-right-pane"
        :aria-expanded="compactPane === 'right'"
        :title="t('resizable.showInspector')"
        :aria-label="t('resizable.showInspector')"
        @click="toggleCompactPane('right')"
      >
        <PanelRight aria-hidden="true" />
      </button>
    </div>

    <button
      v-if="compactPane"
      class="compact-pane-backdrop"
      type="button"
      :aria-label="t('resizable.closePanels')"
      @click="closeCompactPane"
    ></button>

    <div
      id="group-left-pane"
      class="layout-pane left-pane"
      :class="{ 'compact-open': compactPane === 'left' }"
      @click="handleLeftPaneClick"
    >
      <slot name="left" />
    </div>

    <button
      class="layout-resizer"
      type="button"
      role="separator"
      aria-orientation="vertical"
      aria-controls="group-left-pane"
      :aria-valuemin="220"
      :aria-valuemax="420"
      :aria-valuenow="leftWidth"
      :aria-valuetext="`${leftWidth}px`"
      :aria-label="t('resizable.left')"
      @pointerdown.prevent="startResize('left', $event)"
      @keydown="resizeWithKeyboard('left', $event)"
    >
      <span class="layout-resizer-grip" aria-hidden="true"></span>
    </button>

    <div
      id="group-main-pane"
      class="layout-pane main-pane"
      :inert="compactPane ? true : undefined"
      :aria-hidden="compactPane ? 'true' : undefined"
    >
      <slot name="main" />
    </div>

    <button
      class="layout-resizer"
      type="button"
      role="separator"
      aria-orientation="vertical"
      aria-controls="group-right-pane"
      :aria-valuemin="240"
      :aria-valuemax="520"
      :aria-valuenow="rightWidth"
      :aria-valuetext="`${rightWidth}px`"
      :aria-label="t('resizable.right')"
      @pointerdown.prevent="startResize('right', $event)"
      @keydown="resizeWithKeyboard('right', $event)"
    >
      <span class="layout-resizer-grip" aria-hidden="true"></span>
    </button>

    <div
      id="group-right-pane"
      class="layout-pane right-pane"
      :class="{ 'compact-open': compactPane === 'right' }"
    >
      <slot name="right" />
    </div>
  </div>
</template>
