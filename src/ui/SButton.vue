<script setup lang="ts">
withDefaults(
  defineProps<{
    variant?: "filled" | "tonal" | "outlined" | "text";
    disabled?: boolean;
    type?: "button" | "submit";
  }>(),
  { variant: "filled", disabled: false, type: "button" },
);
</script>

<template>
  <button
    :type="type"
    :disabled="disabled"
    class="inline-flex items-center justify-center gap-2 rounded-pill px-5 py-2.5 text-sm font-medium transition active:scale-95 disabled:pointer-events-none disabled:opacity-40"
    :class="[
      variant === 'filled' ? 'bg-primary text-on-primary shadow-e1' : '',
      variant === 'tonal' ? 'bg-primary-container text-on-primary-container' : '',
      variant === 'outlined' ? 'border border-outline-variant text-on-surface' : '',
      variant === 'text' ? 'text-primary hover:bg-surface-high' : '',
    ]"
  >
    <slot />
  </button>
</template>
