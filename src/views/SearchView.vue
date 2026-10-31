<script setup lang="ts">
import { computed, onActivated, onMounted, onUnmounted, ref, watch } from "vue";
import { useRouter } from "vue-router";
import QualitySheet from "@/components/QualitySheet.vue";
import SCard from "@/ui/SCard.vue";
import {
  addDirectDownloadTask,
  bilibiliPages,
  describeError,
  recentPlaylists,
  searchSources,
  type BiliPage,
  type ConcreteQuality,
  type NeteaseQuality,
  type QualityFallback,
  type RecentPlaylist,
  type SearchHit,
  type SourceFailure,
} from "@/ipc/bindings";
import { useAccountStore } from "@/stores/account";
import { usePlayerStore } from "@/stores/player";
import { usePlaylistStore } from "@/stores/playlist";
import { useSearchStore } from "@/stores/search";
import { useSettingsStore } from "@/stores/settings";
import { useTasksStore } from "@/stores/tasks";
import { useToastStore } from "@/stores/toast";
import { useUiStore } from "@/stores/ui";
import { fallbackToast } from "@/utils/quality";
import { formatDuration } from "@/utils/format";

// KeepAlive 的 include 按组件名匹配
defineOptions({ name: "SearchView" });

const tasks = useTasksStore();
const account = useAccountStore();
const prefs = useSettingsStore();
const search = useSearchStore();
const playlist = usePlaylistStore();
const player = usePlayerStore();
const toast = useToastStore();
const router = useRouter();

/** 点了下载之后待选音质的目标：搜索结果 或 分享链接解析结果 */
type SheetTarget =
  | { kind: "hit"; hit: SearchHit }
  | { kind: "link"; id: string; title: string; artist: string };

const sheetTarget = ref<SheetTarget | null>(null);
const loading = ref(false);
const actionError = ref<string | null>(null);
const notice = ref<string | null>(null);
const busyId = ref<string | null>(null);
const recent = ref<RecentPlaylist[]>([]);

// B 站多 P 选集
const biliTarget = ref<SearchHit | null>(null);
const biliPageList = ref<BiliPage[]>([]);
const biliSelected = ref<Set<string>>(new Set());
const biliBusy = ref(false);
const ui = useUiStore();

watch(
  () => biliTarget.value,
  (target) => ui.setSheetOpen(!!target),
);
onUnmounted(() => ui.setSheetOpen(false));

/** 多源聚合：spinner 之外只影响下一次搜索 */
const SOURCE_META: Record<string, { label: string; badge: string; hint?: string }> = {
  netease: { label: "网易云", badge: "bg-[#c62f2f] text-white" },
  // QQ 只有匿名通道：部分免费曲目可下，VIP / 版权曲必须登录
  qq: { label: "QQ音乐", badge: "bg-[#31c27c] text-white", hint: "需登录" },
  bilibili: { label: "B站", badge: "bg-[#fb7299] text-white" },
  navidrome: { label: "Navidrome", badge: "bg-[#4a90d9] text-white" },
  direct: { label: "直链", badge: "bg-surface-highest text-on-surface" },
};

const sources = [
  { key: "all", label: "全部" },
  { key: "netease", label: "网易云" },
  { key: "qq", label: "QQ" },
  { key: "bilibili", label: "B站" },
  { key: "navidrome", label: "Navidrome" },
] as const;

const selectedSource = ref<string>("all");

function sourceMeta(source: string): { label: string; badge: string; hint?: string } {
  return (
    SOURCE_META[source] ?? {
      label: source,
      badge: "bg-surface-highest text-on-surface",
    }
  );
}

/** 目前只有网易云接了试听代理；其他源点 ▶ 只会拿到错误，不如当场说明 */
const PREVIEW_SOURCES = new Set<string>(["netease"]);

/**
 * 单源失败的展示文案。
 * Rust 侧已经把 HTTP 状态 / 响应片段带在 message 里，这里只做前缀清理与截断，
 * 避免一条 500 字的网络错误把整个搜索页撑爆。
 */
