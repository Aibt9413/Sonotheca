<script setup lang="ts">
const model = defineModel<boolean>({ default: false });
</script>

<template>
  <button
    type="button"
    role="switch"
    :aria-checked="model"
    class="relative h-8 w-14 shrink-0 rounded-pill transition-colors"
    :class="model ? 'bg-primary' : 'bg-surface-highest'"
    @click="model = !model"
  >
    <span
      class="absolute top-1 size-6 rounded-pill shadow-e1 transition-all"
      :class="model ? 'left-7 bg-on-primary' : 'left-1 bg-surface-lowest'"
    />
  </button>
</template>
