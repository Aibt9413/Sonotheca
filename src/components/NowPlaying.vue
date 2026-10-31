<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { usePlayerStore } from "@/stores/player";
import { useToastStore } from "@/stores/toast";
import { formatClock, parseLrc } from "@/utils/lrc";
import { fallbackToast } from "@/utils/quality";

/**
 * 全屏播放器（Now Playing）。
 *
 * 背景用封面自身做大半径高斯模糊 + 主题色半透明蒙层，等价于"取主色"的
 * MD3 质感，但不需要 canvas 跨域取色。
 */
const player = usePlayerStore();
const toast = useToastStore();

const lyricsBox = ref<HTMLElement | null>(null);
const bar = ref<HTMLElement | null>(null);
const seeking = ref(false);

const cover = computed(() => player.track?.coverUrl ?? null);
const title = computed(() => player.track?.title ?? "");
const artist = computed(() => player.track?.artist ?? "");
const lines = computed(() => parseLrc(player.lyrics));

/** 当前行：最后一个 time <= 播放时间（+0.2s 容差） */
const activeIndex = computed(() => {
  const now = player.currentTime + 0.2;
  let index = -1;
  for (let i = 0; i < lines.value.length; i += 1) {
    if (lines.value[i].time <= now) index = i;
    else break;
  }
  return index;
});

watch(activeIndex, (index) => {
  const box = lyricsBox.value;
  if (!box || index < 0) return;

  const line = box.querySelector<HTMLElement>(`[data-line="${index}"]`);
  if (!line) return;

  const top = line.offsetTop - box.clientHeight / 2 + line.clientHeight / 2;
  box.scrollTo({ top, behavior: "smooth" });
});

watch(
  () => player.track?.id,
  () => {
    lyricsBox.value?.scrollTo({ top: 0 });
  },
);

function ratioFrom(clientX: number): number {
  const el = bar.value;
  if (!el) return 0;
  const rect = el.getBoundingClientRect();
  if (rect.width <= 0) return 0;
  return Math.min(1, Math.max(0, (clientX - rect.left) / rect.width));
}

function seekToEvent(clientX: number) {
  player.seek(ratioFrom(clientX) * (player.duration || 0));
}

function onPointerDown(event: PointerEvent) {
  seeking.value = true;
  (event.currentTarget as HTMLElement).setPointerCapture(event.pointerId);
  seekToEvent(event.clientX);
}

function onPointerMove(event: PointerEvent) {
  if (seeking.value) seekToEvent(event.clientX);
}

function onPointerUp() {
  seeking.value = false;
}

async function step(direction: "prev" | "next") {
  try {
    const fallback = direction === "next" ? await player.next() : await player.prev();
    if (fallback) {
      const message = fallbackToast(fallback);
      if (message) toast.show(message.text, message.tone);
    }
  } catch {
    if (player.error) toast.show(player.error, "warning");
  }
}
</script>