function friendlyFailure(failure: SourceFailure): string {
  const message = failure.message.replace(/^网络错误[:：]\s*/, "");

  if (failure.source === "qq") {
    if (/登录|VIP|104003|风控/.test(message)) {
      return "该曲目需登录或为 VIP，匿名通道不可用";
    }
    if (/超时|timeout/i.test(message)) return "接口超时，已跳过";
  }

  const compact = message.replace(/\s+/g, " ").trim();
  return compact.length > 96 ? `${compact.slice(0, 96)}…` : compact;
}

/** 网易云分享链接特征（含短链） */
const NET_PATTERN = /music\.163\.com|163cn\.tv|y\.music\.163\.com/i;
/** 歌单 / 收藏夹链接：优先于单曲链接识别 */
const PLAYLIST_PATTERN =
  /music\.163\.com[^\s]*(playlist|toplist)|163cn\.tv|space\.bilibili\.com\/\d+\/favlist|bilibili\.com[^\s]*favlist|b23\.tv/i;
const urlPattern = /https?:\/\/\S+/i;

const directUrl = computed(() => search.keyword.match(urlPattern)?.[0] ?? "");
const isPlaylistLink = computed(
  () => !!directUrl.value && PLAYLIST_PATTERN.test(directUrl.value),
);
const playlistUrl = computed(() => (isPlaylistLink.value ? directUrl.value : ""));
const isNeteaseLink = computed(
  () =>
    !!directUrl.value &&
    NET_PATTERN.test(directUrl.value) &&
    !isPlaylistLink.value,
);
const isOtherLink = computed(
  () => !!directUrl.value && !isNeteaseLink.value && !isPlaylistLink.value,
);

