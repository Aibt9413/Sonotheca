<script setup lang="ts">
import { computed, onActivated, onDeactivated, onMounted, onUnmounted, ref, watch } from "vue";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import SCard from "@/ui/SCard.vue";
import FileManager from "@/components/FileManager.vue";
import TrackCover from "@/components/TrackCover.vue";
import ScrapeSheet from "@/components/ScrapeSheet.vue";
import { useBackGuard } from "@/composables/useBackGuard";
import { useLibraryStore } from "@/stores/library";
import { usePlayerStore } from "@/stores/player";
import { useUiStore } from "@/stores/ui";
import { formatBitrate, formatBytes, formatDuration } from "@/utils/format";
import {
  batchScrapeMetadata,
  describeError,
  getTrack,
  listTrash,
  purgeTrash,
  readTrackLyrics,
  renameTrack,
  restoreTrash,
  scanLibraryDir,
  trackPlayInfo,
  trackPlayPath,
  syncTrackLyrics,
  trashTrack,
  updateTrackMetadata,
  type MetadataProgress,
  type ScanProgress,
  type Track,
  type TrashEntry,
} from "@/ipc/bindings";

defineOptions({ name: "LibraryView" });

const library = useLibraryStore();
const player = usePlayerStore();

const error = ref<string | null>(null);
const notice = ref<string | null>(null);

// 多选
const selecting = ref(false);
const selected = ref<number[]>([]);
const selectedTracks = computed(() =>
  library.tracks.filter((track) => selected.value.includes(track.id)),
);

// 浮层
const actionTrack = ref<Track | null>(null);
const scrapeTrack = ref<Track | null>(null);
const manager = ref<{ selection: string[] } | null>(null);
const trashOpen = ref(false);
const trash = ref<TrashEntry[]>([]);

// 扫描
const scan = ref<ScanProgress | null>(null);
const metadataProgress = ref<MetadataProgress | null>(null);
const searchQuery = ref("");

const visibleTracks = computed(() => {
  const query = searchQuery.value.trim().toLowerCase();
  if (!query) return library.tracks;
  return library.tracks.filter((track) =>
    [track.title, track.artist, track.album, track.relPath].some((value) =>
      value.toLowerCase().includes(query),
    ),
  );
});
const metadataPercent = computed(() => {
  const current = metadataProgress.value;
  if (!current || current.total <= 0) return 0;
  return Math.min(100, Math.round((current.current / current.total) * 100));
});

// ── 音乐库无限滚动：底部停顿 300ms 再请求下一页 ─────────────
let libraryScrollHost: HTMLElement | null = null;
let libraryScrollTimer: ReturnType<typeof setTimeout> | null = null;
let libraryScrollAttached = false;

function clearLibraryScrollTimer() {
  if (libraryScrollTimer) {
    clearTimeout(libraryScrollTimer);
    libraryScrollTimer = null;
  }
}

function onLibraryScroll() {
  if (!libraryScrollHost || searchQuery.value.trim()) return;

  const host = libraryScrollHost;
  const nearBottom = host.scrollHeight - host.scrollTop - host.clientHeight < 200;
  if (!nearBottom) {
    clearLibraryScrollTimer();
    return;
  }

  if (
    libraryScrollTimer ||
    library.loading ||
    library.loadingMore ||
    !library.hasMore
  ) {
    return;
  }

  libraryScrollTimer = setTimeout(async () => {
    libraryScrollTimer = null;
    const current = libraryScrollHost;
    if (!current || searchQuery.value.trim()) return;
    if (current.scrollHeight - current.scrollTop - current.clientHeight >= 200) return;
    await library.loadMore();
  }, 300);
}

function attachLibraryScroll() {
  if (libraryScrollAttached) return;
  libraryScrollHost = document.querySelector("main");
  if (!libraryScrollHost) return;
  libraryScrollHost.addEventListener("scroll", onLibraryScroll, { passive: true });
  libraryScrollAttached = true;
}

function detachLibraryScroll() {
  clearLibraryScrollTimer();
  if (libraryScrollHost && libraryScrollAttached) {
    libraryScrollHost.removeEventListener("scroll", onLibraryScroll);
  }
  libraryScrollHost = null;
  libraryScrollAttached = false;
}

// 本地搜索覆盖未加载的曲目：输入后补齐全部分页，再做本地过滤
let searchLoadTimer: ReturnType<typeof setTimeout> | null = null;
watch(searchQuery, (value) => {
  if (searchLoadTimer) clearTimeout(searchLoadTimer);
  searchLoadTimer = null;
  if (!value.trim() || library.tracks.length >= library.total) return;
  searchLoadTimer = setTimeout(() => {
    searchLoadTimer = null;
    void library.loadAll();
  }, 300);
});

