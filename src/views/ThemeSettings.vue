<script setup lang="ts">
import { computed, onDeactivated, onUnmounted, ref, watch } from "vue";
import { useRouter } from "vue-router";
import SCard from "@/ui/SCard.vue";
import SSwitch from "@/ui/SSwitch.vue";
import { useBackGuard } from "@/composables/useBackGuard";
import { useThemeStore, type ThemeMode } from "@/stores/theme";
import { useToastStore } from "@/stores/toast";
import { useUiStore } from "@/stores/ui";
import { extractSeedFromDataUrl, prepareWallpaper } from "@/utils/wallpaper";

defineOptions({ name: "ThemeSettings" });

/**
 * 主题外观页（P3）。
 *
 * 所有状态都在 stores/theme.ts：明暗、种子色、壁纸、动效、柔光。
 * 本页只负责交互：上传壁纸 → Canvas 取色 → HCT 生成全局色板。
 */
const router = useRouter();
const theme = useThemeStore();
const toast = useToastStore();
const ui = useUiStore();

useBackGuard(
  () => true,
  () => void router.push("/settings"),
);

const modes: ReadonlyArray<{ key: ThemeMode; label: string }> = [
  { key: "light", label: "浅色" },
  { key: "dark", label: "深色" },
  { key: "system", label: "跟随系统" },
];

const motionModel = computed({
  get: () => theme.motion,
  set: (value: boolean) => theme.setMotion(value),
});

const glassModel = computed({
  get: () => theme.glass,
  set: (value: boolean) => theme.setGlass(value),
});

// ── 壁纸 ───────────────────────────────────────────────────
const busyWallpaper = ref(false);
const wallpaperError = ref<string | null>(null);
const uploadInput = ref<HTMLInputElement | null>(null);

function openFilePicker() {
  uploadInput.value?.click();
}

async function onWallpaperPicked(event: Event) {
  const input = event.target as HTMLInputElement;
  const file = input.files?.[0];
  input.value = "";
  if (!file || busyWallpaper.value) return;

  busyWallpaper.value = true;
  wallpaperError.value = null;
  try {
    const { dataUrl, seed } = await prepareWallpaper(file);
    if (!theme.setWallpaper(dataUrl)) {
      wallpaperError.value = "图片太大，本地存储放不下，请换一张";
      return;
    }
    theme.setSeed(seed);
    toast.show(`已应用壁纸，主色 ${seed}`);
  } catch (e) {
    wallpaperError.value = e instanceof Error ? e.message : "壁纸处理失败";
  } finally {
    busyWallpaper.value = false;
  }
}

function removeWallpaper() {
  theme.clearWallpaper();
  wallpaperError.value = null;
  toast.show("壁纸已移除");
}

// ── 自绘取色 Sheet ─────────────────────────────────────────
const sheetOpen = ref(false);
const hexDraft = ref(theme.seed);
const hexError = ref<string | null>(null);
const reExtracting = ref(false);

watch(sheetOpen, (open) => ui.setSheetOpen(open));
onDeactivated(() => {
  sheetOpen.value = false;
  ui.setSheetOpen(false);
});
onUnmounted(() => ui.setSheetOpen(false));

/** MD3 基线色 + 各音源品牌色，6×4 矩阵 */
const presets = [
  "#6750A4",
  "#625B71",
  "#7D5260",
  "#B3261E",
  "#8C4A60",
  "#6E5676",
  "#5C5F7A",
  "#4A5C92",
  "#0061A4",
  "#006A6A",
  "#00696E",
  "#00BFA5",
  "#31C27C",
  "#3F6212",
  "#006E2C",
  "#7A5900",
  "#F4B400",
  "#8B5000",
  "#FF7043",
  "#FB7299",
  "#C62F2F",
  "#E53935",
  "#4A90D9",
  "#2BD4B4",
] as const;

function openPicker() {
  hexDraft.value = theme.seed;
  hexError.value = null;
  sheetOpen.value = true;
}

