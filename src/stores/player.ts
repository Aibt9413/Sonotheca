import { defineStore } from "pinia";
import { computed, ref, shallowRef } from "vue";
import {
  describeError,
  getLyrics,
  neteaseStreamUrl,
  type QualityFallback,
  type SearchHit,
} from "@/ipc/bindings";

export interface PlayerTrack {
  id: string;
  title: string;
  artist: string;
  album?: string;
  coverUrl: string | null;
  lyrics?: string;
}

/**
 * 播放器状态中心。
 *
 * `<audio>` 元素由 MiniPlayer 持有（始终挂载），这里只存引用与状态，
 * 因此全屏页 / 迷你条可以共享同一个播放实例，切页面不断音。
 */
export const usePlayerStore = defineStore("player", () => {
  const queue = ref<PlayerTrack[]>([]);
  const index = ref(-1);

  const src = ref<string | null>(null);
  const playing = ref(false);
  const loading = ref(false);
  const error = ref<string | null>(null);
  const expanded = ref(false);

  const currentTime = ref(0);
  const duration = ref(0);

  const lyrics = ref("");
  const lyricsLoading = ref(false);

  const audio = shallowRef<HTMLAudioElement | null>(null);

  const track = computed<PlayerTrack | null>(() =>
    index.value >= 0 ? (queue.value[index.value] ?? null) : null,
  );
  const hasPrev = computed(() => index.value > 0);
  const hasNext = computed(
    () => index.value >= 0 && index.value < queue.value.length - 1,
  );
  const progress = computed(() =>
    duration.value > 0 ? Math.min(100, (currentTime.value / duration.value) * 100) : 0,
  );

  function setAudio(el: HTMLAudioElement | null) {
    audio.value = el;
  }

  function onTimeUpdate() {
    const el = audio.value;
    if (!el) return;
    currentTime.value = Number.isFinite(el.currentTime) ? el.currentTime : 0;
    duration.value = Number.isFinite(el.duration) ? el.duration : 0;
  }

  function onEnded() {
    if (hasNext.value) {
      void next();
    } else {
      playing.value = false;
      currentTime.value = 0;
    }
  }

  function setPlaying(next: boolean) {
    playing.value = next;
  }

  function toggle() {
    if (!track.value) return;
    playing.value = !playing.value;
  }

  function seek(seconds: number) {
    const el = audio.value;
    if (!el) return;

    const limit =
      Number.isFinite(el.duration) && el.duration > 0 ? el.duration : duration.value;
    const next = Math.max(0, Math.min(seconds, limit || seconds));

    el.currentTime = next;
    currentTime.value = next;
  }

  async function loadLyrics(id: string) {
    lyricsLoading.value = true;
    try {
      lyrics.value = await getLyrics(id, null);
    } catch {
      // 纯音乐 / 无歌词不当作错误
      lyrics.value = "";
    } finally {
      lyricsLoading.value = false;
    }
  }

  async function load(at: number): Promise<QualityFallback | null> {
    if (at < 0 || at >= queue.value.length) return null;

    index.value = at;
    loading.value = true;
    error.value = null;
    src.value = null;
    currentTime.value = 0;
    duration.value = 0;
    lyrics.value = "";

    const item = queue.value[at];

    try {
      const stream = await neteaseStreamUrl(item.id);
      src.value = stream.url;
      playing.value = true;
      void loadLyrics(item.id);
      return stream.fallback;
    } catch (e) {
      error.value = describeError(e);
      playing.value = false;
      throw e;
    } finally {
      loading.value = false;
    }
  }

  function toTrack(hit: SearchHit): PlayerTrack {
    return {
      id: hit.id,
      title: hit.title,
      artist: hit.artist,
      coverUrl: hit.coverUrl,
    };
  }

  /** 从搜索结果入队：上一首 / 下一首就在当前列表里走 */
  async function playFrom(hits: SearchHit[], at: number) {
    queue.value = hits.map(toTrack);
    return load(at);
  }

  async function play(hit: SearchHit) {
    return playFrom([hit], 0);
  }

  /** 音乐库本地文件：走 127.0.0.1 代理直出 */
  function playLocal(url: string, item: PlayerTrack) {
    queue.value = [item];
    index.value = 0;
    loading.value = false;
    error.value = null;
    currentTime.value = 0;
    duration.value = 0;
    lyrics.value = item.lyrics ?? "";
    src.value = url;
    playing.value = true;
  }

  async function next() {
    if (!hasNext.value) return null;
    return load(index.value + 1);
  }

  async function prev() {
    if (!hasPrev.value) return null;
    return load(index.value - 1);
  }

  function expand() {
    if (track.value) expanded.value = true;
  }

  function collapse() {
    expanded.value = false;
  }

  function close() {
    playing.value = false;
    expanded.value = false;
    queue.value = [];
    index.value = -1;
    src.value = null;
    lyrics.value = "";
    currentTime.value = 0;
    duration.value = 0;
  }

  /** 写入标签前释放文件句柄：暂停并卸载当前 src */
  function releaseSource() {
    audio.value?.pause();
    playing.value = false;
    src.value = null;
    currentTime.value = 0;
  }

  return {
    queue,
    index,
    track,
    src,
    playing,
    loading,
    error,
    expanded,
    currentTime,
    duration,
    lyrics,
    lyricsLoading,
    audio,
    hasPrev,
    hasNext,
    progress,
    setAudio,
    onTimeUpdate,
    onEnded,
    setPlaying,
    toggle,
    seek,
    play,
    playFrom,
    playLocal,
    next,
    prev,
    expand,
    collapse,
    close,
    releaseSource,
  };
});
