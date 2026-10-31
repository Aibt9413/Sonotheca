<script setup lang="ts">
import { onDeactivated, onMounted, onUnmounted, ref, watch } from "vue";
import SCard from "@/ui/SCard.vue";
import SButton from "@/ui/SButton.vue";
import SProgress from "@/components/SProgress.vue";
import { useTasksStore } from "@/stores/tasks";
import { useToastStore } from "@/stores/toast";
import { useUiStore } from "@/stores/ui";
import { describeError, isTauri, isTerminal, type DownloadTask, type TaskState } from "@/ipc/bindings";import { formatBytes, formatSpeed } from "@/utils/format";

defineOptions({ name: "TasksView" });

const tasks = useTasksStore();
const toast = useToastStore();
const ui = useUiStore();
const url = ref("");
const submitting = ref(false);

/** 长按 / 点 X 打开的操作面板 */
const actionTask = ref<DownloadTask | null>(null);
const deleting = ref(false);
const restarting = ref(false);

const STATE_LABEL: Record<TaskState, string> = {
  queued: "排队中",
  preflight: "探测中",
  resolving: "解析中",
  downloading: "下载中",
  processing: "处理中",
  verifying: "校验中",
  completed: "已完成",
  failed: "失败",
  canceled: "已暂停",
};

const STATE_CLASS: Record<TaskState, string> = {
  queued: "bg-surface-high text-on-surface-variant",
  preflight: "bg-surface-high text-on-surface-variant",
  resolving: "bg-surface-high text-on-surface-variant",
  downloading: "bg-primary-container text-on-primary-container",
  processing: "bg-tertiary/20 text-tertiary",
  verifying: "bg-tertiary/20 text-tertiary",
  completed: "bg-primary-container text-on-primary-container",
  failed: "bg-error-container text-on-error-container",
  canceled: "bg-surface-high text-on-surface-variant",
};

async function submit() {
  const value = url.value.trim();
  if (!value || submitting.value) return;
  submitting.value = true;
  try {
    await tasks.start(value);
    url.value = "";
  } catch {
    // 错误已写进 tasks.error，输入框保留方便重试
  } finally {
    submitting.value = false;
  }
}

function percent(task: DownloadTask): number | null {
  return tasks.percent(task);
}

function indeterminate(task: DownloadTask): boolean {
  return task.state === "downloading" && tasks.percent(task) === null;
}

/** 这些错误说明 .part 大概率还在，可以真正"接着下" */
const RETRYABLE_KINDS = ["network", "timeout", "rate_limited"];

function canResume(task: DownloadTask): boolean {
  return task.state === "failed" || task.state === "canceled";
}

function resumeLabel(task: DownloadTask): string {
  return task.error && RETRYABLE_KINDS.includes(task.error.kind) ? "继续" : "重试";
}

async function onResume(id: string) {
  try {
    await tasks.resume(id);
  } catch {
    // 错误已写入 tasks.error
  }
}

// ── 历史记录删除：长按卡片 或 点右上角 X ─────────────────────
let pressTimer: ReturnType<typeof setTimeout> | null = null;
let pressOrigin: { x: number; y: number } | null = null;
let longPressedTask: DownloadTask | null = null;

function clearPress() {
  if (pressTimer) {
    clearTimeout(pressTimer);
    pressTimer = null;
  }
  pressOrigin = null;
}

function pressStart(event: PointerEvent, task: DownloadTask) {
  if (!isTerminal(task.state)) return;

  // 卡片里的按钮（继续 / 暂停 / X）优先，不抢它们的手势
  const target = event.target as HTMLElement | null;
  if (target?.closest("button, a, input")) return;

  clearPress();
  longPressedTask = null;
  pressOrigin = { x: event.clientX, y: event.clientY };
  pressTimer = setTimeout(() => {
    longPressedTask = task;
    pressTimer = null;
    pressOrigin = null;
  }, 480);
}

function pressMove(event: PointerEvent) {
  if (!pressOrigin) return;
  if (Math.hypot(event.clientX - pressOrigin.x, event.clientY - pressOrigin.y) > 10) {
    clearPress();
    longPressedTask = null;
  }
}

/** 长按计时器到点后，松手才弹面板，避免 pointerup 又把遮罩点掉 */
function pressEnd() {
  const long = longPressedTask;
  longPressedTask = null;
  clearPress();
  if (long) actionTask.value = long;
}

async function removeTask(task: DownloadTask) {
  if (deleting.value) return;

  deleting.value = true;
  try {
    await tasks.remove(task.id);
    actionTask.value = null;
    toast.show("已删除下载记录");
  } catch (e) {
    toast.show(describeError(e), "warning");
  } finally {
    deleting.value = false;
  }
}

async function restartTask(task: DownloadTask) {
  if (restarting.value) return;

  restarting.value = true;
  try {
    await tasks.restart(task.id);
    actionTask.value = null;
    toast.show("已重新入队，完成时将覆盖旧文件");
  } catch (e) {
    toast.show(describeError(e), "warning");
  } finally {
    restarting.value = false;
  }
}

onUnmounted(() => {
  clearPress();
  ui.setSheetOpen(false);
});
onDeactivated(() => {
  clearPress();
  ui.setSheetOpen(false);
});

watch(actionTask, (task) => ui.setSheetOpen(!!task));

onMounted(() => {
  void tasks.refresh();
});
</script>

