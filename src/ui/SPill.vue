<script setup lang="ts">
withDefaults(
  defineProps<{
    active?: boolean;
    disabled?: boolean;
  }>(),
  { active: false, disabled: false },
);
</script>

<template>
  <button
    type="button"
    :disabled="disabled"
    class="rounded-pill px-4 py-2 text-xs font-medium transition disabled:pointer-events-none disabled:opacity-40"
    :class="active ? 'bg-primary-container text-on-primary-container shadow-e1' : 'bg-surface-high text-on-surface-variant'"
  >
    <slot />
  </button>
</template>
