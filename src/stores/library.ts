import { defineStore } from "pinia";
import { ref } from "vue";
import { describeError, isTauri, listTracks, type Track } from "@/ipc/bindings";

const PAGE_SIZE = 50;

export const useLibraryStore = defineStore("library", () => {
  const tracks = ref<Track[]>([]);
  const total = ref(0);
  const loading = ref(false);
  const loadingMore = ref(false);
  const hasMore = ref(true);
  const error = ref<string | null>(null);

  async function refresh() {
    if (!isTauri()) return;
    loading.value = true;
    loadingMore.value = false;
    error.value = null;
    try {
      const page = await listTracks(PAGE_SIZE, 0);
      tracks.value = page.tracks;
      total.value = page.total;
      hasMore.value = page.tracks.length < page.total;
    } catch (e) {
      error.value = describeError(e);
    } finally {
      loading.value = false;
    }
  }

  async function loadMore() {
    if (!isTauri() || loading.value || loadingMore.value || !hasMore.value) return;
    loadingMore.value = true;
    error.value = null;
    try {
      const page = await listTracks(PAGE_SIZE, tracks.value.length);
      const existing = new Set(tracks.value.map((track) => track.id));
      tracks.value.push(...page.tracks.filter((track) => !existing.has(track.id)));
      total.value = page.total;
      hasMore.value = tracks.value.length < page.total;
    } catch (e) {
      error.value = describeError(e);
    } finally {
      loadingMore.value = false;
    }
  }

  /** 本地搜索需要全量数据时，顺序补齐所有分页 */
  async function loadAll() {
    while (hasMore.value && !loading.value && !loadingMore.value) {
      const before = tracks.value.length;
      await loadMore();
      if (tracks.value.length === before) break;
    }
  }

  /** 本地立即可见地移除（删除/移动后不必等下一次 refresh） */
  function remove(id: number) {
    const before = tracks.value.length;
    tracks.value = tracks.value.filter((track) => track.id !== id);
    total.value = Math.max(0, total.value - (before - tracks.value.length));
  }

  /** 用 Rust 返回的新 Track 直接替换，保留列表里已有的时长/码率等展示字段 */
  function replace(track: Track) {
    const index = tracks.value.findIndex((item) => item.id === track.id);
    if (index < 0) return;
    const previous = tracks.value[index];
    tracks.value[index] = {
      ...previous,
      ...track,
      durationMs: track.durationMs ?? previous.durationMs,
      bitrate: track.bitrate ?? previous.bitrate,
      sizeBytes: track.sizeBytes ?? previous.sizeBytes,
      format: track.format ?? previous.format,
    } as Track;
  }

  return {
    tracks,
    total,
    loading,
    loadingMore,
    hasMore,
    error,
    refresh,
    loadMore,
    loadAll,
    remove,
    replace,
  };
});
