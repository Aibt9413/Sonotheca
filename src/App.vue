<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from "vue";
import { RouterView, useRoute, useRouter } from "vue-router";
import BottomBar from "@/components/BottomBar.vue";
import MiniPlayer from "@/components/MiniPlayer.vue";
import NowPlaying from "@/components/NowPlaying.vue";
import { useNavigationStore } from "@/stores/navigation";
import { usePlayerStore } from "@/stores/player";
import { useSettingsStore } from "@/stores/settings";
import { useTasksStore } from "@/stores/tasks";
import { useThemeStore } from "@/stores/theme";
import { useUiStore } from "@/stores/ui";
import SToast from "@/ui/SToast.vue";

const tasks = useTasksStore();
const prefs = useSettingsStore();
const theme = useThemeStore();
const nav = useNavigationStore();
const ui = useUiStore();
const player = usePlayerStore();
const route = useRoute();
const router = useRouter();

const pageTitle = computed(() => String(route.meta.title ?? "Sonotheca"));
const isSecondary = computed(() => Number(route.meta.depth ?? 0) > 0);
const parentPath = computed(() => String(route.meta.parent ?? "/"));

const transitionName = computed(() => {
  if (!theme.motion) return "page-off";
  return nav.direction === "back" ? "page-back" : "page-forward";
});

/** 每个路由记住自己的滚动位置：新页面从顶部开始，回到搜索页仍停在原处 */
const scrollHost = ref<HTMLElement | null>(null);
const scrollPositions = new Map<string, number>();
const pendingScroll = ref(0);

watch(
  () => route.path,
  (to, from) => {
    if (from && scrollHost.value) {
      scrollPositions.set(from, scrollHost.value.scrollTop);
    }
    pendingScroll.value = scrollPositions.get(to) ?? 0;
  },
);

/** 等新页面真正进场后再恢复，避免 out-in 期间被旧内容高度夹断 */
function restoreScroll() {
  if (scrollHost.value) scrollHost.value.scrollTop = pendingScroll.value;
}

// 事件监听只在这里挂一次，页面切换不重连
onMounted(() => {
  void tasks.bootstrap();
  void prefs.hydrateQuality();
  document.addEventListener("contextmenu", onContextMenu);
});

onUnmounted(() => {
  tasks.dispose();
  document.removeEventListener("contextmenu", onContextMenu);
});

/**
 * 屏蔽长按弹出的系统菜单（复制/分享/朗读），避免抢占我们的手势。
 * 输入框与可编辑区域放行，否则连粘贴菜单都没了。
 */
function onContextMenu(event: Event) {
  const target = event.target as HTMLElement | null;
  if (!target) return;
  if (target.closest("input, textarea, [contenteditable='true']")) return;
  event.preventDefault();
}
</script>

<template>
  <div
    class="relative flex h-full flex-col text-on-surface"
    :class="theme.wallpaper ? '' : 'bg-surface'"
  >
    <!-- 壁纸层：根元素挂 data-wallpaper + --app-wallpaper，遮罩保证正文可读 -->
    <div
      v-if="theme.wallpaper"
      class="app-wallpaper pointer-events-none fixed inset-0 z-0"
      aria-hidden="true"
    >
      <div class="absolute inset-0 bg-surface/55 backdrop-blur-sm" />
      <div class="absolute inset-0 bg-gradient-to-b from-surface/10 via-surface/40 to-surface/90" />
    </div>

    <header class="relative z-10 flex items-center gap-2 px-5 pb-2 pt-safe">
      <button
        v-if="isSecondary"
        type="button"
        class="grid size-8 shrink-0 place-items-center rounded-pill text-on-surface-variant"
        aria-label="返回"
        @click="router.push(parentPath)"
      >
        <svg class="size-5" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
          <path d="M20 11H7.83l5.59-5.59L12 4l-8 8 8 8 1.41-1.41L7.83 13H20v-2z" />
        </svg>
      </button>
      <p
        class="min-w-0 truncate font-bold tracking-tight"
        :class="isSecondary ? 'text-xl' : 'text-2xl'"
      >
        {{ pageTitle }}
      </p>
    </header>

    <main
      ref="scrollHost"
      class="relative z-10 min-h-0 flex-1 overflow-y-auto overflow-x-hidden px-5 pb-nav"
    >
      <div class="mx-auto flex w-full max-w-3xl flex-col gap-3">
        <RouterView v-slot="{ Component, route }">
          <!-- 页面动效：二级页右进、返回反向；关闭全局动效时用无样式的 page-off -->
          <Transition
            :name="transitionName"
            mode="out-in"
            @after-enter="restoreScroll"
          >
            <!-- 底部 Tab 与设置二级页全部缓存：滚动位置、列表与搜索状态不丢 -->
            <KeepAlive
              :include="[
                'SearchView',
                'LibraryView',
                'TasksView',
                'SettingsView',
                'SourcesView',
                'ThemeSettings',
                'PlaylistView',
                'DownloadSettingsView',
                'MyPlaylistsView',
                'AboutView',
              ]"
            >
              <component :is="Component" :key="route.path" />
            </KeepAlive>
          </Transition>
        </RouterView>
      </div>
    </main>

    <NowPlaying />

    <!-- MiniPlayer 必须常驻：它持有唯一的 <audio>；展开/弹层时只隐藏，不卸载 -->
    <div
      class="pointer-events-none fixed inset-x-0 bottom-0 z-30 flex flex-col items-center gap-2"
    >
      <MiniPlayer
        v-show="!player.expanded && !ui.managerOpen && !ui.sheetOpen"
        class="pointer-events-auto"
      />
      <BottomBar
        v-if="!player.expanded && !ui.managerOpen && !ui.sheetOpen"
        class="pointer-events-auto w-full"
      />
    </div>

    <!-- Toast 单独一层（z-50）：全屏播放器打开时降级提示也必须可见 -->
    <div class="pointer-events-none fixed inset-x-0 bottom-0 z-50 flex justify-center pb-[9rem]">
      <SToast />
    </div>
  </div>
</template>