let unlisteners: UnlistenFn[] = [];

onMounted(async () => {
  void library.refresh();
  attachLibraryScroll();

  unlisteners.push(
    await listen<ScanProgress>("library://scan-progress", ({ payload }) => {
      scan.value = payload;
      if (payload.done) {
        void library.refresh();
        setTimeout(() => (scan.value = null), 1800);
      }
    }),
  );

  unlisteners.push(await listen("library://changed", () => void library.refresh()));

  unlisteners.push(
    await listen<MetadataProgress>("metadata_progress", ({ payload }) => {
      metadataProgress.value = payload;
    }),
  );
});

onActivated(() => {
  if (library.tracks.length === 0 && !library.loading) void library.refresh();
  attachLibraryScroll();
});

onDeactivated(() => {
  detachLibraryScroll();
  detailsTrack.value = null;
  renameTarget.value = null;
  actionTrack.value = null;
  editingTrack.value = null;
  trashOpen.value = false;
  ui.setSheetOpen(false);
});

onUnmounted(() => {
  detachLibraryScroll();
  ui.setSheetOpen(false);
  unlisteners.forEach((fn) => fn());
  unlisteners = [];
});

function flash(message: string) {
  notice.value = message;
  setTimeout(() => {
    if (notice.value === message) notice.value = null;
  }, 2600);
}

// ── 手势：短按切换 / 长按进入多选 / 长按后滑动区间选中 ──────────
interface LibPending {
  id: number;
  index: number;
  x: number;
  y: number;
}

let libPending: LibPending | null = null;
let libTimer: ReturnType<typeof setTimeout> | null = null;
let libLong = false;
let libAnchor = -1;
let libBase: number[] = [];
let touchStartY = 0;
let suppressPlay = false;

function onCardTouchStart(event: TouchEvent) {
  const touch = event.touches[0];
  if (!touch) return;
  touchStartY = touch.clientY;
  suppressPlay = false;
}

function onCardTouchEnd(event: TouchEvent) {
  const touch = event.changedTouches[0];
  if (touch && Math.abs(touch.clientY - touchStartY) > 10) {
    suppressPlay = true;
  }
}

function cardDown(track: Track, index: number, event: PointerEvent) {
  libPending = { id: track.id, index, x: event.clientX, y: event.clientY };
  libLong = false;
  suppressPlay = false;

  if (libTimer) clearTimeout(libTimer);
  libTimer = setTimeout(() => {
    libLong = true;
    selecting.value = true;
    libAnchor = index;
    // 记住长按前已选的，划选时与之取并集，不清空别人的选择
    libBase = [...selected.value];
    if (!selected.value.includes(track.id)) selected.value.push(track.id);
  }, 500);

  window.addEventListener("pointermove", onCardMove);
  window.addEventListener("pointerup", onCardUp, { once: true });
  window.addEventListener("pointercancel", onCardUp, { once: true });
}

function onCardMove(event: PointerEvent) {
  if (!libPending) return;

  const distance = Math.hypot(event.clientX - libPending.x, event.clientY - libPending.y);

  if (!libLong) {
    // 还没到 450ms 就动了 → 当滚动，放弃长按
    if (distance > 10) {
      suppressPlay = true;
      if (libTimer) clearTimeout(libTimer);
      libTimer = null;
      detachCardGesture();
    }
    return;
  }

  const element = document.elementFromPoint(event.clientX, event.clientY) as HTMLElement | null;
  const row = element?.closest("[data-track-index]") as HTMLElement | null;
  if (!row) return;

  const hover = Number(row.dataset.trackIndex);
  if (Number.isNaN(hover)) return;

  // 锚点 → 当前触点，区间全选（与长按前的选择取并集）
  const start = Math.min(libAnchor, hover);
  const end = Math.max(libAnchor, hover);
  const rangeIds = visibleTracks.value.slice(start, end + 1).map((item) => item.id);
  selected.value = Array.from(new Set([...libBase, ...rangeIds]));
}

function onCardUp() {
  if (libTimer) {
    clearTimeout(libTimer);
    libTimer = null;
  }

  const pending = libPending;
  const wasLong = libLong;
  detachCardGesture();

  if (!pending || wasLong) {
    if (wasLong) suppressPlay = true;
    return;
  }

  // 短按：多选模式下切换选择，普通模式交给 click 播放
  if (selecting.value) {
    const index = selected.value.indexOf(pending.id);
    if (index >= 0) selected.value.splice(index, 1);
    else selected.value.push(pending.id);
  }
}

function onCardClick(track: Track) {
  if (selecting.value) return;
  if (suppressPlay) {
    suppressPlay = false;
    return;
  }
  void playLibraryTrack(track);
}

