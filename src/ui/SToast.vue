<script setup lang="ts">
import { useToastStore } from "@/stores/toast";

/**
 * MD3 Snackbar：反色容器 + 大圆角 + e3 阴影，自动消失，点击可提前关闭。
 * 挂载在 App 的底部悬浮层里，因此会出现在迷你播放器 / 底栏上方。
 */
const toast = useToastStore();
</script>

<template>
  <Transition
    enter-active-class="transition duration-200 ease-out"
    enter-from-class="translate-y-2 opacity-0"
    enter-to-class="translate-y-0 opacity-100"
    leave-active-class="transition duration-150 ease-in"
    leave-from-class="opacity-100"
    leave-to-class="opacity-0"
  >
    <div v-if="toast.message" class="pointer-events-auto w-full px-5">
      <div
        class="glass-edge mx-auto flex w-full max-w-md items-start gap-2 rounded-card px-4 py-3 text-xs leading-relaxed shadow-e3"
        :class="
          toast.tone === 'warning'
            ? 'glass-warning text-on-warning-container'
            : 'glass-inverse text-inverse-on-surface'
        "
        role="status"
        aria-live="polite"
        @click="toast.hide()"
      >
        <span class="min-w-0 flex-1">{{ toast.message }}</span>
      </div>
    </div>
  </Transition>
</template>