<template>
  <Transition
    enter-active-class="transition duration-300 ease-out"
    enter-from-class="translate-y-full"
    enter-to-class="translate-y-0"
    leave-active-class="transition duration-200 ease-in"
    leave-from-class="translate-y-0"
    leave-to-class="translate-y-full"
  >
    <div
      v-if="player.expanded && player.track"
      class="fixed inset-0 z-40 flex flex-col overflow-hidden bg-surface"
    >
      <!-- 高斯模糊封面背景 + 主题色蒙层 -->
      <div class="pointer-events-none absolute inset-0">
        <img
          v-if="cover"
          :src="cover"
          alt=""
          referrerpolicy="no-referrer"
          class="absolute left-1/2 top-1/2 h-[140%] w-[140%] -translate-x-1/2 -translate-y-1/2 object-cover opacity-70 blur-3xl"
        />
        <div class="absolute inset-0 bg-surface/70 backdrop-blur-2xl" />
        <div class="absolute inset-0 bg-gradient-to-b from-surface/10 via-surface/60 to-surface" />
      </div>

      <div class="relative z-10 px-3 pt-safe">
        <header
          class="glass glass-edge flex items-center justify-between rounded-pill px-2 py-1.5 shadow-e1"
        >
          <button
            type="button"
            class="grid size-10 place-items-center rounded-pill text-on-surface-variant"
            aria-label="收起播放器"
            @click="player.collapse()"
          >
            <svg class="size-7" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
              <path d="M7.4 8.6 12 13.2l4.6-4.6L18 10l-6 6-6-6z" />
            </svg>
          </button>
          <p class="text-[11px] tracking-[0.2em] text-on-surface-variant">正在播放</p>
          <span class="size-10" />
        </header>
      </div>

      <main class="relative flex min-h-0 flex-1 flex-col px-6">
        <div class="mt-1 flex flex-col items-center">
          <img
            v-if="cover"
            :src="cover"
            alt=""
            referrerpolicy="no-referrer"
            class="aspect-square w-full max-w-[min(68vw,17rem)] rounded-card object-cover shadow-e3"
          />
          <div
            v-else
            class="grid aspect-square w-full max-w-[min(68vw,17rem)] place-items-center rounded-card bg-surface-highest text-4xl text-on-surface-variant"
          >
            ♪
          </div>

          <p class="mt-4 max-w-full truncate text-lg font-semibold">{{ title }}</p>
          <p class="mt-0.5 max-w-full truncate text-xs text-on-surface-variant">{{ artist }}</p>
        </div>

        <!-- 歌词 -->
        <div ref="lyricsBox" class="relative mt-3 min-h-0 flex-1 overflow-y-auto">
          <div class="flex flex-col items-center gap-3 px-2 py-12">
            <p v-if="lines.length === 0" class="text-xs text-on-surface-variant">
              {{ player.lyricsLoading ? "歌词加载中…" : "暂无歌词" }}
            </p>

            <button
              v-for="(line, index) in lines"
              :key="`${index}-${line.time}`"
              type="button"
              :data-line="index"
              class="max-w-full text-center text-sm leading-relaxed transition-all duration-300"
              :class="
                index === activeIndex
                  ? 'scale-105 font-semibold text-primary'
                  : 'text-on-surface-variant/60'
              "
              @click="player.seek(line.time)"
            >
              <template v-if="line.words?.length">
                <span
                  v-for="(word, wordIndex) in line.words"
                  :key="wordIndex"
                  :class="
                    index === activeIndex && player.currentTime + 0.15 >= word.time
                      ? 'text-primary'
                      : ''
                  "
                >{{ word.text }}</span>
              </template>
              <span v-else>{{ line.text }}</span>
            </button>
          </div>
        </div>

        <!-- 进度条（可拖动） -->
        <div class="mt-2">
          <div
            ref="bar"
            class="flex h-9 cursor-pointer items-center"
            @pointerdown="onPointerDown"
            @pointermove="onPointerMove"
            @pointerup="onPointerUp"
            @pointercancel="onPointerUp"
          >
            <div class="relative h-1.5 w-full rounded-pill bg-on-surface/15">
              <div
                class="absolute inset-y-0 left-0 rounded-pill bg-primary"
                :style="{ width: `${player.progress}%` }"
              />
              <div
                class="absolute top-1/2 size-3.5 -translate-x-1/2 -translate-y-1/2 rounded-pill bg-primary shadow-e1"
                :style="{ left: `${player.progress}%` }"
              />
            </div>
          </div>

          <div class="flex justify-between text-[10px] tabular-nums text-on-surface-variant">
            <span>{{ formatClock(player.currentTime) }}</span>
            <span>{{ formatClock(player.duration) }}</span>
          </div>
        </div>

        <!-- 控制 -->
        <div class="flex items-center justify-center gap-10 pb-safe pt-5">
          <button
            type="button"
            class="grid size-12 place-items-center rounded-pill text-on-surface disabled:opacity-30"
            aria-label="上一首"
            :disabled="!player.hasPrev"
            @click="step('prev')"
          >
            <svg class="size-7" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
              <path d="M6 6h2v12H6zm3.5 6 8.5 6V6z" />
            </svg>
          </button>

          <button
            type="button"
            class="grid size-16 place-items-center rounded-pill bg-primary text-on-primary shadow-e2"
            :aria-label="player.playing ? '暂停' : '播放'"
            @click="player.toggle()"
          >
            <svg
              v-if="player.playing"
              class="size-7"
              viewBox="0 0 24 24"
              fill="currentColor"
              aria-hidden="true"
            >
              <path d="M6 5h4v14H6zM14 5h4v14h-4z" />
            </svg>
            <svg v-else class="size-7" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
              <path d="M8 5v14l11-7z" />
            </svg>
          </button>

          <button
            type="button"
            class="grid size-12 place-items-center rounded-pill text-on-surface disabled:opacity-30"
            aria-label="下一首"
            :disabled="!player.hasNext"
            @click="step('next')"
          >
            <svg class="size-7" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
              <path d="M16 6h2v12h-2zM6 6v12l8.5-6z" />
            </svg>
          </button>
        </div>
      </main>
    </div>
  </Transition>
</template>
