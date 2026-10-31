<script setup lang="ts">
/**
 * 全站唯一的卡片出口。
 * tone 仍保留三级层级，但底色统一走液态玻璃令牌：
 * 浅色是半透明白、深色是半透明黑，由「全局柔光」开关统一控制。
 */
withDefaults(
  defineProps<{
    tone?: "low" | "base" | "high";
    elevated?: boolean;
    interactive?: boolean;
  }>(),
  { tone: "base", elevated: false, interactive: false },
);
</script>

<template>
  <div
    class="rounded-card p-4 transition-all duration-200 ease-[cubic-bezier(0.2,0,0,1)] active:scale-95"
    :class="[
      tone === 'low' ? 'glass-card-low' : tone === 'high' ? 'glass-card-high' : 'glass-card',
      elevated ? 'shadow-e2' : 'shadow-e1',
      interactive ? 'cursor-pointer' : '',
    ]"
  >
    <slot />
  </div>
</template>
