import { computed, onBeforeUnmount, ref } from "vue";

type ResizeHandle = "left" | "right";

interface ResizeSnapshot {
  handle: ResizeHandle;
  startX: number;
  leftWidth: number;
  rightWidth: number;
}

const minLeftWidth = 220;
const maxLeftWidth = 420;
const minRightWidth = 240;
const maxRightWidth = 520;

function clamp(value: number, min: number, max: number) {
  return Math.min(Math.max(value, min), max);
}

export function useResizableColumns() {
  const leftWidth = ref(300);
  const rightWidth = ref(320);
  const resizing = ref<ResizeSnapshot | null>(null);

  const layoutStyle = computed(() => ({
    "--left-column-width": `${leftWidth.value}px`,
    "--right-column-width": `${rightWidth.value}px`,
  }));

  function stopResize() {
    resizing.value = null;
    window.removeEventListener("pointermove", resize);
    window.removeEventListener("pointerup", stopResize);
  }

  function resize(event: PointerEvent) {
    const snapshot = resizing.value;

    if (!snapshot) {
      return;
    }

    const delta = event.clientX - snapshot.startX;

    if (snapshot.handle === "left") {
      leftWidth.value = clamp(snapshot.leftWidth + delta, minLeftWidth, maxLeftWidth);
      return;
    }

    rightWidth.value = clamp(snapshot.rightWidth - delta, minRightWidth, maxRightWidth);
  }

  function startResize(handle: ResizeHandle, event: PointerEvent) {
    resizing.value = {
      handle,
      startX: event.clientX,
      leftWidth: leftWidth.value,
      rightWidth: rightWidth.value,
    };
    window.addEventListener("pointermove", resize);
    window.addEventListener("pointerup", stopResize, { once: true });
  }

  function resizeWithKeyboard(handle: ResizeHandle, event: KeyboardEvent) {
    const step = event.shiftKey ? 32 : 12;
    let handled = true;

    if (handle === "left") {
      if (event.key === "ArrowLeft") {
        leftWidth.value = clamp(leftWidth.value - step, minLeftWidth, maxLeftWidth);
      } else if (event.key === "ArrowRight") {
        leftWidth.value = clamp(leftWidth.value + step, minLeftWidth, maxLeftWidth);
      } else if (event.key === "Home") {
        leftWidth.value = minLeftWidth;
      } else if (event.key === "End") {
        leftWidth.value = maxLeftWidth;
      } else {
        handled = false;
      }
    } else if (event.key === "ArrowLeft") {
      rightWidth.value = clamp(rightWidth.value + step, minRightWidth, maxRightWidth);
    } else if (event.key === "ArrowRight") {
      rightWidth.value = clamp(rightWidth.value - step, minRightWidth, maxRightWidth);
    } else if (event.key === "Home") {
      rightWidth.value = minRightWidth;
    } else if (event.key === "End") {
      rightWidth.value = maxRightWidth;
    } else {
      handled = false;
    }

    if (handled) {
      event.preventDefault();
    }
  }

  onBeforeUnmount(stopResize);

  return {
    leftWidth,
    layoutStyle,
    resizing,
    resizeWithKeyboard,
    rightWidth,
    startResize,
  };
}
