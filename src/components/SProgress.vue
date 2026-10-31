<script setup lang="ts">
withDefaults(
  defineProps<{
    /** 0-100；null 表示未知总量 */
    value: number | null;
    indeterminate?: boolean;
  }>(),
  { value: null, indeterminate: false },
);
</script>

<template>
  <div class="h-2 w-full overflow-hidden rounded-pill bg-surface-highest" role="progressbar" :aria-valuenow="value ?? 0">
    <div v-if="indeterminate" class="h-full w-1/3 animate-pulse rounded-pill bg-primary" />
    <div
      v-else
      class="h-full rounded-pill bg-primary transition-[width] duration-300"
      :style="{ width: `${Math.min(100, Math.max(0, value ?? 0))}%` }"
    />
  </div>
</template>