async function playLibraryTrack(track: Track) {
  error.value = null;
  try {
    const info = await trackPlayInfo(track.id);
    player.playLocal(info.url, {
      id: `local:${track.id}`,
      title: info.title || track.title,
      artist: info.artist || track.artist,
      album: info.album || track.album,
      coverUrl: info.coverUrl,
      lyrics: info.lyrics,
    });
  } catch (e) {
    error.value = describeError(e);
  }
}

function detachCardGesture() {
  libPending = null;
  libLong = false;
  libAnchor = -1;
  libBase = [];
  window.removeEventListener("pointermove", onCardMove);
}

function clearSelection() {
  selecting.value = false;
  selected.value = [];
}

// 取消掉最后一个选中项时自动退出多选模式（底部工具栏随之消失）
watch(selected, (value) => {
  if (value.length === 0 && selecting.value) selecting.value = false;
});

// 多选模式下拦截系统返回手势：只取消多选，不退出页面
useBackGuard(
  () => selecting.value,
  () => clearSelection(),
);

const detailsTrack = ref<Track | null>(null);
const renameTarget = ref<Track | null>(null);
const renameValue = ref("");
const renaming = ref(false);

// 手动编辑元数据
const editingTrack = ref<Track | null>(null);
const editTitle = ref("");
const editArtist = ref("");
const editAlbum = ref("");
const editCover = ref<string | null>(null);
const editLyrics = ref("");
const editLyricsLoaded = ref(false);
const syncingLyrics = ref(false);
const editSaving = ref(false);
const editError = ref<string | null>(null);
const editCoverInput = ref<HTMLInputElement | null>(null);

// 任一浮层打开时让全局 Tab 栏让位，避免盖住 Bottom Sheet 的取消按钮
const ui = useUiStore();
const anyOverlay = computed(
  () =>
    Boolean(detailsTrack.value) ||
    Boolean(renameTarget.value) ||
    Boolean(actionTrack.value) ||
    Boolean(editingTrack.value) ||
    trashOpen.value,
);
watch(anyOverlay, (open) => ui.setSheetOpen(open));

function openDetails() {
  const track = selectedTracks.value[0];
  if (track) detailsTrack.value = track;
}

function openRename() {
  const track = selectedTracks.value[0];
  if (!track) return;
  renameTarget.value = track;
  renameValue.value = track.relPath.split("/").pop() ?? track.title;
}

function beginEdit(track: Track) {
  editingTrack.value = track;
  editTitle.value = track.title;
  editArtist.value = track.artist;
  editAlbum.value = track.album;
  editCover.value = null;
  editLyrics.value = "";
  editLyricsLoaded.value = false;
  editError.value = null;
  detailsTrack.value = null;

  void readTrackLyrics(track.id)
    .then((lyrics) => {
      editLyrics.value = lyrics;
      editLyricsLoaded.value = true;
    })
    .catch(() => {
      // 读不到内嵌歌词时不覆盖原字段
    });
}

function openEditMetadata() {
  const track = detailsTrack.value;
  if (track) beginEdit(track);
}

async function trackById(trackId: number): Promise<Track | null> {
  const cached = library.tracks.find((track) => track.id === trackId);
  if (cached) return cached;
  try {
    return await getTrack(trackId);
  } catch (e) {
    error.value = describeError(e);
    return null;
  }
}

async function openScrapeById(trackId: number) {
  const track = await trackById(trackId);
  if (!track) return;
  manager.value = null;
  clearSelection();
  scrapeTrack.value = track;
}

async function openEditById(trackId: number) {
  const track = await trackById(trackId);
  if (!track) return;
  manager.value = null;
  clearSelection();
  beginEdit(track);
}

async function syncLyrics() {
  const track = editingTrack.value;
  if (!track || syncingLyrics.value) return;
  syncingLyrics.value = true;
  editError.value = null;
  try {
    const lyrics = await syncTrackLyrics(track.id);
    if (!lyrics.trim()) {
      editError.value = "没有找到匹配的歌词";
      return;
    }
    editLyrics.value = lyrics;
    editLyricsLoaded.value = true;
  } catch (e) {
    editError.value = describeError(e);
  } finally {
    syncingLyrics.value = false;
  }
}

function pickEditCover(event: Event) {
  const input = event.target as HTMLInputElement;
  const file = input.files?.[0];
  input.value = "";
  if (!file) return;
  if (!file.type.startsWith("image/")) {
    editError.value = "请选择图片文件";
    return;
  }
  if (file.size > 8 * 1024 * 1024) {
    editError.value = "封面不能超过 8MB";
    return;
  }

  const reader = new FileReader();
  reader.onload = () => {
    if (typeof reader.result === "string") editCover.value = reader.result;
  };
  reader.onerror = () => (editError.value = "读取封面失败");
  reader.readAsDataURL(file);
}

