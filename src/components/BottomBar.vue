<script setup lang="ts">
import { useRoute } from "vue-router";
import { useNavigationStore } from "@/stores/navigation";
const route = useRoute();
const nav = useNavigationStore();

/** 设置 Tab 记住上次进入的二级页（/theme、/sources） */
function targetFor(to: string): string {
  return to === "/settings" ? nav.settingsTarget(route.path) : to;
}

function isActive(to: string): boolean {
  return to === "/settings" ? nav.isSettingsPath(route.path) : route.path === to;
}

const items = [
  {
    to: "/",
    label: "搜索",
    d: "M15.5 14h-.79l-.28-.27A6.47 6.47 0 0 0 16 9.5 6.5 6.5 0 1 0 9.5 16c1.61 0 3.09-.59 4.23-1.57l.27.28v.79l5 4.99L20.49 19l-4.99-5zm-6 0C7.01 14 5 11.99 5 9.5S7.01 5 9.5 5 14 7.01 14 9.5 11.99 14 9.5 14z",
  },
  { to: "/library", label: "音乐库", d: "M12 3v10.55A4 4 0 1 0 14 17V7h4V3h-6z" },
  { to: "/tasks", label: "任务", d: "M5 20h14v-2H5v2zM19 9h-4V3H9v6H5l7 7 7-7z" },
  {
    to: "/settings",
    label: "设置",
    d: "M19.14 12.94c.04-.3.06-.61.06-.94 0-.32-.02-.64-.07-.94l2.03-1.58a.49.49 0 0 0 .12-.61l-1.92-3.32a.488.488 0 0 0-.59-.22l-2.39.96c-.5-.38-1.03-.7-1.62-.94l-.36-2.54a.484.484 0 0 0-.48-.41h-3.84c-.24 0-.43.17-.47.41l-.36 2.54c-.59.24-1.13.57-1.62.94l-2.39-.96a.484.484 0 0 0-.59.22L2.74 8.87c-.12.21-.08.47.12.61l2.03 1.58c-.05.3-.09.63-.09.94s.02.64.07.94l-2.03 1.58a.49.49 0 0 0-.12.61l1.92 3.32c.12.22.37.29.59.22l2.39-.96c.5.38 1.03.7 1.62.94l.36 2.54c.05.24.24.41.48.41h3.84c.24 0 .44-.17.47-.41l.36-2.54c.59-.24 1.13-.56 1.62-.94l2.39.96c.22.08.47 0 .59-.22l1.92-3.32c.12-.22.07-.47-.12-.61l-2.01-1.58zM12 15.6A3.6 3.6 0 1 1 12 8.4a3.6 3.6 0 0 1 0 7.2z",
  },
] as const;
</script>

<template>
  <nav class="flex justify-center px-5 pb-safe">
    <!-- 液态玻璃令牌由“全局柔光”开关统一控制 -->
    <div class="glass glass-edge flex items-center gap-1 rounded-pill p-1.5 shadow-e3">
      <RouterLink
        v-for="it in items"
        :key="it.to"
        :to="targetFor(it.to)"
        class="flex items-center gap-1.5 rounded-pill px-3.5 py-2.5 text-sm transition"
        :class="
          isActive(it.to)
            ? 'bg-primary-container text-on-primary-container'
            : 'text-on-surface-variant hover:bg-surface-high'
        "
      >
        <svg class="size-5" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
          <path :d="it.d" />
        </svg>
        <span v-if="isActive(it.to)" class="text-xs font-medium">{{ it.label }}</span>
      </RouterLink>
    </div>
  </nav>
</template>
