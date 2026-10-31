<script setup lang="ts">
import { onMounted, onUnmounted, ref } from "vue";
import SButton from "@/ui/SButton.vue";
import { useUiStore } from "@/stores/ui";
import {
  describeError,
  scrapeApply,
  scrapePreview,
  type ScrapeCandidate,
  type ScrapePreview,
} from "@/ipc/bindings";

/**
 * 刮削确认 Bottom Sheet。
 *
 * 关键 UX：**绝不静默覆盖**。左边是文件当前信息，右边/下方是候选，
 * 用户点了「确认应用」才真正写回（Rust 侧 lofty）。
 */
const props = defineProps<{ trackId: number; title: string }>();
const emit = defineEmits<{ close: []; applied: [] }>();
const ui = useUiStore();

const preview = ref<ScrapePreview | null>(null);
const loading = ref(true);
const applying = ref(false);
const error = ref<string | null>(null);
const pickedIndex = ref(0);

const picked = ref<ScrapeCandidate | null>(null);

async function load() {
  loading.value = true;
  error.value = null;
  try {
    preview.value = await scrapePreview(props.trackId);
    pickedIndex.value = 0;
    picked.value = preview.value.candidates[0] ?? null;
  } catch (e) {
    error.value = describeError(e);
  } finally {
    loading.value = false;
  }
}

function pick(candidate: ScrapeCandidate, index: number) {
  picked.value = candidate;
  pickedIndex.value = index;
}

async function apply() {
  if (!picked.value || applying.value) return;
  applying.value = true;
  error.value = null;
  try {
    await scrapeApply(props.trackId, picked.value);
    emit("applied");
  } catch (e) {
    error.value = describeError(e);
  } finally {
    applying.value = false;
  }
}

onMounted(() => {
  ui.setSheetOpen(true);
  void load();
});

onUnmounted(() => ui.setSheetOpen(false));
</script>

<template>
  <div class="fixed inset-0 z-50 flex items-end bg-black/50" @click.self="emit('close')">
    <div class="glass-sheet max-h-[85vh] w-full overflow-y-auto rounded-t-card p-5 pb-safe shadow-e3">
      <div class="mx-auto mb-3 h-1 w-10 rounded-pill bg-outline-variant" />

      <p class="text-sm font-medium">补全元数据</p>
      <p class="mt-0.5 truncate text-xs text-on-surface-variant">{{ title }}</p>

      <p v-if="error" class="mt-3 text-xs text-error">{{ error }}</p>

      <div v-if="applying" class="mt-3 rounded-card bg-surface-low p-3">
        <div class="flex items-center justify-between gap-3 text-[11px]">
          <span class="font-medium">正在补全 (1/1)</span>
          <span class="text-on-surface-variant">写入标签与封面…</span>
        </div>
        <div class="mt-2 h-1.5 overflow-hidden rounded-pill bg-surface-highest">
          <div class="h-full w-1/2 animate-pulse rounded-pill bg-primary" />
        </div>
      </div>

      <div v-if="loading" class="mt-4 animate-pulse space-y-2">
        <div class="h-4 w-1/3 rounded-card bg-surface-highest" />
        <div class="h-16 rounded-card bg-surface-highest" />
      </div>

      <template v-else-if="preview">
        <!-- 当前信息 -->
        <div class="mt-4 rounded-card bg-surface-low p-3">
          <p class="text-[11px] text-on-surface-variant">当前信息</p>
          <p class="mt-1 truncate text-sm">{{ preview.current.title }}</p>
          <p class="truncate text-xs text-on-surface-variant">
            {{ preview.current.artist || "未知艺术家" }}
            <template v-if="preview.current.album"> · {{ preview.current.album }}</template>
          </p>
        </div>

        <!-- 候选 -->
        <p class="mt-4 text-[11px] text-on-surface-variant">刮削结果（{{ preview.candidates.length }}）</p>

        <div class="mt-2 flex flex-col gap-2">
          <button
            v-for="(candidate, index) in preview.candidates"
            :key="candidate.id"
            type="button"
            class="flex items-center gap-3 rounded-card p-2.5 text-left transition"
            :class="
              index === pickedIndex
                ? 'bg-primary-container text-on-primary-container'
                : 'bg-surface-low'
            "
            @click="pick(candidate, index)"
          >
            <img
              v-if="candidate.coverUrl"
              :src="candidate.coverUrl"
              alt=""
              referrerpolicy="no-referrer"
              class="size-11 shrink-0 rounded-card object-cover"
            />
            <div v-else class="size-11 shrink-0 rounded-card bg-surface-highest" />
            <div class="min-w-0 flex-1">
              <p class="truncate text-sm">{{ candidate.title }}</p>
              <p class="truncate text-[11px] opacity-80">
                {{ candidate.artist }}<template v-if="candidate.album"> · {{ candidate.album }}</template>
              </p>
            </div>
            <span class="shrink-0 rounded-pill bg-surface-highest px-2 py-0.5 text-[10px] text-on-surface-variant">
              {{ candidate.score }}
            </span>
          </button>

          <p v-if="preview.candidates.length === 0" class="text-xs text-on-surface-variant">
            没有找到匹配项
          </p>
        </div>
      </template>

      <div class="mt-4 flex gap-2">
        <SButton variant="text" @click="emit('close')">取消</SButton>
        <SButton
          class="flex-1"
          :disabled="!picked || applying"
          @click="apply"
        >
          {{ applying ? "写入中…" : "确认应用" }}
        </SButton>
      </div>
    </div>
  </div>
</template>