<template>
  <div class="flex w-full flex-col gap-3">
  <SCard>
    <p class="text-sm font-medium">新建下载</p>
    <p class="mt-0.5 text-xs text-on-surface-variant">
      粘贴音频直链，下载后自动写标签并按规则归档
    </p>
    <div class="mt-3 flex items-center gap-2">
      <input
        v-model="url"
        type="url"
        spellcheck="false"
        placeholder="https://example.com/song.flac"
        class="min-w-0 flex-1 rounded-pill bg-surface-low px-4 py-2.5 text-xs outline-none placeholder:text-on-surface-variant/60"
        @keyup.enter="submit"
      />
      <SButton :disabled="!url.trim() || submitting" @click="submit">
        {{ submitting ? "入队中…" : "下载" }}
      </SButton>
    </div>
    <p v-if="tasks.error" class="mt-2 text-xs text-error">{{ tasks.error }}</p>
    <p v-else-if="!isTauri()" class="mt-2 text-xs text-on-surface-variant">
      浏览器预览模式：IPC 不可用，请在 `pnpm tauri:dev` 下测试
    </p>
  </SCard>

  <SCard v-if="tasks.items.length === 0" class="border border-dashed border-outline-variant/60">
    <p class="text-sm font-medium">没有下载任务</p>
    <p class="mt-1 text-xs text-on-surface-variant">
      下载流先写入 <code>.temp/downloads/&lt;uuid&gt;.part</code>，处理完成后原子移动到最终目录。
    </p>
  </SCard>

  <SCard
    v-for="task in tasks.items"
    :key="task.id"
    class="space-y-2.5"
    @pointerdown="pressStart($event, task)"
    @pointermove="pressMove"
    @pointerup="pressEnd"
    @pointerleave="pressEnd"
    @pointercancel="pressEnd"
  >
    <div class="flex items-start justify-between gap-3">
      <div class="min-w-0">
        <p class="truncate text-sm font-medium">{{ task.title }}</p>
        <p class="truncate text-xs text-on-surface-variant">
          {{ task.artist }} · {{ task.snapshot.relativeDir }}
        </p>
      </div>
      <div class="flex shrink-0 items-center gap-1">
        <span
          class="rounded-pill px-3 py-1 text-[11px] font-medium"
          :class="STATE_CLASS[task.state]"
        >
          {{ STATE_LABEL[task.state] }}
        </span>
        <button
          v-if="isTerminal(task.state)"
          type="button"
          class="grid size-7 place-items-center rounded-pill text-on-surface-variant hover:bg-surface-high"
          aria-label="删除下载记录"
          @click.stop="actionTask = task"
        >
          <svg class="size-3.5" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
            <path
              d="M18.3 5.7 12 12l6.3 6.3-1.4 1.4L10.6 13.4 4.3 19.7 2.9 18.3 9.2 12 2.9 5.7 4.3 4.3l6.3 6.3 6.3-6.3z"
            />
          </svg>
        </button>
      </div>
    </div>

    <SProgress :value="percent(task)" :indeterminate="indeterminate(task)" />

    <div class="flex items-center justify-between gap-3 text-[11px] text-on-surface-variant">
      <span class="truncate font-mono">
        {{ formatBytes(task.bytesDone) }} / {{ formatBytes(task.bytesTotal) }}
        <template v-if="task.state === 'downloading'">
          · {{ formatSpeed(tasks.speed(task)) }}
        </template>
      </span>
      <div class="flex shrink-0 items-center gap-1">
        <button
          v-if="canResume(task)"
          type="button"
          class="rounded-pill bg-primary-container px-3 py-1 font-medium text-on-primary-container"
          @click="onResume(task.id)"
        >
          {{ resumeLabel(task) }}
        </button>
        <button
          v-if="!isTerminal(task.state)"
          type="button"
          class="rounded-pill px-3 py-1 text-error"
          @click="tasks.cancel(task.id)"
        >
          暂停
        </button>
      </div>
    </div>

    <p v-if="task.error" class="text-xs text-error">
      {{ task.error.kind }} · {{ task.error.message }}
    </p>
    <p v-else-if="task.finalPath" class="truncate font-mono text-[11px] text-on-surface-variant">
      {{ task.finalPath }}
    </p>
  </SCard>

  <!-- 历史记录操作面板：只清理 SQLite 行，不碰文件 -->
  <div
    v-if="actionTask"
    class="fixed inset-0 z-50 flex items-end bg-black/50"
    @click.self="actionTask = null"
  >
    <div class="glass-sheet w-full rounded-t-card p-5 pb-safe shadow-e3">
      <div class="mx-auto mb-3 h-1 w-10 rounded-pill bg-outline-variant" />
      <p class="truncate text-sm font-medium">{{ actionTask.title }}</p>
      <p class="mt-1 text-[11px] leading-relaxed text-on-surface-variant">
        重新下载会覆盖同名旧文件；删除记录则不会动已归档音频。
      </p>

      <button
        type="button"
        class="mt-4 w-full rounded-card bg-primary-container py-3 text-sm font-medium text-on-primary-container disabled:opacity-50"
        :disabled="restarting || deleting"
        @click="restartTask(actionTask)"
      >
        {{ restarting ? "重新入队…" : "重新下载" }}
      </button>
      <button
        type="button"
        class="mt-2 w-full rounded-card bg-error-container py-3 text-sm text-on-error-container disabled:opacity-50"
        :disabled="deleting || restarting"
        @click="removeTask(actionTask)"
      >
        {{ deleting ? "删除中…" : "删除记录" }}
      </button>
      <button
        type="button"
        class="mt-2 w-full rounded-card bg-surface-high py-3 text-sm text-on-surface-variant"
        @click="actionTask = null"
      >
        取消
      </button>
    </div>
  </div>
  </div>
</template>