async function saveEditMetadata() {
  const track = editingTrack.value;
  if (!track || editSaving.value) return;
  if (!editTitle.value.trim()) {
    editError.value = "标题不能为空";
    return;
  }

  editSaving.value = true;
  editError.value = null;
  try {
    let absolutePath: string | null = null;
    try {
      absolutePath = await trackPlayPath(track.id);
    } catch {
      // 退回 Rust 侧的 DB relPath 解析
    }

    // 正在播放这首歌时先卸载 <audio>，释放文件句柄
    if (player.track?.id === `local:${track.id}`) player.releaseSource();

    const updated = await updateTrackMetadata(
      track.id,
      absolutePath,
      editTitle.value.trim(),
      editArtist.value.trim(),
      editAlbum.value.trim(),
      editCover.value,
      editLyricsLoaded.value ? editLyrics.value : null,
    );
    library.replace(updated);
    editingTrack.value = null;
    flash("元数据已更新");
  } catch (e) {
    editError.value = describeError(e);
  } finally {
    editSaving.value = false;
  }
}

async function doRename() {
  if (!renameTarget.value || renaming.value) return;
  renaming.value = true;
  error.value = null;
  try {
    await renameTrack(renameTarget.value.id, renameValue.value);
    renameTarget.value = null;
    clearSelection();
    await library.refresh();
    flash("已重命名");
  } catch (e) {
    error.value = describeError(e);
  } finally {
    renaming.value = false;
  }
}

/** 当前选中的全部文件交给双窗格去复制/移动 */
function openManagerForSelection() {
  const paths = selectedTracks.value.map((track) => track.relPath);
  if (paths.length === 0) return;
  manager.value = { selection: paths };
}

// ── 单曲操作 ──────────────────────────────────────────────
async function removeTrack(id: number) {
  actionTrack.value = null;
  try {
    await trashTrack(id);
    // 立刻从响应式列表移除，不等下一次 refresh
    library.remove(id);
    flash("已移入回收站");
    await library.refresh();
  } catch (e) {
    error.value = describeError(e);
  }
}

async function batchScrape() {
  const targets = [...selectedTracks.value];
  clearSelection();
  if (targets.length === 0) return;

  error.value = null;
  metadataProgress.value = { current: 0, total: targets.length };

  try {
    const result = await batchScrapeMetadata(targets.map((track) => track.id));
    flash(
      `批量刮削：成功 ${result.applied} 首${
        result.skipped ? `，跳过 ${result.skipped} 首` : ""
      }${result.failed ? `，失败 ${result.failed} 首` : ""}`,
    );
    await library.refresh();
  } catch (e) {
    error.value = describeError(e);
  } finally {
    metadataProgress.value = null;
  }
}

async function batchTrash() {
  const ids = [...selected.value];
  clearSelection();
  for (const id of ids) {
    try {
      await trashTrack(id);
      library.remove(id);
    } catch {
      /* 单条失败不影响其余 */
    }
  }
  flash("已移入回收站");
  await library.refresh();
}

function openManager(paths: string[]) {
  if (paths.length === 0) {
    flash("先选中要整理的文件");
    return;
  }
  manager.value = { selection: paths };
}

/** ⋮ 菜单里的「文件管理」：针对当前这一首 */
function moveCurrent() {
  const track = actionTrack.value;
  if (!track) return;
  actionTrack.value = null;
  openManager([track.relPath]);
}

// ── 扫描 / 回收站 ─────────────────────────────────────────
async function startScan() {
  error.value = null;
  try {
    await scanLibraryDir("");
    flash("开始扫描…");
  } catch (e) {
    error.value = describeError(e);
  }
}

/** 顶栏唯一刷新按钮：重新扫描 + 重置分页列表 */
async function rescan() {
  await startScan();
  await library.refresh();
}

async function openTrash() {
  trashOpen.value = true;
  try {
    trash.value = await listTrash();
  } catch (e) {
    error.value = describeError(e);
  }
}

async function doRestore(id: number) {
  try {
    await restoreTrash(id);
    trash.value = await listTrash();
    await library.refresh();
    flash("已恢复");
  } catch (e) {
    error.value = describeError(e);
  }
}

async function doPurge(id?: number) {
  try {
    await purgeTrash(id);
    trash.value = await listTrash();
    flash("已清空");
  } catch (e) {
    error.value = describeError(e);
  }
}

function closeScrape() {
  scrapeTrack.value = null;
  metadataProgress.value = null;
}

async function onScraped() {
  scrapeTrack.value = null;
  metadataProgress.value = null;
  await library.refresh();
  flash("元数据已写入");
}

