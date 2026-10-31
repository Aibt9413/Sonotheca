<script setup lang="ts">
import logo from "@/assets/logo.webp";

defineOptions({ name: "AboutView" });
</script>

<template>
  <div class="flex min-h-[70vh] flex-col items-center justify-center gap-4">
    <img :src="logo" alt="Sonotheca" class="size-28 rounded-[2rem] shadow-e2" />
    <p class="text-lg font-semibold tracking-tight">Sonotheca</p>
    <p class="font-mono text-xs text-on-surface-variant">1.0.19</p>
  </div>
</template>
