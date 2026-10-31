<script setup lang="ts">
import { onMounted, ref } from "vue";
import { useRouter } from "vue-router";
import FileManager from "@/components/FileManager.vue";
import SCard from "@/ui/SCard.vue";
import SPill from "@/ui/SPill.vue";
import { useSettingsStore } from "@/stores/settings";
import {
  describeError,
  isTauri,
  libraryRootPath,
  setLibraryRootPath,
  type ConcreteQuality,
} from "@/ipc/bindings";
import { useToastStore } from "@/stores/toast";

defineOptions({ name: "SettingsView" });

const router = useRouter();
const prefs = useSettingsStore();
const toast = useToastStore();

/** 音乐库根目录：只读展示 + 目录选择器，避免手输路径写错 */
const libraryPath = ref("");
const libraryError = ref<string | null>(null);
const savingPath = ref(false);
const pickerOpen = ref(false);

async function loadLibraryPath() {
  if (!isTauri()) return;
  try {
    libraryPath.value = await libraryRootPath();
  } catch (e) {
    libraryError.value = describeError(e);
  }
}

/** 选择器返回绝对路径后，交给 Rust 做建目录 + 可写探测，成功才落库 */
async function applyLibraryPath(path: string) {
  pickerOpen.value = false;
  if (!path || savingPath.value) return;

  savingPath.value = true;
  libraryError.value = null;
  try {
    const saved = await setLibraryRootPath(path);
    libraryPath.value = saved;
    toast.show("音乐库路径已更新");
  } catch (e) {
    libraryError.value = describeError(e);
  } finally {
    savingPath.value = false;
  }
}

const qualityOptions: ReadonlyArray<{ value: ConcreteQuality; label: string }> = [
  { value: "jymaster", label: "超清母带" },
  { value: "sky", label: "沉浸声" },
  { value: "hires", label: "Hi-Res" },
  { value: "lossless", label: "无损" },
  { value: "exhigh", label: "极高 320K" },
  { value: "standard", label: "标准 128K" },
];

const qualityError = ref<string | null>(null);

async function chooseQuality(value: ConcreteQuality) {
  qualityError.value = null;
  try {
    // 落到 Rust 的 settings 表；localStorage 只是启动缓存
    await prefs.setDefaultQuality(value);
  } catch (e) {
    qualityError.value = describeError(e);
  }
}

onMounted(() => {
  void loadLibraryPath();
});
</script>

