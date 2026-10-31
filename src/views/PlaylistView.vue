<script setup lang="ts">
import {
  computed,
  onActivated,
  onDeactivated,
  onMounted,
  onUnmounted,
  ref,
  watch,
} from "vue";
import { useRouter } from "vue-router";
import SCard from "@/ui/SCard.vue";
import { useBackGuard } from "@/composables/useBackGuard";
import { usePlaylistStore } from "@/stores/playlist";
import { useSettingsStore } from "@/stores/settings";
import { useTasksStore } from "@/stores/tasks";
import { useToastStore } from "@/stores/toast";
import { type PlaylistTrack, getDownloadSettings } from "@/ipc/bindings";
import { formatDuration } from "@/utils/format";

defineOptions({ name: "PlaylistView" });

const router = useRouter();
const playlist = usePlaylistStore();
const tasks = useTasksStore();
const prefs = useSettingsStore();
const toast = useToastStore();

useBackGuard(
  () => true,
  () => void router.push("/"),
);

const searchInput = ref("");
const selected = ref<Set<string>>(new Set());
const downloading = ref(false);
const downloadDone = ref(0);
const downloadTotal = ref(0);

const visibleTracks = computed(() => playlist.filtered);
const selectedCount = computed(() => selected.value.size);
const allVisibleSelected = computed(
  () =>
    visibleTracks.value.length > 0 &&
    visibleTracks.value.every((track) => selected.value.has(track.id)),
);

// ── 滚动加载：距底 200px + 300ms 防抖 ──────────────────────
let scrollHost: HTMLElement | null = null;
let scrollTimer: ReturnType<typeof setTimeout> | null = null;
let attached = false;

function clearScrollTimer() {
  if (scrollTimer) {
    clearTimeout(scrollTimer);
    scrollTimer = null;
  }
}

function onScroll() {
  if (playlist.query.trim() || !scrollHost) return;

  const host = scrollHost;
  const nearBottom =
    host.scrollHeight - host.scrollTop - host.clientHeight < 200;
  if (!nearBottom) {
    clearScrollTimer();
    return;
  }

  if (scrollTimer || playlist.loading || playlist.loadingMore || !playlist.hasMore) {
    return;
  }

  // 滑到底部先停 300ms：快速滑动不会连发几十个请求
  scrollTimer = setTimeout(async () => {
    scrollTimer = null;
    if (playlist.query.trim()) return;
    const current = scrollHost;
    if (!current) return;
    if (current.scrollHeight - current.scrollTop - current.clientHeight >= 200) return;
    await playlist.loadMore();
  }, 300);
}

function attachScroll() {
  if (attached) return;
  scrollHost = document.querySelector("main");
  if (!scrollHost) return;
  scrollHost.addEventListener("scroll", onScroll, { passive: true });
  attached = true;
}

function detachScroll() {
  clearScrollTimer();
  if (scrollHost && attached) {
    scrollHost.removeEventListener("scroll", onScroll);
  }
  scrollHost = null;
  attached = false;
}

async function ensureLoaded() {
  if (!playlist.preview) {
    await router.replace("/");
    return;
  }

  let configured = playlist.pageSize;
  try {
    configured = (await getDownloadSettings()).playlistPageSize;
  } catch {
    // 读取设置失败时沿用当前值
  }

  if (playlist.tracks.length === 0) {
    playlist.pageSize = configured;
    await playlist.loadMore();
  } else if (playlist.pageSize !== configured) {
    // 设置页改过数量：下次回到歌单按新数量重新加载
    await playlist.setPageSize(configured);
  }
}

onMounted(async () => {
  await ensureLoaded();
  attachScroll();
});

onActivated(async () => {
  await ensureLoaded();
  attachScroll();
});

onDeactivated(detachScroll);
onUnmounted(detachScroll);

// 打开另一个歌单时清掉上一份选择/搜索词
watch(
  () => playlist.preview,
  () => {
    selected.value = new Set();
    searchInput.value = "";
  },
);

// ── 本地搜索（只过滤已加载的几十/几百首） ───────────────────
function applySearch() {
  playlist.query = searchInput.value.trim();
}

function clearSearch() {
  searchInput.value = "";
  playlist.query = "";
}

// ── 选择 ───────────────────────────────────────────────────
function toggle(track: PlaylistTrack) {
  const next = new Set(selected.value);
  if (next.has(track.id)) next.delete(track.id);
  else next.add(track.id);
  selected.value = next;
}

