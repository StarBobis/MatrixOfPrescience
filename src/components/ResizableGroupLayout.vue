<script setup lang="ts">
import { useI18n } from "vue-i18n";
import { useResizableColumns } from "../composables/useResizableColumns";

const { layoutStyle, resizing, startResize } = useResizableColumns();
const { t } = useI18n();
</script>

<template>
  <div class="resizable-group-layout" :class="{ resizing: Boolean(resizing) }" :style="layoutStyle">
    <div class="layout-pane left-pane">
      <slot name="left" />
    </div>

    <button
      class="layout-resizer"
      type="button"
      :aria-label="t('resizable.left')"
      @pointerdown.prevent="startResize('left', $event)"
    ></button>

    <div class="layout-pane main-pane">
      <slot name="main" />
    </div>

    <button
      class="layout-resizer"
      type="button"
      :aria-label="t('resizable.right')"
      @pointerdown.prevent="startResize('right', $event)"
    ></button>

    <div class="layout-pane right-pane">
      <slot name="right" />
    </div>
  </div>
</template>