<template>
  <div class="flex w-full flex-col gap-3">
    <!-- 二级页入口：账号与主题并列 -->
    <div class="grid grid-cols-2 gap-3">
      <SCard interactive class="flex flex-col gap-3" @click="router.push('/sources')">
        <div class="flex items-start justify-between">
          <div
            class="grid size-9 place-items-center rounded-pill bg-primary-container text-on-primary-container"
          >
            <svg class="size-5" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
              <path
                d="M12 12a4 4 0 1 0 0-8 4 4 0 0 0 0 8zm0 2c-3.87 0-7 1.79-7 4v2h14v-2c0-2.21-3.13-4-7-4z"
              />
            </svg>
          </div>
          <span class="text-lg leading-none text-on-surface-variant" aria-hidden="true">›</span>
        </div>
        <p class="text-sm font-medium">账号与数据源</p>
      </SCard>

      <SCard interactive class="flex flex-col gap-3" @click="router.push('/theme')">
        <div class="flex items-start justify-between">
          <div
            class="grid size-9 place-items-center rounded-pill bg-secondary-container text-on-secondary-container"
          >
            <svg class="size-5" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
              <path
                d="M12 3a9 9 0 0 0 0 18c.83 0 1.5-.67 1.5-1.5 0-.39-.15-.74-.39-1.01-.23-.26-.38-.61-.38-.99 0-.83.67-1.5 1.5-1.5H16a5 5 0 0 0 5-5c0-4.42-4.03-8-9-8zm-5.5 9c-.83 0-1.5-.67-1.5-1.5S5.67 9 6.5 9 8 9.67 8 10.5 7.33 12 6.5 12zm3-4C8.67 8 8 7.33 8 6.5S8.67 5 9.5 5s1.5.67 1.5 1.5S10.33 8 9.5 8zm5 0c-.83 0-1.5-.67-1.5-1.5S13.67 5 14.5 5s1.5.67 1.5 1.5S15.33 8 14.5 8zm3 4c-.83 0-1.5-.67-1.5-1.5S16.67 9 17.5 9s1.5.67 1.5 1.5-.67 1.5-1.5 1.5z"
              />
            </svg>
          </div>
          <span class="text-lg leading-none text-on-surface-variant" aria-hidden="true">›</span>
        </div>
        <p class="text-sm font-medium">主题外观</p>
      </SCard>
    </div>

    <SCard interactive class="flex items-center gap-3" @click="router.push('/downloads')">
      <div class="grid size-9 shrink-0 place-items-center rounded-pill bg-tertiary/20 text-tertiary">
        <svg class="size-5" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
          <path d="M5 20h14v-2H5v2zM19 9h-4V3H9v6H5l7 7 7-7z" />
        </svg>
      </div>
      <div class="min-w-0 flex-1">
        <p class="text-sm font-medium">下载设置</p>
      </div>
      <span class="text-lg leading-none text-on-surface-variant" aria-hidden="true">›</span>
    </SCard>

    <SCard>
      <div class="flex items-start gap-3">
        <p class="min-w-0 flex-1 self-center text-sm font-medium">音乐库存储路径</p>
        <button
          type="button"
          class="shrink-0 rounded-pill bg-primary-container px-4 py-2 text-xs font-medium text-on-primary-container disabled:opacity-50"
          :disabled="savingPath"
          @click="pickerOpen = true"
        >
          {{ savingPath ? "校验中…" : "更改" }}
        </button>
      </div>
      <p class="mt-2 break-all font-mono text-[11px] text-on-surface-variant">
        {{ libraryPath || "未读取" }}
      </p>
      <p v-if="libraryError" class="mt-2 text-xs text-error">{{ libraryError }}</p>
    </SCard>

    <SCard>
      <p class="text-sm font-medium">默认下载音质</p>
      <div class="mt-3 flex flex-wrap gap-2">
        <SPill
          v-for="item in qualityOptions"
          :key="item.value"
          :active="prefs.settings.defaultQuality === item.value"
          @click="chooseQuality(item.value)"
        >
          {{ item.label }}
        </SPill>
      </div>
      <p v-if="qualityError" class="mt-2 text-xs text-error">{{ qualityError }}</p>
    </SCard>

    <SCard interactive class="flex items-center gap-3" @click="router.push('/about')">
      <div class="grid size-9 shrink-0 place-items-center rounded-pill bg-surface-high text-on-surface-variant">
        <svg class="size-5" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
          <path
            d="M11 7h2v2h-2V7zm0 4h2v6h-2v-6zm1-9a10 10 0 1 0 0 20 10 10 0 0 0 0-20z"
          />
        </svg>
      </div>
      <p class="min-w-0 flex-1 text-sm font-medium">关于</p>
      <span class="text-lg leading-none text-on-surface-variant" aria-hidden="true">›</span>
    </SCard>

    <!-- P1：目录选择 Bottom Sheet，复用 FileManager 的 pick-dir 模式 -->
    <div
      v-if="pickerOpen"
      class="fixed inset-0 z-50 flex items-end bg-black/50"
      @click.self="pickerOpen = false"
    >
      <div class="glass-sheet relative h-[92vh] w-full overflow-hidden rounded-t-card shadow-e3">
        <FileManager
          embedded
          mode="pick-dir"
          :selection="[]"
          :initial-path="libraryPath"
          @pick="applyLibraryPath"
          @close="pickerOpen = false"
        />
      </div>
    </div>
  </div>
</template>