/** 双窗格内互传成功后：只刷新列表，不关闭窗口 */
async function onApplied() {
  await library.refresh();
}
</script>

<template>
  <div class="flex w-full flex-col gap-3">
  <SCard tone="low">
    <div class="flex items-center justify-between gap-3">
      <div class="min-w-0">
        <p class="text-sm font-medium">音乐库</p>
        <p class="mt-0.5 text-xs text-on-surface-variant">
          {{ library.total }} 个文件
        </p>
      </div>
      <div class="flex shrink-0 items-center gap-1.5">
        <button
          type="button"
          class="grid size-9 place-items-center rounded-pill bg-surface-high text-on-surface-variant disabled:opacity-40"
          aria-label="刷新并扫描"
          title="刷新并扫描"
          :disabled="library.loading || !!scan"
          @click="rescan"
        >
          <svg class="size-4" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
            <path d="M17.65 6.35A7.95 7.95 0 0 0 12 4a8 8 0 1 0 7.73 10h-2.08A6 6 0 1 1 12 6c1.66 0 3.14.69 4.22 1.78L13 11h7V4l-2.35 2.35z" />
          </svg>
        </button>
        <button
          type="button"
          class="grid size-9 place-items-center rounded-pill bg-surface-high text-on-surface-variant"
          aria-label="回收站"
          title="回收站"
          @click="openTrash"
        >
          <svg class="size-4" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
            <path d="M6 19c0 1.1.9 2 2 2h8c1.1 0 2-.9 2-2V7H6v12zM8 9h8v10H8V9zm7.5-5-1-1h-5l-1 1H5v2h14V4z" />
          </svg>
        </button>
      </div>
    </div>

    <p v-if="scan" class="mt-2 text-xs text-on-surface-variant">
      {{ scan.done ? `扫描完成，入库 ${scan.inserted} 首` : `已扫描 ${scan.scanned} 首…` }}
    </p>

    <div class="mt-3 flex items-center gap-2">
      <input
        v-model="searchQuery"
        type="search"
        enterkeyhint="search"
        class="min-w-0 flex-1 rounded-pill bg-surface-low px-4 py-2.5 text-xs outline-none placeholder:text-on-surface-variant/60"
        placeholder="搜索歌名 / 艺术家 / 专辑 / 路径"
        spellcheck="false"
      />
      <button
        v-if="searchQuery"
        type="button"
        class="shrink-0 rounded-pill bg-surface-high px-3 py-2.5 text-[11px] text-on-surface-variant"
        @click="searchQuery = ''"
      >
        清空
      </button>
    </div>
  </SCard>

  <!-- 补全进度：单首与批量共用 metadata_progress 事件 -->
  <SCard v-if="metadataProgress" tone="low">
    <div class="flex items-center justify-between gap-3 text-[11px]">
      <span class="font-medium">正在补全 ({{ metadataProgress.current }}/{{ metadataProgress.total }})</span>
      <span class="font-mono text-on-surface-variant">{{ metadataPercent }}%</span>
    </div>
    <div class="mt-2 h-1.5 overflow-hidden rounded-pill bg-surface-highest">
      <div
        class="h-full rounded-pill bg-primary transition-[width] duration-200"
        :style="{ width: `${metadataPercent}%` }"
      />
    </div>
  </SCard>

  <p v-if="error" class="px-1 text-xs text-error">{{ error }}</p>
  <p v-else-if="notice" class="px-1 text-xs text-on-surface-variant">{{ notice }}</p>

  <SCard v-if="library.tracks.length === 0" class="border border-dashed border-outline-variant/60">
    <p class="text-sm font-medium">音乐库为空</p>
    <p class="mt-1 text-xs text-on-surface-variant">
      点「扫描」把已有文件登记进来，或去「任务」页下载。
    </p>
  </SCard>

  <p v-else-if="visibleTracks.length === 0" class="px-1 py-8 text-center text-xs text-on-surface-variant">
    没有匹配的歌曲
  </p>

  <!-- 多选工具栏 -->
  <!-- 多选工具栏：单选给详情/重命名，多选只留批量操作 -->
  <div v-if="selecting" class="flex flex-col gap-2 rounded-card bg-primary-container p-2.5">
    <div class="flex items-center justify-between px-1">
      <span class="text-xs text-on-primary-container">已选 {{ selected.length }} 首</span>
      <button type="button" class="text-[11px] text-on-primary-container/80" @click="clearSelection">
        取消
      </button>
    </div>

    <div class="flex flex-wrap gap-1">
      <template v-if="selected.length === 1">
        <button type="button" class="rounded-pill bg-surface/30 px-3 py-1.5 text-[11px] text-on-primary-container" @click="openDetails">
          详情
        </button>
        <button type="button" class="rounded-pill bg-surface/30 px-3 py-1.5 text-[11px] text-on-primary-container" @click="openRename">
          重命名
        </button>
        <button type="button" class="rounded-pill bg-surface/30 px-3 py-1.5 text-[11px] text-on-primary-container" @click="openManagerForSelection">
          文件管理
        </button>
      </template>

      <template v-else>
        <button type="button" class="rounded-pill bg-surface/30 px-3 py-1.5 text-[11px] text-on-primary-container" @click="batchScrape">
          批量刮削
        </button>
        <button type="button" class="rounded-pill bg-surface/30 px-3 py-1.5 text-[11px] text-on-primary-container" @click="openManagerForSelection">
          文件管理
        </button>
      </template>

      <button type="button" class="rounded-pill bg-error-container/80 px-3 py-1.5 text-[11px] text-on-error-container" @click="batchTrash">
        删除
      </button>
    </div>
  </div>

  <SCard
    v-for="(track, index) in visibleTracks"
    :key="track.id"
    :data-track-index="index"
    class="no-select flex items-center gap-3"
    :class="selected.includes(track.id) ? 'bg-primary-container text-on-primary-container' : ''"
    @pointerdown="cardDown(track, index, $event)"
    @click="onCardClick(track)"
    @touchstart.passive="onCardTouchStart"
    @touchend.passive="onCardTouchEnd"
  >
    <TrackCover :track="track" />
    <div class="min-w-0 flex-1">
      <p class="truncate text-sm font-medium">{{ track.title }}</p>
      <p class="truncate text-xs text-on-surface-variant">
        {{ track.artist }} · {{ formatDuration(track.durationMs) }}
      </p>
    </div>
    <div class="shrink-0 text-right">
      <p class="font-mono text-[11px]">{{ formatBytes(track.sizeBytes) }}</p>
      <p class="font-mono text-[11px] text-on-surface-variant">{{ formatBitrate(track.bitrate) }}</p>
    </div>
    <button
      type="button"
      class="grid size-9 shrink-0 place-items-center rounded-pill text-on-surface-variant"
      @pointerdown.stop
      @click.stop="actionTrack = track"
    >
      ⋮
    </button>
  </SCard>

  <div v-if="library.loadingMore" class="py-3 text-center text-xs text-on-surface-variant">
    正在加载更多…
  </div>
  <p
    v-else-if="!library.hasMore && library.tracks.length > 0"
    class="py-3 text-center text-[11px] text-on-surface-variant"
  >
    已加载全部 {{ library.total }} 首
  </p>

  <!-- 详情 -->
  <div
    v-if="detailsTrack"
    class="fixed inset-0 z-50 flex items-end bg-black/50"
    @click.self="detailsTrack = null"
  >
    <div class="glass-sheet max-h-[85vh] w-full overflow-y-auto rounded-t-card p-5 pb-safe shadow-e3">
      <div class="mx-auto mb-3 h-1 w-10 rounded-pill bg-outline-variant" />
      <p class="text-sm font-medium">文件详情</p>

      <button
        type="button"
        class="mt-3 w-full rounded-card bg-primary-container py-2.5 text-xs font-medium text-on-primary-container"
        @click="openEditMetadata"
      >
        编辑元数据
      </button>

      <div class="mt-3 space-y-2">
        <div class="rounded-card bg-surface-low p-3">
          <p class="text-[10px] text-on-surface-variant">标题</p>
          <p class="mt-0.5 text-sm">{{ detailsTrack.title }}</p>
        </div>
        <div class="rounded-card bg-surface-low p-3">
          <p class="text-[10px] text-on-surface-variant">路径</p>
          <p class="mt-0.5 break-all font-mono text-[11px]">{{ detailsTrack.relPath }}</p>
        </div>
        <div class="grid grid-cols-2 gap-2">
          <div class="rounded-card bg-surface-low p-3">
            <p class="text-[10px] text-on-surface-variant">艺术家</p>
            <p class="mt-0.5 text-xs">{{ detailsTrack.artist || "—" }}</p>
          </div>
          <div class="rounded-card bg-surface-low p-3">
            <p class="text-[10px] text-on-surface-variant">专辑</p>
            <p class="mt-0.5 text-xs">{{ detailsTrack.album || "—" }}</p>
          </div>
          <div class="rounded-card bg-surface-low p-3">
            <p class="text-[10px] text-on-surface-variant">大小</p>
            <p class="mt-0.5 font-mono text-xs">{{ formatBytes(detailsTrack.sizeBytes) }}</p>
          </div>
          <div class="rounded-card bg-surface-low p-3">
            <p class="text-[10px] text-on-surface-variant">码率</p>
            <p class="mt-0.5 font-mono text-xs">{{ formatBitrate(detailsTrack.bitrate) }}</p>
          </div>
          <div class="rounded-card bg-surface-low p-3">
            <p class="text-[10px] text-on-surface-variant">时长</p>
            <p class="mt-0.5 font-mono text-xs">{{ formatDuration(detailsTrack.durationMs) }}</p>
          </div>
          <div class="rounded-card bg-surface-low p-3">
            <p class="text-[10px] text-on-surface-variant">格式</p>
            <p class="mt-0.5 font-mono text-xs uppercase">{{ detailsTrack.format || "—" }}</p>
          </div>
        </div>
      </div>

      <div class="mt-4">
        <button
          type="button"
          class="w-full rounded-card bg-surface-high py-3 text-sm text-on-surface-variant"
          @click="detailsTrack = null"
        >
          关闭
        </button>
      </div>
    </div>
  </div>

  <!-- 手动编辑元数据 -->
  <div
    v-if="editingTrack"
    class="fixed inset-0 z-50 flex items-end bg-black/50"
    @click.self="editingTrack = null"
  >
    <div class="glass-sheet max-h-[88vh] w-full overflow-y-auto rounded-t-card p-5 pb-safe shadow-e3">
      <div class="mx-auto mb-3 h-1 w-10 rounded-pill bg-outline-variant" />
      <p class="text-sm font-medium">编辑元数据</p>
      <p class="mt-0.5 truncate text-[11px] text-on-surface-variant">
        {{ editingTrack.relPath }}
      </p>

      <div class="mt-4 flex flex-col gap-2">
        <input
          v-model="editTitle"
          type="text"
          class="rounded-pill bg-surface-low px-4 py-2.5 text-sm outline-none"
          placeholder="标题"
          spellcheck="false"
        />
        <input
          v-model="editArtist"
          type="text"
          class="rounded-pill bg-surface-low px-4 py-2.5 text-sm outline-none"
          placeholder="艺术家"
          spellcheck="false"
        />
        <input
          v-model="editAlbum"
          type="text"
          class="rounded-pill bg-surface-low px-4 py-2.5 text-sm outline-none"
          placeholder="专辑"
          spellcheck="false"
        />
      </div>

      <div class="mt-3 flex items-center gap-3 rounded-card bg-surface-low p-3">
        <img
          v-if="editCover"
          :src="editCover"
          alt="新封面"
          class="size-12 rounded-card object-cover"
        />
        <div
          v-else
          class="grid size-12 shrink-0 place-items-center rounded-card bg-surface-highest text-[10px] text-on-surface-variant"
        >
          封面
        </div>
        <div class="min-w-0 flex-1">
          <p class="text-xs font-medium">封面</p>
          <p class="text-[10px] text-on-surface-variant">留空保留原封面</p>
        </div>
        <button
          type="button"
          class="shrink-0 rounded-pill bg-surface-high px-3 py-1.5 text-[11px] text-on-surface-variant"
          @click="editCoverInput?.click()"
        >
          选择
        </button>
        <input
          ref="editCoverInput"
          type="file"
          accept="image/*"
          class="hidden"
          @change="pickEditCover"
        />
      </div>

      <div class="mt-3 rounded-card bg-surface-low p-3">
        <div class="flex items-center justify-between gap-2">
          <p class="text-xs font-medium">歌词 LRC</p>
          <button
            type="button"
            class="rounded-pill bg-surface-high px-3 py-1.5 text-[11px] text-primary disabled:opacity-50"
            :disabled="syncingLyrics"
            @click="syncLyrics"
          >
            {{ syncingLyrics ? "同步中…" : "一键同步歌词" }}
          </button>
        </div>
        <textarea
          v-model="editLyrics"
          rows="7"
          class="mt-2 w-full resize-y rounded-card bg-surface px-3 py-2 font-mono text-[11px] leading-relaxed outline-none"
          placeholder="[00:00.00] 歌词…"
          spellcheck="false"
          @input="editLyricsLoaded = true"
        />
      </div>

      <p v-if="editError" class="mt-2 text-xs text-error">{{ editError }}</p>

      <div class="mt-4 flex gap-2">
        <button
          type="button"
          class="rounded-pill bg-surface-high px-4 py-2.5 text-xs text-on-surface-variant"
          :disabled="editSaving"
          @click="editingTrack = null"
        >
          取消
        </button>
        <button
          type="button"
          class="flex-1 rounded-pill bg-primary px-4 py-2.5 text-xs font-medium text-on-primary disabled:opacity-50"
          :disabled="editSaving || !editTitle.trim()"
          @click="saveEditMetadata"
        >
          {{ editSaving ? "保存中…" : "保存" }}
        </button>
      </div>
    </div>
  </div>

  <!-- 重命名 -->
  <div
    v-if="renameTarget"
    class="fixed inset-0 z-50 flex items-center justify-center bg-black/50 p-6"
    @click.self="renameTarget = null"
  >
    <div class="w-full max-w-sm rounded-card bg-surface-container p-5 shadow-e3">
      <p class="text-sm font-medium">重命名</p>
      <p class="mt-1 text-[11px] text-on-surface-variant">只改文件名，不动音频标签</p>

      <input
        v-model="renameValue"
        class="mt-3 w-full rounded-pill bg-surface-high px-4 py-2.5 text-sm outline-none"
        spellcheck="false"
        @keyup.enter="doRename"
      />

      <div class="mt-4 flex justify-end gap-2">
        <button type="button" class="rounded-pill px-4 py-2 text-xs text-on-surface-variant" @click="renameTarget = null">
          取消
        </button>
        <button
          type="button"
          class="rounded-pill bg-primary px-4 py-2 text-xs font-medium text-on-primary disabled:opacity-50"
          :disabled="renaming || !renameValue.trim()"
          @click="doRename"
        >
          {{ renaming ? "处理中…" : "确定" }}
        </button>
      </div>
    </div>
  </div>

  <!-- 单曲操作面板 -->
  <div
    v-if="actionTrack"
    class="fixed inset-0 z-50 flex items-end bg-black/50"
    @click.self="actionTrack = null"
  >
    <div class="glass-sheet w-full rounded-t-card p-5 pb-safe shadow-e3">
      <div class="mx-auto mb-3 h-1 w-10 rounded-pill bg-outline-variant" />
      <p class="truncate text-sm font-medium">{{ actionTrack.title }}</p>
      <div class="mt-3 flex flex-col gap-2">
        <button
          type="button"
          class="rounded-card bg-surface-low px-4 py-3 text-left text-sm"
          @click="scrapeTrack = actionTrack; actionTrack = null"
        >
          补全元数据
        </button>
        <button
          type="button"
          class="rounded-card bg-surface-low px-4 py-3 text-left text-sm"
          @click="moveCurrent"
        >
          文件管理
        </button>
        <button
          type="button"
          class="rounded-card bg-error-container px-4 py-3 text-left text-sm text-on-error-container"
          @click="removeTrack(actionTrack.id)"
        >
          移入回收站
        </button>
        <button
          type="button"
          class="rounded-card bg-surface-high px-4 py-3 text-left text-sm text-on-surface-variant"
          @click="actionTrack = null"
        >
          取消
        </button>
      </div>
    </div>
  </div>

  <!-- 回收站 -->
  <div
    v-if="trashOpen"
    class="fixed inset-0 z-50 flex items-end bg-black/50"
    @click.self="trashOpen = false"
  >
    <div class="glass-sheet max-h-[80vh] w-full overflow-y-auto rounded-t-card p-5 pb-safe shadow-e3">
      <div class="mx-auto mb-3 h-1 w-10 rounded-pill bg-outline-variant" />
      <div class="flex items-center justify-between">
        <p class="text-sm font-medium">回收站</p>
        <button type="button" class="text-xs text-error" @click="doPurge()">清空</button>
      </div>

      <p v-if="trash.length === 0" class="mt-3 text-xs text-on-surface-variant">空</p>

      <div class="mt-3 flex flex-col gap-2">
        <div
          v-for="item in trash"
          :key="item.id"
          class="flex items-center gap-3 rounded-card bg-surface-low p-3"
        >
          <div class="min-w-0 flex-1">
            <p class="truncate text-xs">{{ item.name }}</p>
            <p class="truncate text-[10px] text-on-surface-variant">{{ item.originalRelPath }}</p>
          </div>
          <button
            type="button"
            class="rounded-pill bg-primary-container px-3 py-1.5 text-[11px] text-on-primary-container"
            @click="doRestore(item.id)"
          >
            恢复
          </button>
        </div>
      </div>

      <button
        type="button"
        class="mt-4 w-full rounded-card bg-surface-high py-3 text-sm text-on-surface-variant"
        @click="trashOpen = false"
      >
        关闭
      </button>
    </div>
  </div>

  <!-- 双列文件管理 -->
  <FileManager
    v-if="manager"
    :selection="manager.selection"
    @close="manager = null; clearSelection()"
    @applied="onApplied"
    @scrape="openScrapeById"
    @edit="openEditById"
  />

  <!-- 刮削确认 -->
  <ScrapeSheet
    v-if="scrapeTrack"
    :track-id="scrapeTrack.id"
    :title="scrapeTrack.title"
    @close="closeScrape"
    @applied="onScraped"
  />
  </div>
</template>