function toggleAll() {
  if (allVisibleSelected.value) {
    selected.value = new Set();
    return;
  }
  selected.value = new Set(visibleTracks.value.map((track) => track.id));
}

function invertSelection() {
  const next = new Set<string>();
  for (const track of visibleTracks.value) {
    if (!selected.value.has(track.id)) next.add(track.id);
  }
  selected.value = next;
}

const selectedTracks = computed(() =>
  playlist.tracks.filter((track) => selected.value.has(track.id)),
);

// ── 批量入队：3 并发，避免一次打爆音源接口 ──────────────────
async function downloadSelected() {
  const items = [...selectedTracks.value];
  if (items.length === 0 || downloading.value) return;

  downloading.value = true;
  downloadDone.value = 0;
  downloadTotal.value = items.length;

  const quality = prefs.settings.defaultQuality;
  const queue = [...items];
  let ok = 0;
  let failed = 0;

  const worker = async () => {
    while (queue.length > 0) {
      const track = queue.shift();
      if (!track) break;
      try {
        await tasks.startSource(
          track.source,
          { id: track.id, title: track.title, artist: track.artist },
          quality,
        );
        ok += 1;
      } catch {
        failed += 1;
      }
      downloadDone.value += 1;
    }
  };

  try {
    await Promise.all(
      Array.from({ length: Math.min(3, items.length) }, () => worker()),
    );
  } finally {
    downloading.value = false;
  }

  selected.value = new Set();

  if (ok > 0) {
    toast.show(
      `已入队 ${ok} 首${failed ? `，失败 ${failed} 首` : ""}`,
      failed ? "warning" : "default",
    );
    await router.push("/tasks");
  } else {
    toast.show(`批量入队失败 ${failed} 首`, "warning");
  }
}
</script>

