import { defineStore } from "pinia";
import { computed, ref } from "vue";
import {
  describeError,
  playlistPage,
  playlistPreview,
  type PlaylistPreview,
  type PlaylistTrack,
  type RecentPlaylist,
} from "@/ipc/bindings";

/**
 * 歌单状态。
 *
 * 只保留已经加载进来的曲目；`filtered` 是本地搜索，绝不为搜索拉全量。
 * 分页游标永远等于 `tracks.length`，每页固定 50 首。
 */
export const usePlaylistStore = defineStore("playlist", () => {
  const preview = ref<PlaylistPreview | null>(null);
  const tracks = ref<PlaylistTrack[]>([]);
  const loading = ref(false);
  const loadingMore = ref(false);
  const hasMore = ref(false);
  const query = ref("");
  const error = ref<string | null>(null);
  /** 50 / 100 / 200 / 0（无限制），由下载设置同步 */
  const pageSize = ref(50);

  const filtered = computed(() => {
    const keyword = query.value.trim().toLowerCase();
    if (!keyword) return tracks.value;
    return tracks.value.filter(
      (track) =>
        track.title.toLowerCase().includes(keyword) ||
        track.artist.toLowerCase().includes(keyword),
    );
  });

  async function open(url: string) {
    const next = await playlistPreview(url);
    preview.value = next;
    tracks.value = [];
    loading.value = false;
    loadingMore.value = false;
    hasMore.value = true;
    query.value = "";
    error.value = null;
    return next;
  }

  /** 从“最近歌单”直接进入：不重新预检，复用落库的元数据 */
  function openRecent(item: RecentPlaylist) {
    preview.value = {
      source: item.source,
      id: item.id,
      title: item.name,
      coverUrl: item.coverUrl,
      total: item.total,
      description: null,
    };
    tracks.value = [];
    loading.value = false;
    loadingMore.value = false;
    hasMore.value = true;
    query.value = "";
    error.value = null;
  }

  /** 拉下一页（默认 50）；由页面的 200px / 300ms 防抖逻辑调用 */
  async function loadMore() {
    if (!preview.value || loading.value || loadingMore.value || !hasMore.value) return;

    const first = tracks.value.length === 0;
    loading.value = first;
    loadingMore.value = !first;
    error.value = null;

    try {
      const page = await playlistPage(
        preview.value.source,
        preview.value.id,
        tracks.value.length,
        pageSize.value,
      );

      const seen = new Set(tracks.value.map((track) => `${track.source}:${track.id}`));
      const appended = page.tracks.filter(
        (track) => !seen.has(`${track.source}:${track.id}`),
      );

      tracks.value = [...tracks.value, ...appended];
      hasMore.value = page.hasMore;
      if (page.total > 0) preview.value.total = page.total;
    } catch (e) {
      error.value = describeError(e);
      hasMore.value = false;
    } finally {
      loading.value = false;
      loadingMore.value = false;
    }
  }

  async function retry() {
    hasMore.value = true;
    await loadMore();
  }

  /** 修改分页大小：清空已加载列表，按新 size 从第 0 条重新请求 */
  async function setPageSize(size: number) {
    const next = [50, 100, 200, 0].includes(size) ? size : 50;
    if (next === pageSize.value) return;

    pageSize.value = next;
    tracks.value = [];
    hasMore.value = true;
    query.value = "";
    error.value = null;
    loading.value = false;
    loadingMore.value = false;
    await loadMore();
  }

  function reset() {
    preview.value = null;
    tracks.value = [];
    loading.value = false;
    loadingMore.value = false;
    hasMore.value = false;
    query.value = "";
    error.value = null;
  }

  return {
    preview,
    tracks,
    filtered,
    loading,
    loadingMore,
    hasMore,
    query,
    error,
    pageSize,
    open,
    openRecent,
    loadMore,
    retry,
    setPageSize,
    reset,
  };
});
