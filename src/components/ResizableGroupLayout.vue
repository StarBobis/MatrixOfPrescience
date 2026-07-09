<script setup lang="ts">
import { useResizableColumns } from "../composables/useResizableColumns";

const { layoutStyle, resizing, startResize } = useResizableColumns();
</script>

<template>
  <div class="resizable-group-layout" :class="{ resizing: Boolean(resizing) }" :style="layoutStyle">
    <div class="layout-pane left-pane">
      <slot name="left" />
    </div>

    <button
      class="layout-resizer"
      type="button"
      aria-label="调整左侧区域宽度"
      @pointerdown.prevent="startResize('left', $event)"
    ></button>

    <div class="layout-pane main-pane">
      <slot name="main" />
    </div>

    <button
      class="layout-resizer"
      type="button"
      aria-label="调整右侧区域宽度"
      @pointerdown.prevent="startResize('right', $event)"
    ></button>

    <div class="layout-pane right-pane">
      <slot name="right" />
    </div>
  </div>
</template>