<template>
  <div class="flex w-full flex-col gap-3 pb-28">
    <SCard v-if="playlist.preview" class="flex items-center gap-3">
      <img
        v-if="playlist.preview.coverUrl"
        :src="playlist.preview.coverUrl"
        alt=""
        referrerpolicy="no-referrer"
        class="size-16 shrink-0 rounded-card object-cover"
      />
      <div
        v-else
        class="grid size-16 shrink-0 place-items-center rounded-card bg-surface-highest text-on-surface-variant"
      >
        <span class="text-lg">♫</span>
      </div>
      <div class="min-w-0 flex-1">
        <p class="truncate text-sm font-medium">{{ playlist.preview.title }}</p>
      </div>
    </SCard>

    <!-- 搜索 / 选择工具栏 -->
    <div class="glass-card rounded-card p-2">
      <div class="flex items-center gap-2">
        <input
          v-model="searchInput"
          type="search"
          enterkeyhint="search"
          class="min-w-0 flex-1 rounded-pill bg-surface-low px-4 py-2 text-xs outline-none placeholder:text-on-surface-variant/60"
          placeholder="在已加载曲目中搜索歌名 / 歌手，回车生效"
          spellcheck="false"
          @keyup.enter="applySearch"
        />
        <button
          v-if="searchInput || playlist.query"
          type="button"
          class="shrink-0 rounded-pill bg-surface-high px-3 py-2 text-[11px] text-on-surface-variant"
          @click="clearSearch"
        >
          清空
        </button>
        <button
          type="button"
          class="shrink-0 rounded-pill bg-primary-container px-3 py-2 text-[11px] font-medium text-on-primary-container"
          @click="applySearch"
        >
          搜索
        </button>
      </div>

      <div class="mt-2 flex items-center gap-1.5 px-1">
        <button
          type="button"
          class="rounded-pill bg-surface-high px-3 py-1 text-[10px] text-on-surface-variant"
          @click="toggleAll"
        >
          {{ allVisibleSelected ? "取消全选" : "全选" }}
        </button>
        <button
          type="button"
          class="rounded-pill bg-surface-high px-3 py-1 text-[10px] text-on-surface-variant"
          @click="invertSelection"
        >
          反选
        </button>
        <span class="ml-auto text-[10px] text-on-surface-variant">
          已选 {{ selected.size }}
        </span>
      </div>
    </div>

    <!-- 首屏骨架 -->
    <div v-if="playlist.loading" class="flex flex-col gap-2">
      <div
        v-for="n in 8"
        :key="n"
        class="h-12 animate-pulse rounded-card bg-surface-low/60"
      />
    </div>

    <SCard v-else-if="playlist.error && playlist.tracks.length === 0" tone="low">
      <p class="text-xs text-error">{{ playlist.error }}</p>
      <button
        type="button"
        class="mt-3 rounded-pill bg-primary-container px-4 py-2 text-xs font-medium text-on-primary-container"
        @click="playlist.retry()"
      >
        重试
      </button>
    </SCard>

    <SCard
      v-else-if="visibleTracks.length === 0"
      tone="low"
      class="items-center text-center"
    >
      <p class="text-sm font-medium">
        {{ playlist.query ? "已加载曲目中没有匹配" : "歌单还没有曲目" }}
      </p>
      <p class="mt-1 text-xs text-on-surface-variant">
        {{ playlist.query ? "清空搜索查看完整歌单" : "换一个歌单链接试试" }}
      </p>
    </SCard>

    <!-- 曲目列表 -->
    <div v-else class="flex flex-col gap-2">
      <button
        v-for="track in visibleTracks"
        :key="`${track.source}:${track.id}`"
        type="button"
        class="flex w-full items-center gap-3 rounded-card px-3 py-2.5 text-left transition"
        :class="
          selected.has(track.id)
            ? 'bg-primary-container/80 text-on-primary-container'
            : 'bg-surface-low/70 text-on-surface'
        "
        @click="toggle(track)"
      >
        <span
          class="grid size-5 shrink-0 place-items-center rounded-pill border"
          :class="
            selected.has(track.id)
              ? 'border-primary bg-primary text-on-primary'
              : 'border-outline-variant/60'
          "
        >
          <svg
            v-if="selected.has(track.id)"
            class="size-3"
            viewBox="0 0 24 24"
            fill="currentColor"
            aria-hidden="true"
          >
            <path d="M9 16.2 4.8 12l-1.4 1.4L9 19 21 7l-1.4-1.4z" />
          </svg>
        </span>

        <span class="w-8 shrink-0 text-right font-mono text-[10px] text-on-surface-variant">
          {{ track.index }}
        </span>

        <span class="min-w-0 flex-1">
          <span class="block truncate text-xs font-medium">{{ track.title }}</span>
          <span class="block truncate text-[10px] text-on-surface-variant">
            {{ track.artist }} · {{ track.album }}
          </span>
        </span>

        <span class="shrink-0 font-mono text-[10px] text-on-surface-variant">
          {{ formatDuration(track.durationMs) }}
        </span>
      </button>
    </div>

    <!-- 加载状态 / 本地搜索提示 -->
    <div class="flex flex-col items-center gap-1 py-2 text-[11px] text-on-surface-variant">
      <template v-if="playlist.query">
        <span>本地搜索：已加载 {{ playlist.tracks.length }} 首，匹配 {{ visibleTracks.length }} 首</span>
        <span class="text-on-surface-variant/60">清空搜索恢复列表与触底加载</span>
      </template>
      <template v-else-if="playlist.loadingMore">
        <span class="inline-flex items-center gap-2">
          <span class="size-3 animate-spin rounded-pill border-2 border-outline-variant border-t-primary" />
          正在加载更多…
        </span>
      </template>
      <template v-else-if="playlist.error">
        <button type="button" class="text-primary" @click="playlist.retry()">
          加载失败，点此重试
        </button>
      </template>
      <template v-else-if="!playlist.hasMore && playlist.tracks.length > 0">
        <span>已加载全部 {{ playlist.tracks.length }} 首</span>
      </template>
      <template v-else-if="playlist.tracks.length > 0">
        <span>上拉加载更多</span>
      </template>
    </div>

    <!-- 批量下载：浮在全局底栏上方 -->
    <div
      v-if="selectedCount > 0"
      class="fixed inset-x-0 bottom-[calc(5rem+env(safe-area-inset-bottom))] z-20 px-5"
    >
      <div
        class="glass glass-edge mx-auto flex max-w-3xl items-center gap-1.5 rounded-pill p-1.5 pl-4 shadow-e3"
      >
        <span class="min-w-0 flex-1 truncate text-[11px] text-on-surface-variant">
          已选 {{ selectedCount }} 首
        </span>
        <button
          type="button"
          class="shrink-0 rounded-pill bg-surface-high px-3 py-2 text-[11px] text-on-surface-variant"
          :disabled="downloading"
          @click="selected = new Set()"
        >
          清空
        </button>
        <button
          type="button"
          class="shrink-0 rounded-pill bg-primary px-4 py-2 text-xs font-medium text-on-primary disabled:opacity-50"
          :disabled="downloading"
          @click="downloadSelected"
        >
          {{ downloading ? `入队 ${downloadDone}/${downloadTotal}` : "批量下载" }}
        </button>
      </div>
    </div>
  </div>
</template>
