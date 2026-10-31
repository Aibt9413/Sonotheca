<script setup lang="ts">
import { computed, onUnmounted, watch } from "vue";
import type { ConcreteQuality, NeteaseQuality } from "@/ipc/bindings";
import { useUiStore } from "@/stores/ui";

/**
 * 音质选择 Bottom Sheet。
 *
 * 「默认」不直接落库：调用方拿到 `default` 后解析成当前设置里的具体档位，
 * 这样设置页一改，弹窗首项文案自然跟着变（需求见交接单 6.4）。
 */
const props = defineProps<{ open: boolean; defaultQuality: ConcreteQuality }>();
const emit = defineEmits<{ close: []; pick: [quality: NeteaseQuality] }>();
const ui = useUiStore();

// 弹层打开时隐藏全局 Tab 栏，避免底栏盖住「取消」按钮
watch(() => props.open, (open) => ui.setSheetOpen(open), { immediate: true });
onUnmounted(() => ui.setSheetOpen(false));

const LABELS: Record<ConcreteQuality, string> = {
  standard: "标准 128K",
  exhigh: "极高 320K",
  lossless: "无损",
  hires: "Hi-Res",
  jymaster: "超清母带",
  sky: "沉浸声",
};

const options: ReadonlyArray<{ value: ConcreteQuality; label: string; hint: string }> = [
  { value: "jymaster", label: "超清母带", hint: "SVIP 专享" },
  { value: "sky", label: "沉浸声", hint: "SVIP · 空间音频" },
  { value: "hires", label: "Hi-Res", hint: "SVIP 专享" },
  { value: "lossless", label: "无损", hint: "FLAC" },
  { value: "exhigh", label: "极高", hint: "320K" },
  { value: "standard", label: "标准", hint: "128K" },
];

const defaultLabel = computed(() => LABELS[props.defaultQuality]);

function pick(value: NeteaseQuality) {
  emit("pick", value);
  emit("close");
}
</script>

<template>
  <div v-if="open" class="fixed inset-0 z-50 flex items-end bg-black/50" @click.self="emit('close')">
    <div class="glass-sheet max-h-[85vh] w-full overflow-y-auto rounded-t-card p-5 pb-safe shadow-e3">
      <div class="mx-auto mb-3 h-1 w-10 rounded-pill bg-outline-variant" />

      <p class="text-sm font-medium">选择下载音质</p>
      <p class="mt-0.5 text-[11px] text-on-surface-variant">SVIP 档位需要登录；拿不到会自动降级</p>

      <div class="mt-4 flex flex-col gap-2">
        <!-- 跟随设置 -->
        <button
          type="button"
          class="flex items-center justify-between gap-3 rounded-card bg-surface-low px-3.5 py-3 text-left transition active:scale-95"
          @click="pick('default')"
        >
          <span class="min-w-0 flex-1 truncate text-sm">
            默认（跟随设置：{{ defaultLabel }}）
          </span>
          <span
            class="shrink-0 rounded-pill bg-surface-highest px-2 py-0.5 text-[10px] text-on-surface-variant"
          >
            推荐
          </span>
        </button>

        <button
          v-for="item in options"
          :key="item.value"
          type="button"
          class="flex items-center justify-between gap-3 rounded-card px-3.5 py-3 text-left transition active:scale-95"
          :class="
            item.value === defaultQuality
              ? 'bg-primary-container text-on-primary-container'
              : 'bg-surface-low'
          "
          @click="pick(item.value)"
        >
          <span class="min-w-0 flex-1 truncate text-sm">{{ item.label }}</span>
          <span
            class="shrink-0 text-[10px]"
            :class="item.value === defaultQuality ? 'opacity-80' : 'text-on-surface-variant'"
          >
            {{ item.hint }}
          </span>
        </button>
      </div>

      <button
        type="button"
        class="mt-4 w-full rounded-pill bg-surface-high py-2.5 text-xs text-on-surface-variant"
        @click="emit('close')"
      >
        取消
      </button>
    </div>
  </div>
</template>
