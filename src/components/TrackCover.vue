<script lang="ts">
// 模块级缓存：同一曲目只读一次内嵌封面
const coverCache = new Map<number, string | null>();
</script>

<script setup lang="ts">
import { onMounted, onUnmounted, ref } from "vue";
import { fileDetails } from "@/ipc/bindings";

const props = defineProps<{
  track: {
    id: number;
    relPath: string;
    format: string | null;
  };
}>();

const cover = ref<string | null>(coverCache.get(props.track.id) ?? null);
const loaded = ref(coverCache.has(props.track.id));
let observer: IntersectionObserver | null = null;
let disposed = false;

async function loadCover() {
  if (loaded.value || disposed) return;
  loaded.value = true;
  try {
    const info = await fileDetails("app", props.track.relPath);
    cover.value = info.cover;
    coverCache.set(props.track.id, info.cover);
  } catch {
    coverCache.set(props.track.id, null);
  }
}

onMounted(() => {
  if (loaded.value) return;

  if (typeof IntersectionObserver === "undefined") {
    void loadCover();
    return;
  }

  observer = new IntersectionObserver(
    (entries) => {
      if (entries.some((entry) => entry.isIntersecting)) {
        observer?.disconnect();
        observer = null;
        void loadCover();
      }
    },
    { rootMargin: "160px" },
  );
  const element = document.querySelector<HTMLElement>(`[data-cover-id="${props.track.id}"]`);
  if (element) observer.observe(element);
  else void loadCover();
});

onUnmounted(() => {
  disposed = true;
  observer?.disconnect();
  observer = null;
});
</script>

<template>
  <div
    :data-cover-id="track.id"
    class="grid size-11 shrink-0 place-items-center overflow-hidden rounded-pill bg-surface-highest text-on-surface-variant"
  >
    <img
      v-if="cover"
      :src="cover"
      alt=""
      class="size-full object-cover"
    />
    <span v-else class="text-[10px] font-medium uppercase">{{ track.format ?? "?" }}</span>
  </div>
</template>