function pickPreset(color: string) {
  theme.setSeed(color);
  hexDraft.value = color;
  hexError.value = null;
}

function applyHex() {
  const raw = hexDraft.value.trim().toUpperCase();
  const normalized = raw.startsWith("#") ? raw : `#${raw}`;
  if (!/^#[0-9A-F]{6}$/.test(normalized)) {
    hexError.value = "请输入 #RRGGBB 格式的 6 位色值";
    return;
  }

  theme.setSeed(normalized);
  hexDraft.value = normalized;
  hexError.value = null;
  sheetOpen.value = false;
  toast.show("主色已更新");
}

/** 对已保存的壁纸重新跑一次 Canvas 取色 */
async function extractFromWallpaper() {
  if (!theme.wallpaper || reExtracting.value) return;

  reExtracting.value = true;
  hexError.value = null;
  try {
    const seed = await extractSeedFromDataUrl(theme.wallpaper);
    theme.setSeed(seed);
    hexDraft.value = seed;
    toast.show(`已从壁纸提取主色 ${seed}`);
  } catch (e) {
    hexError.value = e instanceof Error ? e.message : "壁纸取色失败";
  } finally {
    reExtracting.value = false;
  }
}
</script>

<template>
  <div class="flex w-full flex-col gap-3">
    <!-- 明暗模式 -->
    <SCard>
      <p class="text-sm font-medium">明暗模式</p>
      <div class="mt-3 grid grid-cols-3 gap-1 rounded-pill bg-surface-high p-1">
        <button
          v-for="item in modes"
          :key="item.key"
          type="button"
          class="rounded-pill py-2 text-xs font-medium transition"
          :class="
            theme.mode === item.key
              ? 'bg-primary text-on-primary shadow-e1'
              : 'text-on-surface-variant'
          "
          :aria-pressed="theme.mode === item.key"
          @click="theme.setMode(item.key)"
        >
          {{ item.label }}
        </button>
      </div>
    </SCard>

    <!-- 壁纸与主色 -->
    <SCard>
      <div class="flex items-center justify-between gap-3">
        <p class="text-sm font-medium">壁纸</p>
        <span v-if="theme.wallpaper" class="font-mono text-[10px] text-on-surface-variant">
          JPEG · 本机
        </span>
      </div>

      <div class="relative mt-3 aspect-[16/9] overflow-hidden rounded-card bg-surface-low">
        <img
          v-if="theme.wallpaper"
          :src="theme.wallpaper"
          alt="当前壁纸"
          class="size-full object-cover"
        />
        <div v-else class="grid size-full place-items-center text-xs text-on-surface-variant">
          未设置壁纸
        </div>

        <div
          v-if="busyWallpaper"
          class="absolute inset-0 grid place-items-center bg-black/45 text-[11px] text-white backdrop-blur-sm"
        >
          正在压缩并取色…
        </div>
      </div>

      <div class="mt-3 flex items-center gap-2">
        <button
          type="button"
          class="rounded-pill bg-primary-container px-4 py-2 text-xs font-medium text-on-primary-container disabled:opacity-50"
          :disabled="busyWallpaper"
          @click="openFilePicker"
        >
          {{ theme.wallpaper ? "更换图片" : "上传图片" }}
        </button>
        <button
          v-if="theme.wallpaper"
          type="button"
          class="rounded-pill bg-surface-high px-4 py-2 text-xs text-on-surface-variant"
          @click="removeWallpaper"
        >
          移除
        </button>
      </div>

      <input
        ref="uploadInput"
        type="file"
        accept="image/*"
        class="hidden"
        @change="onWallpaperPicked"
      />

      <div class="mt-4 flex items-center gap-3 rounded-card bg-surface-low p-3">
        <span
          class="size-8 shrink-0 rounded-pill border border-outline-variant/40"
          :style="{ backgroundColor: theme.seed }"
          aria-hidden="true"
        />
        <div class="min-w-0 flex-1">
          <p class="text-xs font-medium">主色调</p>
          <p class="font-mono text-[11px] text-on-surface-variant">{{ theme.seed }}</p>
        </div>
        <button
          type="button"
          class="shrink-0 rounded-pill bg-surface-high px-3 py-1.5 text-[11px] text-on-surface-variant"
          @click="openPicker"
        >
          自定义
        </button>
      </div>

      <p v-if="wallpaperError" class="mt-2 text-xs text-error">{{ wallpaperError }}</p>
    </SCard>

    <!-- 体验开关 -->
    <SCard>
      <div class="flex items-center justify-between gap-3">
        <div class="min-w-0">
          <p class="text-sm font-medium">全局动效</p>
          <p class="mt-0.5 text-[11px] text-on-surface-variant">页面转场与组件过渡</p>
        </div>
        <SSwitch v-model="motionModel" />
      </div>

      <div class="my-3 h-px bg-outline-variant/30" />

      <div class="flex items-center justify-between gap-3">
        <p class="text-sm font-medium">全局柔光</p>
        <SSwitch v-model="glassModel" />
      </div>
    </SCard>

    <!-- 自绘 MD3 取色弹窗 -->
    <div
      v-if="sheetOpen"
      class="fixed inset-0 z-50 flex items-end bg-black/50"
      @click.self="sheetOpen = false"
    >
      <div
        class="glass-sheet max-h-[85vh] w-full overflow-y-auto rounded-t-card p-5 pb-safe shadow-e3"
      >
        <div class="mx-auto mb-3 h-1 w-10 rounded-pill bg-outline-variant" />

        <div class="flex items-center justify-between">
          <p class="text-sm font-medium">自定义主色</p>
          <button
            type="button"
            class="rounded-pill px-2 py-1 text-xs text-on-surface-variant"
            @click="sheetOpen = false"
          >
            关闭
          </button>
        </div>

        <p class="mt-1 font-mono text-[11px] text-on-surface-variant">
          当前 {{ theme.seed }}
        </p>

        <button
          v-if="theme.wallpaper"
          type="button"
          class="mt-4 w-full rounded-pill bg-surface-high py-2.5 text-xs font-medium text-on-surface-variant disabled:opacity-50"
          :disabled="reExtracting"
          @click="extractFromWallpaper"
        >
          {{ reExtracting ? "正在取色…" : "从当前壁纸取色" }}
        </button>

        <p class="mt-4 text-[11px] text-on-surface-variant">MD3 标准色</p>

        <div class="mt-2 grid grid-cols-6 gap-2">
          <button
            v-for="color in presets"
            :key="color"
            type="button"
            class="size-10 rounded-pill border border-outline-variant/30"
            :class="theme.seed === color ? 'outline outline-2 outline-offset-2 outline-primary' : ''"
            :style="{ backgroundColor: color }"
            :aria-label="`使用主色 ${color}`"
            @click="pickPreset(color)"
          />
        </div>

        <div class="mt-4 flex items-center gap-2">
          <input
            v-model="hexDraft"
            type="text"
            spellcheck="false"
            maxlength="7"
            class="min-w-0 flex-1 rounded-pill bg-surface-low px-4 py-2.5 font-mono text-sm uppercase outline-none"
            placeholder="#2BD4B4"
            @keyup.enter="applyHex"
          />
          <button
            type="button"
            class="shrink-0 rounded-pill bg-primary px-5 py-2.5 text-xs font-medium text-on-primary"
            @click="applyHex"
          >
            应用
          </button>
        </div>
        <p v-if="hexError" class="mt-2 text-xs text-error">{{ hexError }}</p>

        <button
          type="button"
          class="mt-4 w-full rounded-card bg-surface-high py-3 text-sm text-on-surface-variant"
          @click="sheetOpen = false"
        >
          完成
        </button>
      </div>
    </div>
  </div>
</template>
