<script setup lang="ts">
import { onActivated, onMounted, ref, watch } from "vue";
import { useRouter } from "vue-router";
import SCard from "@/ui/SCard.vue";
import { usePlaylistStore } from "@/stores/playlist";
import {
  describeError,
  myPlaylists,
  type MyPlaylistItem,
  type SourceId,
} from "@/ipc/bindings";

defineOptions({ name: "MyPlaylistsView" });

const router = useRouter();
const playlist = usePlaylistStore();

const source = ref<SourceId>("netease");
const items = ref<MyPlaylistItem[]>([]);
const loading = ref(false);
const error = ref<string | null>(null);

const sources: ReadonlyArray<{ key: SourceId; label: string }> = [
  { key: "netease", label: "网易云" },
  { key: "qq", label: "QQ音乐" },
  { key: "bilibili", label: "B站" },
];

async function load() {
  loading.value = true;
  error.value = null;
  items.value = [];
  try {
    items.value = await myPlaylists(source.value);
  } catch (e) {
    error.value = describeError(e);
  } finally {
    loading.value = false;
  }
}

async function open(item: MyPlaylistItem) {
  playlist.openRecent({
    source: item.source,
    id: item.id,
    name: item.name,
    coverUrl: item.coverUrl,
    total: item.total,
  });
  await router.push("/playlist");
}

watch(source, () => {
  void load();
});

onMounted(() => {
  void load();
});

onActivated(() => {
  void load();
});
</script>

<template>
  <div class="flex w-full flex-col gap-3">
    <div class="grid grid-cols-3 gap-1 rounded-pill bg-surface-high p-1">
      <button
        v-for="item in sources"
        :key="item.key"
        type="button"
        class="rounded-pill py-2 text-[11px] font-medium transition"
        :class="
          source === item.key
            ? 'bg-primary text-on-primary shadow-e1'
            : 'text-on-surface-variant'
        "
        @click="source = item.key"
      >
        {{ item.label }}
      </button>
    </div>

    <p v-if="loading" class="px-1 text-xs text-on-surface-variant">加载中…</p>
    <p v-else-if="error" class="px-1 text-xs text-error">{{ error }}</p>
    <p v-else-if="items.length === 0" class="px-1 py-8 text-center text-xs text-on-surface-variant">
      还没有歌单
    </p>

    <div v-else class="grid grid-cols-2 gap-2">
      <SCard
        v-for="item in items"
        :key="`${item.source}:${item.id}`"
        interactive
        class="flex items-center gap-2 p-2.5"
        @click="open(item)"
      >
        <img
          v-if="item.coverUrl"
          :src="item.coverUrl"
          alt=""
          referrerpolicy="no-referrer"
          class="size-11 shrink-0 rounded-card object-cover"
        />
        <div
          v-else
          class="grid size-11 shrink-0 place-items-center rounded-card bg-surface-highest text-sm text-on-surface-variant"
        >
          ♫
        </div>
        <div class="min-w-0 flex-1">
          <p class="truncate text-[11px] font-medium">{{ item.name }}</p>
          <p class="text-[9px] text-on-surface-variant">{{ item.total }} 首</p>
        </div>
      </SCard>
    </div>
  </div>
</template>