/** 从分享链接里抠歌曲 id：?id=123 / /song/123 / 末尾长数字 */
function extractSongId(url: string): string | null {
  const byQuery = url.match(/[?&#]id=(\d+)/i);
  if (byQuery) return byQuery[1];
  const byPath = url.match(/\/song\/(\d+)/i);
  if (byPath) return byPath[1];
  const tail = url.match(/(\d{5,})/);
  return tail ? tail[1] : null;
}

function flash(message: string) {
  notice.value = message;
  setTimeout(() => {
    if (notice.value === message) notice.value = null;
  }, 2600);
}

function notifyFallback(fallback: QualityFallback | null) {
  if (!fallback) return;
  const message = fallbackToast(fallback);
  if (message) toast.show(message.text, message.tone);
}

async function runSearch() {
  const kw = search.keyword.trim();
  if (!kw) return;
  if (isPlaylistLink.value) {
    await openPlaylistLink();
    return;
  }
  if (isNeteaseLink.value) {
    flash("这是分享链接，直接用下方直达卡片");
    return;
  }

  loading.value = true;
  search.error = null;
  search.failures = [];
  search.searched = true;
  search.results = [];
  try {
    const aggregate = await searchSources(
      kw,
      selectedSource.value === "all" ? ["all"] : [selectedSource.value],
    );
    search.results = aggregate.hits;
    // 单源失败不影响其他源结果：按源分行展示，不拼成一条长错误
    search.failures = aggregate.errors;
  } catch (e) {
    search.error = describeError(e);
  } finally {
    loading.value = false;
  }
}

/** X：清空输入与结果，重置整页状态 */
function clearSearch() {
  search.reset();
  sheetTarget.value = null;
  actionError.value = null;
  notice.value = null;
  busyId.value = null;
}

/** 下载按钮不再直接入队：先弹音质；QQ 没有六档音质，走匿名专用通道 */
async function openQualitySheet(hit: SearchHit) {
  if (busyId.value) return;

  if (hit.source === "qq") {
    void downloadQq(hit);
    return;
  }

  // B 站先查分 P；多 P 弹选集，单 P 继续走普通音质弹窗
  if (hit.source === "bilibili") {
    busyId.value = hit.id;
    try {
      const pages = await bilibiliPages(hit.id);
      if (pages.length > 1) {
        biliTarget.value = hit;
        biliPageList.value = pages;
        biliSelected.value = new Set(pages.map((page) => page.cid));
        return;
      }
    } catch (e) {
      actionError.value = describeError(e);
      return;
    } finally {
      busyId.value = null;
    }
  }

  sheetTarget.value = { kind: "hit", hit };
}

function closeBiliSheet() {
  if (biliBusy.value) return;
  biliTarget.value = null;
  biliPageList.value = [];
  biliSelected.value = new Set();
}

function toggleBiliPage(cid: string) {
  const next = new Set(biliSelected.value);
  if (next.has(cid)) next.delete(cid);
  else next.add(cid);
  biliSelected.value = next;
}

function toggleAllBili() {
  if (biliSelected.value.size === biliPageList.value.length) {
    biliSelected.value = new Set();
  } else {
    biliSelected.value = new Set(biliPageList.value.map((page) => page.cid));
  }
}

async function confirmBiliPages() {
  const hit = biliTarget.value;
  if (!hit || biliBusy.value || biliSelected.value.size === 0) return;

  const chosen = biliPageList.value.filter((page) => biliSelected.value.has(page.cid));
  biliBusy.value = true;
  let ok = 0;
  let failed = 0;

  // 2 并发：B 站 playurl 接口对突发请求比较敏感
  const queue = [...chosen];
  const worker = async () => {
    while (queue.length > 0) {
      const page = queue.shift();
      if (!page) break;
      try {
        await tasks.startSource(
          "bilibili",
          {
            id: `${hit.id}:${page.cid}`,
            title: `${hit.title} - ${page.title}`,
            artist: hit.artist,
          },
          prefs.settings.defaultQuality,
        );
        ok += 1;
      } catch {
        failed += 1;
      }
    }
  };

  try {
    await Promise.all(Array.from({ length: Math.min(2, chosen.length) }, () => worker()));
  } finally {
    biliBusy.value = false;
  }

  biliTarget.value = null;
  biliPageList.value = [];
  biliSelected.value = new Set();

  if (ok > 0) {
    toast.show(`已入队 ${ok} 个分P${failed ? `，失败 ${failed} 个` : ""}`);
    await router.push("/tasks");
  } else {
    toast.show(`分P入队失败 ${failed} 个`, "warning");
  }
}

/**
 * QQ 匿名下载：vkey 只给 128K，且 VIP / 版权曲会返回空 purl。
 * 失败时不跳任务页，用黄色 Toast + 行内文案把"需登录"说清楚。
 */
async function downloadQq(hit: SearchHit) {
  busyId.value = hit.id;
  actionError.value = null;
  try {
    const result = await tasks.startSource("qq", hit, "standard");
    notifyFallback(result.fallback);
    toast.show("QQ 音乐匿名仅提供 128K，已按标准音质入队", "default");
    await router.push("/tasks");
  } catch (e) {
    const message = describeError(e);
    actionError.value = /登录|VIP|104003|Auth/i.test(message)
      ? "QQ 音乐需登录：该曲目匿名不可下载，请换用网易云 / B站"
      : `QQ 音乐下载失败：${message}`;
    toast.show("QQ 音乐需登录或该曲目受限，暂不可下载", "warning");
  } finally {
    busyId.value = null;
  }
}

/** 直达卡片：解析出 id 后同样先弹音质 */
function openDirectSheet() {
  if (busyId.value) return;
  const id = extractSongId(directUrl.value);
  if (!id) {
    actionError.value = "没能从链接里解析出歌曲 id";
    return;
  }
  sheetTarget.value = { kind: "link", id, title: `网易云 ${id}`, artist: "网易云" };
}

/** 歌单 / 收藏夹：先预检元数据，再进独立歌单页 */
async function openPlaylistLink() {
  if (busyId.value) return;
  const url = playlistUrl.value;
  if (!url) return;

  busyId.value = "playlist";
  actionError.value = null;
  try {
    await playlist.open(url);
    await router.push("/playlist");
  } catch (e) {
    actionError.value = describeError(e);
  } finally {
    busyId.value = null;
  }
}

/** 弹窗选中后：default 解析成设置里的具体档位，再走同一条入队链路 */
async function applyQuality(picked: NeteaseQuality) {
  const target = sheetTarget.value;
  sheetTarget.value = null;
  if (!target) return;

  const quality: ConcreteQuality = picked === "default" ? prefs.settings.defaultQuality : picked;
  const item =
    target.kind === "hit"
      ? { id: target.hit.id, title: target.hit.title, artist: target.hit.artist }
      : { id: target.id, title: target.title, artist: target.artist };

  busyId.value = item.id;
  actionError.value = null;
  try {
    const result =
      target.kind === "hit"
        ? await tasks.startSource(target.hit.source, item, quality)
        : await tasks.startNetease(item, quality);
    notifyFallback(result.fallback);
    // 入队成功直接切到任务页，用户立刻看到进度
    await router.push("/tasks");
  } catch (e) {
    actionError.value = describeError(e);
  } finally {
    busyId.value = null;
  }
}

/** 试听：整张结果列表入队，上一首/下一首在列表里走 */
async function playHit(index: number) {
  const hit = search.results[index];
  if (!hit) return;

  // QQ / B站 / Navidrome 还没接试听代理，直接调用只会拿到跨源错误
  if (!PREVIEW_SOURCES.has(hit.source)) {
    toast.show(
      hit.source === "qq"
        ? "QQ 音乐试听需登录，暂不支持；可先下载免费曲目"
        : "该音源暂不支持在线试听",
      "warning",
    );
    return;
  }

  actionError.value = null;
  try {
    // 只在同源结果里排队，避免上一首/下一首切到无法解析的音源
    const queue = search.results.filter((item) => item.source === hit.source);
    const at = queue.findIndex((item) => item.id === hit.id);
    const fallback = await player.playFrom(queue, at);
    notifyFallback(fallback);
  } catch (e) {
    actionError.value = describeError(e);
  }
}

/** 普通直链：交给原有的直链下载 */
async function downloadDirectUrl() {
  busyId.value = directUrl.value;
  actionError.value = null;
  try {
    await addDirectDownloadTask(directUrl.value);
    await tasks.refresh();
    await router.push("/tasks");
  } catch (e) {
    actionError.value = describeError(e);
  } finally {
    busyId.value = null;
  }
}

async function loadRecent() {
  try {
    recent.value = await recentPlaylists(6);
  } catch {
    // 浏览器预览 / 首次启动没有最近歌单，不打扰用户
    recent.value = [];
  }
}

async function openRecent(item: RecentPlaylist) {
  playlist.openRecent(item);
  actionError.value = null;
  await router.push("/playlist");
}

onMounted(() => {
  void account.refresh();
  void loadRecent();
});

onActivated(() => {
  void loadRecent();
});
</script>

<template>
  <div class="flex w-full flex-col gap-3">
    <SCard>
    <div class="flex items-center gap-2">
      <div class="relative min-w-0 flex-1">
        <input
          v-model="search.keyword"
          type="search"
          enterkeyhint="search"
          class="w-full rounded-pill bg-surface-low py-3 pl-4 pr-11 text-sm outline-none placeholder:text-on-surface-variant/60"
          placeholder="搜索歌曲 / 歌手，或粘贴歌单、分享链接"
          spellcheck="false"
          @keyup.enter="runSearch"
        />
        <button
          v-if="search.keyword"
          type="button"
          class="absolute right-1.5 top-1/2 grid size-8 -translate-y-1/2 place-items-center rounded-pill text-on-surface-variant"
          aria-label="清空搜索"
          @click="clearSearch"
        >
          <svg class="size-4" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
            <path d="M18.3 5.7 12 12l6.3 6.3-1.4 1.4L10.6 13.4 4.3 19.7 2.9 18.3 9.2 12 2.9 5.7 4.3 4.3l6.3 6.3 6.3-6.3z" />
          </svg>
        </button>
      </div>
      <button
        type="button"
        class="shrink-0 rounded-pill bg-primary px-5 py-3 text-xs font-medium text-on-primary disabled:opacity-50"
        :disabled="loading || !search.keyword.trim()"
        @click="runSearch"
      >
        {{ loading ? "搜索中" : "搜索" }}
      </button>
    </div>

    <div class="mt-3 flex flex-wrap items-center gap-2">
      <button
        v-for="item in sources"
        :key="item.key"
        type="button"
        class="rounded-pill px-3.5 py-1.5 text-[11px] font-medium transition"
        :class="
          selectedSource === item.key
            ? 'bg-primary-container text-on-primary-container'
            : 'bg-surface-high text-on-surface-variant'
        "
        @click="selectedSource = item.key"
      >
        {{ item.label }}
      </button>
    </div>

    <div v-if="!account.loggedIn" class="mt-3 text-[11px] text-on-surface-variant">
      未登录：无损及以上会自动降级到极高 320K
    </div>
    <div
      v-if="selectedSource === 'qq'"
      class="mt-3 rounded-card bg-surface-low px-3 py-2 text-[11px] leading-relaxed text-on-surface-variant"
    >
      QQ 音乐匿名仅支持部分免费曲目（128K）；VIP / 版权曲需登录，搜索无果时会自动回退到建议接口。
    </div>
  </SCard>

  <!-- 我的歌单入口：搜索框正下方 -->
  <button
    type="button"
    class="glass-card flex w-full items-center gap-3 rounded-card p-3 text-left"
    @click="router.push('/my-playlists')"
  >
    <div class="grid size-10 shrink-0 place-items-center rounded-pill bg-primary-container text-on-primary-container">
      <svg class="size-5" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
        <path d="M4 6h16v2H4V6zm0 5h10v2H4v-2zm0 5h10v2H4v-2zm14-1v5.5a2.5 2.5 0 1 1-2-2.45V12h4v3h-2z" />
      </svg>
    </div>
    <p class="min-w-0 flex-1 text-sm font-medium">我的歌单</p>
    <span class="text-lg leading-none text-on-surface-variant" aria-hidden="true">›</span>
  </button>

  <!-- 最近歌单：只存元数据，点击直接进详情 -->
  <div v-if="recent.length">
    <div class="flex items-center justify-between px-1">
      <p class="text-[11px] font-medium">最近歌单</p>
      <p class="text-[10px] text-on-surface-variant">点击继续</p>
    </div>
    <div class="mt-2 flex gap-2 overflow-x-auto pb-1">
      <button
        v-for="item in recent"
        :key="`${item.source}:${item.id}`"
        type="button"
        class="glass-card flex w-40 shrink-0 items-center gap-2 rounded-card p-2 text-left"
        @click="openRecent(item)"
      >
        <img
          v-if="item.coverUrl"
          :src="item.coverUrl"
          alt=""
          referrerpolicy="no-referrer"
          class="size-9 shrink-0 rounded-card object-cover"
        />
        <div
          v-else
          class="grid size-9 shrink-0 place-items-center rounded-card bg-surface-highest text-on-surface-variant"
        >
          <span class="text-xs">♫</span>
        </div>
        <div class="min-w-0 flex-1">
          <p class="truncate text-[11px] font-medium">{{ item.name }}</p>
          <p class="text-[9px] text-on-surface-variant">{{ item.total }} 首</p>
        </div>
      </button>
    </div>
  </div>

  <!-- 歌单 / 收藏夹：先预检元数据再进列表 -->
  <SCard v-if="isPlaylistLink" tone="low">
    <div class="flex items-center gap-3">
      <div class="grid size-11 shrink-0 place-items-center rounded-pill bg-surface-highest">♫</div>
      <div class="min-w-0 flex-1">
        <p class="text-xs font-medium">歌单 / 收藏夹</p>
        <p class="truncate text-[10px] text-on-surface-variant">{{ playlistUrl }}</p>
      </div>
      <button
        type="button"
        class="shrink-0 rounded-pill bg-primary px-4 py-2 text-xs font-medium text-on-primary disabled:opacity-50"
        :disabled="!!busyId"
        @click="openPlaylistLink"
      >
        {{ busyId ? "预检中…" : "打开歌单" }}
      </button>
    </div>
  </SCard>

  <!-- 直达解析卡片 -->
  <SCard v-if="isNeteaseLink || isOtherLink" tone="low">
    <div class="flex items-center gap-3">
      <div class="grid size-11 shrink-0 place-items-center rounded-pill bg-surface-highest">🔗</div>
      <div class="min-w-0 flex-1">
        <p class="text-xs font-medium">{{ isNeteaseLink ? "网易云分享链接" : "直链地址" }}</p>
        <p class="truncate text-[10px] text-on-surface-variant">{{ directUrl }}</p>
      </div>
      <button
        type="button"
        class="shrink-0 rounded-pill bg-primary px-4 py-2 text-xs font-medium text-on-primary disabled:opacity-50"
        :disabled="!!busyId"
        @click="isNeteaseLink ? openDirectSheet() : downloadDirectUrl()"
      >
        {{ busyId ? "解析中…" : "直链下载" }}
      </button>
    </div>
  </SCard>

  <!-- 动作失败：下载 / 解析 / 试听 -->
  <SCard v-if="actionError" tone="low">
    <p class="text-xs leading-relaxed text-error">{{ actionError }}</p>
  </SCard>

  <!-- 单源失败：按音源分行，不阻塞其他源结果 -->
  <SCard v-else-if="search.failures.length" tone="low">
    <p class="text-[11px] font-medium">部分音源没有返回</p>
    <div class="mt-2 space-y-1.5">
      <p
        v-for="failure in search.failures"
        :key="failure.source"
        class="text-[11px] leading-relaxed text-on-surface-variant"
      >
        <span class="font-medium text-on-surface">{{ sourceMeta(failure.source).label }}</span>
        · {{ friendlyFailure(failure) }}
      </p>
    </div>
  </SCard>

  <p v-else-if="search.error" class="px-1 text-xs text-error">{{ search.error }}</p>
  <p v-else-if="notice" class="px-1 text-xs text-on-surface-variant">{{ notice }}</p>

  <!-- 骨架屏 -->
  <div v-if="loading" class="flex flex-col gap-3">
    <SCard v-for="n in 4" :key="n" class="animate-pulse">
      <div class="flex items-center gap-3">
        <div class="size-14 shrink-0 rounded-card bg-surface-highest" />
        <div class="flex-1 space-y-2">
          <div class="h-3 w-2/3 rounded-pill bg-surface-highest" />
          <div class="h-2.5 w-1/2 rounded-pill bg-surface-highest" />
        </div>
      </div>
    </SCard>
  </div>

  <!-- 空状态 -->
  <SCard
    v-else-if="
      search.searched &&
      search.results.length === 0 &&
      !search.error &&
      !search.failures.length
    "
    class="items-center text-center"
  >
    <div class="grid size-14 place-items-center rounded-pill bg-surface-highest text-xl">🎧</div>
    <p class="mt-3 text-sm font-medium">没有找到结果</p>
    <p class="mt-1 text-xs text-on-surface-variant">换个关键词，或直接粘贴网易云分享链接</p>
  </SCard>

  <!-- 结果卡片 -->
  <SCard v-for="(hit, index) in search.results" :key="hit.id" class="flex items-center gap-3">
    <img
      v-if="hit.coverUrl"
      :src="hit.coverUrl"
      alt=""
      referrerpolicy="no-referrer"
      class="size-14 shrink-0 rounded-card object-cover"
    />
    <div
      v-else
      class="grid size-14 shrink-0 place-items-center rounded-card bg-surface-highest text-on-surface-variant"
    >
      <span class="text-[10px]">♪</span>
    </div>

    <div class="min-w-0 flex-1">
      <div class="flex items-center gap-1.5">
        <p class="truncate text-sm font-medium">{{ hit.title }}</p>
        <span
          class="shrink-0 rounded-pill px-1.5 py-0.5 text-[9px] font-medium"
          :class="sourceMeta(hit.source).badge"
        >
          {{ sourceMeta(hit.source).label }}
        </span>
        <span
          v-if="sourceMeta(hit.source).hint"
          class="shrink-0 rounded-pill bg-surface-highest px-1.5 py-0.5 text-[9px] text-on-surface-variant"
        >
          {{ sourceMeta(hit.source).hint }}
        </span>
      </div>
      <p class="truncate text-xs text-on-surface-variant">{{ hit.artist }}</p>
      <p class="truncate text-[11px] text-on-surface-variant/70">
        {{ hit.album || "—" }}
        <template v-if="hit.durationMs"> · {{ formatDuration(hit.durationMs) }}</template>
      </p>
    </div>

    <button
      type="button"
      class="grid size-10 shrink-0 place-items-center rounded-pill bg-surface-high text-on-surface disabled:opacity-40"
      :disabled="player.loading && player.track?.id === hit.id"
      :aria-label="`试听 ${hit.title}`"
      @click="playHit(index)"
    >
      <span class="text-xs leading-none">
        {{
          player.loading && player.track?.id === hit.id
            ? "…"
            : hit.source === "qq"
              ? "🔒"
              : "▶"
        }}
      </span>
    </button>

    <button
      type="button"
      class="grid size-10 shrink-0 place-items-center rounded-pill bg-primary text-on-primary disabled:opacity-40"
      :disabled="!!busyId"
      :aria-label="`下载 ${hit.title}`"
      @click="openQualitySheet(hit)"
    >
      <span class="text-base leading-none">{{ busyId === hit.id ? "…" : "↓" }}</span>
    </button>
  </SCard>

  <!-- 音质选择：搜索结果与分享链接共用同一个弹窗 -->
  <QualitySheet
    :open="!!sheetTarget"
    :default-quality="prefs.settings.defaultQuality"
    @close="sheetTarget = null"
    @pick="applyQuality"
  />

  <!-- B 站多 P 选集 -->
  <div
    v-if="biliTarget"
    class="fixed inset-0 z-50 flex items-end bg-black/50"
    @click.self="closeBiliSheet"
  >
    <div class="glass-sheet max-h-[85vh] w-full overflow-y-auto rounded-t-card p-5 pb-safe shadow-e3">
      <div class="mx-auto mb-3 h-1 w-10 rounded-pill bg-outline-variant" />

      <p class="text-sm font-medium">选择分P</p>
      <p class="mt-0.5 truncate text-xs text-on-surface-variant">{{ biliTarget.title }}</p>

      <div class="mt-3 flex items-center gap-2">
        <button
          type="button"
          class="rounded-pill bg-surface-high px-3 py-1.5 text-[11px] text-on-surface-variant"
          @click="toggleAllBili"
        >
          {{ biliSelected.size === biliPageList.length ? "取消全选" : "全选" }}
        </button>
        <span class="text-[11px] text-on-surface-variant">
          已选 {{ biliSelected.size }} / {{ biliPageList.length }}
        </span>
      </div>

      <div class="mt-3 flex flex-col gap-2">
        <button
          v-for="page in biliPageList"
          :key="page.cid"
          type="button"
          class="flex items-center gap-3 rounded-card px-3 py-2.5 text-left"
          :class="
            biliSelected.has(page.cid)
              ? 'bg-primary-container text-on-primary-container'
              : 'bg-surface-low text-on-surface'
          "
          @click="toggleBiliPage(page.cid)"
        >
          <span
            class="grid size-5 shrink-0 place-items-center rounded-pill border"
            :class="
              biliSelected.has(page.cid)
                ? 'border-primary bg-primary text-on-primary'
                : 'border-outline-variant/60'
            "
          >
            <svg
              v-if="biliSelected.has(page.cid)"
              class="size-3"
              viewBox="0 0 24 24"
              fill="currentColor"
              aria-hidden="true"
            >
              <path d="M9 16.2 4.8 12l-1.4 1.4L9 19 21 7l-1.4-1.4z" />
            </svg>
          </span>
          <span class="w-7 shrink-0 font-mono text-[10px] text-on-surface-variant">
            P{{ page.page }}
          </span>
          <span class="min-w-0 flex-1 truncate text-xs font-medium">{{ page.title }}</span>
          <span class="shrink-0 font-mono text-[10px] text-on-surface-variant">
            {{ formatDuration(page.durationMs) }}
          </span>
        </button>
      </div>

      <div class="mt-4 flex gap-2">
        <button
          type="button"
          class="rounded-pill bg-surface-high px-4 py-2.5 text-xs text-on-surface-variant disabled:opacity-50"
          :disabled="biliBusy"
          @click="closeBiliSheet"
        >
          取消
        </button>
        <button
          type="button"
          class="flex-1 rounded-pill bg-primary px-4 py-2.5 text-xs font-medium text-on-primary disabled:opacity-50"
          :disabled="biliBusy || biliSelected.size === 0"
          @click="confirmBiliPages"
        >
          {{ biliBusy ? "入队中…" : `下载选中 ${biliSelected.size} 个分P` }}
        </button>
      </div>
    </div>
  </div>
  </div>
</template>
