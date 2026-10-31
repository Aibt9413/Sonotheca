<script setup lang="ts">
import { onMounted, ref, watch } from "vue";
import { usePlayerStore } from "@/stores/player";
import { useToastStore } from "@/stores/toast";

/**
 * 迷你播放条：同时是 `<audio>` 的唯一宿主。
 *
 * 注意根节点必须是**单个元素**：App 会往里透传 `pointer-events-auto`，
 * 之前用 fragment（div + audio 两个根）导致透传失败，整条被父级的
 * pointer-events-none 吃掉，按钮看起来"点击无效"。
 */
const player = usePlayerStore();
const toast = useToastStore();

const audioEl = ref<HTMLAudioElement | null>(null);

onMounted(() => {
  player.setAudio(audioEl.value);
});

watch(
  () => player.src,
  async (src) => {
    const el = audioEl.value;
    if (!el) return;

    if (!src) {
      el.pause();
      el.removeAttribute("src");
      return;
    }

    el.src = src;
    el.load();

    if (player.playing) {
      try {
        await el.play();
      } catch {
        player.setPlaying(false);
      }
    }
  },
);

watch(
  () => player.playing,
  async (on) => {
    const el = audioEl.value;
    if (!el || !player.src) return;

    if (on) {
      try {
        await el.play();
      } catch {
        player.setPlaying(false);
      }
    } else {
      el.pause();
    }
  },
);

function onAudioError() {
  if (!player.src) return;
  player.setPlaying(false);
  toast.show("试听失败：音频流无法播放", "warning");
}
</script>

<template>
  <div class="w-full max-w-3xl px-5">
    <div
      v-if="player.track"
      class="glass glass-edge relative overflow-hidden rounded-card shadow-e2"
    >
      <!-- 非按钮区域：展开全屏播放器 -->
      <div class="flex items-center gap-3 p-2.5" @click="player.expand()">
        <img
          v-if="player.track.coverUrl"
          :src="player.track.coverUrl"
          alt=""
          referrerpolicy="no-referrer"
          class="size-11 shrink-0 rounded-card object-cover"
        />
        <div
          v-else
          class="grid size-11 shrink-0 place-items-center rounded-pill bg-surface-highest text-on-surface-variant"
        >
          <span class="text-sm">♪</span>
        </div>

        <div class="min-w-0 flex-1">
          <p class="truncate text-sm font-medium">{{ player.track.title }}</p>
          <p class="truncate text-xs text-on-surface-variant">{{ player.track.artist }}</p>
        </div>

        <button
          type="button"
          class="grid size-10 shrink-0 place-items-center rounded-pill bg-primary text-on-primary"
          :aria-label="player.playing ? '暂停' : '播放'"
          @click.stop="player.toggle()"
        >
          <svg
            v-if="player.playing"
            class="size-5"
            viewBox="0 0 24 24"
            fill="currentColor"
            aria-hidden="true"
          >
            <path d="M6 5h4v14H6zM14 5h4v14h-4z" />
          </svg>
          <svg v-else class="size-5" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
            <path d="M8 5v14l11-7z" />
          </svg>
        </button>

        <button
          type="button"
          class="grid size-9 shrink-0 place-items-center rounded-pill text-on-surface-variant"
          aria-label="关闭试听"
          @click.stop="player.close()"
        >
          <svg class="size-4" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
            <path
              d="M18.3 5.7 12 12l6.3 6.3-1.4 1.4L10.6 13.4 4.3 19.7 2.9 18.3 9.2 12 2.9 5.7 4.3 4.3l6.3 6.3 6.3-6.3z"
            />
          </svg>
        </button>
      </div>

      <!-- 2px 极细进度条 -->
      <div class="pointer-events-none absolute inset-x-0 bottom-0 h-[2px] bg-on-surface/10">
        <div
          class="h-full bg-primary transition-[width] duration-200"
          :style="{ width: `${player.progress}%` }"
        />
      </div>
    </div>

    <audio
      ref="audioEl"
      class="hidden"
      preload="auto"
      @timeupdate="player.onTimeUpdate()"
      @loadedmetadata="player.onTimeUpdate()"
      @durationchange="player.onTimeUpdate()"
      @ended="player.onEnded()"
      @error="onAudioError"
    />
  </div>
</template>
