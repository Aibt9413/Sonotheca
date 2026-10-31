<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, reactive, ref } from "vue";
import {
  describeError,
  fileDetails,
  libraryRootPath,
  listLibraryDir,
  renamePath,
  storageRequest,
  storageStatus,
  transferEntries,
  trashPaths,
  type DirListing,
  type FileDetails,
  type StorageSpace,
} from "@/ipc/bindings";
import { useBackGuard } from "@/composables/useBackGuard";
import { useUiStore } from "@/stores/ui";

/**
 * 双窗格文件管理器（M5 终版）。
 * - 纯点击选中：点一下选中，再点取消（不做划选）
 * - 每个窗格顶部有 全选 / 反选，只作用于该窗格
 * - 底栏随焦点窗格的选中数变化
 * - 详情走 Rust `file_details`：封面 + 位深/采样率/码率 + 绝对路径 + 修改时间
 */
type Side = "left" | "right";

interface PaneState {
  space: StorageSpace;
  path: string;
  listing: DirListing | null;
  selected: string[];
  loading: boolean;
}

const props = withDefaults(
  defineProps<{
    /** browse = 双窗格整理；pick-dir = 单窗格目录选择 */
    mode?: "browse" | "pick-dir";
    /** pick-dir 的初始定位目录（绝对路径） */
    initialPath?: string;
    /** 嵌在 Bottom Sheet 内时用 absolute 定位，不覆盖整个视口 */
    embedded?: boolean;
    /** browse 模式下预选中的文件相对路径 */
    selection?: string[];
  }>(),
  {
    mode: "browse",
    initialPath: "",
    embedded: false,
    selection: () => [],
  },
);
const emit = defineEmits<{
  close: [];
  applied: [];
  pick: [path: string];
  scrape: [trackId: number];
  edit: [trackId: number];
}>();
const ui = useUiStore();

// 文件管理器有自己的一套底栏：挂载期间隐藏全局 Tab 栏
ui.setManagerOpen(true);

const state = reactive<Record<Side, PaneState>>({
  left: { space: "app", path: "", listing: null, selected: [], loading: false },
  right: { space: "app", path: "", listing: null, selected: [], loading: false },
});

const focused = ref<Side>("left");
const sides: Side[] = ["left", "right"];
const picking = computed(() => props.mode === "pick-dir");
/** pick-dir 只需要一个窗格，避免"选左还是选右"的歧义 */
const visibleSides = computed<Side[]>(() => (picking.value ? ["left"] : sides));
const active = computed(() => state[focused.value]);
const otherSide = computed<Side>(() => (focused.value === "left" ? "right" : "left"));
const other = computed(() => state[otherSide.value]);
const selCount = computed(() => active.value.selected.length);

const storageGranted = ref(false);
const permDialog = ref(false);
const appRoot = ref("");
const toast = ref<string | null>(null);
const error = ref<string | null>(null);

const editing = ref(false);
const pathInput = ref("");
const inputRef = ref<HTMLInputElement | null>(null);

const detail = ref<FileDetails | null>(null);
const detailLoading = ref(false);
const renameTarget = ref<string | null>(null);
const renameValue = ref("");
const renaming = ref(false);

let pressTimer: ReturnType<typeof setTimeout> | null = null;
let toastTimer: ReturnType<typeof setTimeout> | null = null;

function flash(message: string) {
  toast.value = message;
  if (toastTimer) clearTimeout(toastTimer);
  toastTimer = setTimeout(() => (toast.value = null), 2200);
}

const crumbs = computed(() => {
  const pane = active.value;
  const list = [
    pane.space === "app"
      ? { label: "应用目录", path: "" }
      : { label: "/storage/emulated/0", path: "" },
  ];
  if (!pane.path) return list;
  let acc = "";
  for (const part of pane.path.split("/").filter(Boolean)) {
    acc = acc ? `${acc}/${part}` : part;
    list.push({ label: part, path: acc });
  }
  return list;
});

function absolutePathOf(side: Side): string {
  const pane = state[side];
  if (pane.space === "global") {
    return pane.path ? `/storage/emulated/0/${pane.path}` : "/storage/emulated/0";
  }
  const root = appRoot.value.replace(/\/+$/, "");
  return pane.path ? `${root}/${pane.path}` : root;
}

async function load(side: Side, path = state[side].path) {
  const pane = state[side];
  pane.loading = true;
  error.value = null;
  try {
    const listing = await listLibraryDir(pane.space, path);
    pane.listing = listing;
    pane.path = listing.relPath;
  } catch (e) {
    error.value = describeError(e);
    pane.listing = null;
  } finally {
    pane.loading = false;
  }
}

function focusOn(side: Side) {
  focused.value = side;
}

async function enter(side: Side, path: string) {
  focusOn(side);
  await load(side, path);
}

async function goUp(side: Side) {
  const pane = state[side];
  focusOn(side);
  await load(side, pane.listing?.parent ?? "");
}

/** 点一下选中，再点取消 */
function toggle(side: Side, relPath: string) {
  focusOn(side);
  const list = state[side].selected;
  const index = list.indexOf(relPath);
  if (index >= 0) list.splice(index, 1);
  else list.push(relPath);
}

/**
 * pick-dir 只列目录。
 * 后端 `list_dir` 已经跳过所有点开头的目录，这里再按名字兜一层 `.temp`，
 * 防止以后后端策略变化时把下载临时目录暴露成可选的音乐库根。
 */
function visibleDirs(side: Side) {
  return (state[side].listing?.dirs ?? []).filter(
    (dir) => dir.name.toLowerCase() !== ".temp",
  );
}

/** 选中当前目录并关闭；父组件负责调 set_library_root_path 做可写校验 */
function confirmPick() {
  if (picking.value && active.value.loading) return;

  const path = absolutePathOf(focused.value);
  if (!path) {
    flash("当前目录不可用");
    return;
  }

  emit("pick", path);
  emit("close");
}

/** 全选 / 反选：只作用于该窗格的文件列表 */
function selectAll(side: Side) {
  focusOn(side);
  const pane = state[side];
  pane.selected = (pane.listing?.files ?? []).map((file) => file.relPath);
}

function invertSelection(side: Side) {
  focusOn(side);
  const pane = state[side];
  const all = (pane.listing?.files ?? []).map((file) => file.relPath);
  pane.selected = all.filter((rel) => !pane.selected.includes(rel));
}

async function refreshStorage() {
  try {
    const status = await storageStatus();
    storageGranted.value = status.granted;
    if (!status.granted) {
      for (const side of ["left", "right"] as Side[]) {
        if (state[side].space === "global") {
          state[side].space = "app";
          await load(side, "");
        }
      }
    }
  } catch {
    storageGranted.value = false;
  }
}

async function switchSpace(side: Side, space: StorageSpace) {
  focusOn(side);
  if (space === "global" && !storageGranted.value) {
    permDialog.value = true;
    return;
  }
  state[side].space = space;
  state[side].selected = [];
  await load(side, "");
}

async function goGrantPermission() {
  permDialog.value = false;
  flash("已打开系统设置，开启后返回即可");
  await storageRequest();
}

// ── 长按路径 → 输入框 ────────────────────────────────────────
function pressStart() {
  if (editing.value) return;
  if (pressTimer) clearTimeout(pressTimer);
  pressTimer = setTimeout(() => {
    pathInput.value = absolutePathOf(focused.value);
    editing.value = true;
    void nextTick(() => inputRef.value?.focus());
  }, 500);
}

function pressEnd() {
  if (pressTimer) {
    clearTimeout(pressTimer);
    pressTimer = null;
  }
}

function cancelEdit() {
  editing.value = false;
  pressEnd();
}

function resolveInput(raw: string): { space: StorageSpace; rel: string } | null {
  const clean = raw.trim().replace(/\\/g, "/").replace(/\/+$/, "");
  if (!clean) return null;

  const storageRoot = "/storage/emulated/0";
  if (clean === storageRoot || clean.startsWith(`${storageRoot}/`)) {
    return { space: "global", rel: clean.slice(storageRoot.length).replace(/^\//, "") };
  }
  if (clean === "/sdcard" || clean.startsWith("/sdcard/")) {
    return { space: "global", rel: clean.slice("/sdcard".length).replace(/^\//, "") };
  }
  const root = appRoot.value.replace(/\/+$/, "");
  if (root && (clean === root || clean.startsWith(`${root}/`))) {
    return { space: "app", rel: clean.slice(root.length).replace(/^\//, "") };
  }
  return null;
}

async function commitPath() {
  const target = resolveInput(pathInput.value);
  if (!target) {
    flash("路径无效：只支持应用目录与 /storage/emulated/0");
    return;
  }
  if (target.space === "global" && !storageGranted.value) {
    permDialog.value = true;
    return;
  }
  editing.value = false;
  const side = focused.value;
  state[side].space = target.space;
  state[side].selected = [];
  await load(side, target.rel);
}

// ── 操作 ────────────────────────────────────────────────────
async function openDetail() {
  if (selCount.value !== 1) return;
  detailLoading.value = true;
  try {
    detail.value = await fileDetails(active.value.space, active.value.selected[0]);
  } catch (e) {
    error.value = describeError(e);
  } finally {
    detailLoading.value = false;
  }
}

function requestScrape() {
  const id = detail.value?.trackId;
  if (!id) return;
  detail.value = null;
  emit("scrape", id);
}

function requestEdit() {
  const id = detail.value?.trackId;
  if (!id) return;
  detail.value = null;
  emit("edit", id);
}

function openRename() {
  if (selCount.value !== 1) return;
  const rel = active.value.selected[0];
  renameTarget.value = rel;
  renameValue.value = rel.split("/").pop() ?? rel;
}

async function doRename() {
  if (!renameTarget.value || renaming.value) return;
  renaming.value = true;
  try {
    await renamePath(active.value.space, renameTarget.value, renameValue.value);
    renameTarget.value = null;
    state[focusKey()].selected = [];
    await Promise.all([load("left"), load("right")]);
    flash("已重命名");
  } catch (e) {
    error.value = describeError(e);
  } finally {
    renaming.value = false;
  }
}

function focusKey(): Side {
  return focused.value;
}

async function removeSelected() {
  if (selCount.value === 0) return;
  try {
    const removed = await trashPaths(active.value.space, [...active.value.selected]);
    state[focusKey()].selected = [];
    await Promise.all([load("left"), load("right")]);
    flash("已移入回收站");
    emit("applied");
    if (removed === 0) flash("没有文件被移动");
  } catch (e) {
    error.value = describeError(e);
  }
}

async function run(mode: "move" | "copy") {
  if (selCount.value === 0) return;
  const source = active.value;
  const target = other.value;
  try {
    const moved = await transferEntries({
      fromSpace: source.space,
      relPaths: [...source.selected],
      toSpace: target.space,
      targetDir: target.path,
      mode,
    });
    source.selected = [];
    await Promise.all([load("left"), load("right")]);
    flash(`成功${mode === "move" ? "移动" : "复制"} ${moved} 个文件`);
    emit("applied");
  } catch (e) {
    error.value = describeError(e);
  }
}

// 音频参数格式化：16bit / 44.1kHz / 320kbps
const detailSpec = computed(() => {
  const d = detail.value;
  if (!d) return "—";
  const parts: string[] = [];
  if (d.bitDepth) parts.push(`${d.bitDepth}bit`);
  if (d.sampleRate) {
    const khz = d.sampleRate / 1000;
    parts.push(`${Number.isInteger(khz) ? khz : khz.toFixed(1)}kHz`);
  }
  if (d.bitrate) parts.push(`${d.bitrate}kbps`);
  return parts.length ? parts.join(" / ") : "—";
});

const detailModified = computed(() => {
  const d = detail.value;
  if (!d?.modified) return "—";
  return new Date(d.modified * 1000).toLocaleString();
});

function formatSize(bytes: number): string {
  if (bytes < 1024 * 1024) return `${Math.round(bytes / 1024)} KB`;
  return `${(bytes / 1024 / 1024).toFixed(2)} MB`;
}

useBackGuard(
  () => true,
  () => {
    if (state.left.selected.length || state.right.selected.length) {
      state.left.selected = [];
      state.right.selected = [];
      flash("已取消全部选中");
    } else {
      emit("close");
    }
  },
);

function onFocusWindow() {
  void refreshStorage();
}

onMounted(async () => {
  try {
    appRoot.value = await libraryRootPath();
  } catch {
    appRoot.value = "";
  }

  if (picking.value) {
    // 从当前音乐库根目录开始浏览；解析失败就退回应用根目录
    const target = resolveInput(props.initialPath);
    if (target) {
      state.left.space = target.space;
      state.left.path = target.rel;
    }
  } else {
    const first = props.selection[0] ?? "";
    const index = first.lastIndexOf("/");
    state.left.path = index > 0 ? first.slice(0, index) : "";
    state.left.selected = [...props.selection];
  }

  await refreshStorage();
  await load("left");
  if (!picking.value) await load("right", "");
  focusOn("left");

  window.addEventListener("focus", onFocusWindow);
});

onUnmounted(() => {
  ui.setManagerOpen(false);
  window.removeEventListener("focus", onFocusWindow);
  if (pressTimer) clearTimeout(pressTimer);
  if (toastTimer) clearTimeout(toastTimer);
});
</script>

<template>
  <div
    class="glass-strong flex flex-col"
    :class="embedded ? 'absolute inset-0' : 'fixed inset-0 z-40'"
  >
    <!-- 顶部：焦点窗格路径 -->
    <div class="px-5 pb-3 pt-safe">
      <div class="flex items-center gap-2">
        <span
          v-if="picking"
          class="shrink-0 rounded-pill bg-primary-container px-2.5 py-1 text-[10px] text-on-primary-container"
        >
          选择目录
        </span>
        <span
          v-else
          class="shrink-0 rounded-pill bg-primary-container px-2.5 py-1 text-[10px] text-on-primary-container"
        >
          {{ focused === "left" ? "左" : "右" }}
        </span>

        <input
          v-if="editing"
          ref="inputRef"
          v-model="pathInput"
          class="min-w-0 flex-1 rounded-pill bg-surface-high px-4 py-2 font-mono text-[11px] outline-none"
          spellcheck="false"
          @keyup.enter="commitPath"
          @blur="cancelEdit"
        />

        <div
          v-else
          class="flex min-w-0 flex-1 items-center gap-1 overflow-x-auto py-1"
          @pointerdown="pressStart"
          @pointerup="pressEnd"
          @pointerleave="pressEnd"
        >
          <template v-for="(crumb, index) in crumbs" :key="crumb.path">
            <span v-if="index > 0" class="shrink-0 text-[11px] text-on-surface-variant">/</span>
            <button
              type="button"
              class="shrink-0 rounded-pill px-2.5 py-1 text-[11px]"
              :class="
                crumb.path === active.path
                  ? 'bg-primary text-on-primary'
                  : 'bg-surface-high text-on-surface-variant'
              "
              @click="enter(focused, crumb.path)"
            >
              {{ crumb.label }}
            </button>
          </template>
        </div>
      </div>

      <p v-if="picking" class="mt-1 truncate font-mono text-[10px] text-on-surface-variant/70">
        {{ absolutePathOf(focused) || "—" }}
      </p>
      <p v-else class="mt-1 text-[10px] text-on-surface-variant/70">长按路径可输入绝对路径跳转</p>
      <p v-if="error" class="mt-1 text-xs text-error">{{ error }}</p>
    </div>

    <!-- 双窗格 -->
    <div class="flex min-h-0 flex-1 gap-2.5 overflow-hidden px-5 pb-2 pt-2">
      <section
        v-for="side in visibleSides"
        :key="side"
        class="flex min-w-0 flex-1 flex-col overflow-hidden rounded-card transition"
        :class="
          focused === side
            ? 'bg-surface-container ring-2 ring-primary'
            : 'bg-surface-low ring-1 ring-outline-variant/30'
        "
        @pointerdown="focusOn(side)"
      >
        <!-- 头部：不随列表滚动 -->
        <div class="flex shrink-0 flex-col gap-1.5 px-4 pb-3 pt-4">
          <div class="flex items-center justify-between gap-1">
            <button
              type="button"
              class="rounded-pill bg-surface-high px-2 py-1 text-[10px] text-on-surface-variant"
              @click="switchSpace(side, state[side].space === 'app' ? 'global' : 'app')"
            >
              {{ state[side].space === "app" ? "应用" : "全局" }}
            </button>
            <button
              type="button"
              class="rounded-pill bg-surface-high px-2 py-1 text-[10px] text-on-surface-variant"
              @click="goUp(side)"
            >
              ↑ 上一层
            </button>
          </div>

          <div v-if="!picking" class="flex items-center gap-1">
            <button
              type="button"
              class="rounded-pill bg-surface-high px-2 py-1 text-[10px] text-on-surface-variant"
              @click="selectAll(side)"
            >
              全选
            </button>
            <button
              type="button"
              class="rounded-pill bg-surface-high px-2 py-1 text-[10px] text-on-surface-variant"
              @click="invertSelection(side)"
            >
              反选
            </button>
            <span class="ml-auto text-[10px] text-on-surface-variant">
              {{ state[side].selected.length }}
            </span>
          </div>
        </div>

        <!-- 列表：只在这里滚动；pick-dir 给底部胶囊留更大的安全距离 -->
        <div
          class="flex min-h-0 flex-1 flex-col gap-1.5 overflow-y-auto px-2.5"
          :class="picking ? 'pb-44' : 'pb-32'"
        >
          <button
            v-for="dir in visibleDirs(side)"
            :key="dir.relPath"
            type="button"
            class="flex items-center gap-1.5 rounded-card bg-surface-high/60 px-2.5 py-2 text-left"
            @click="enter(side, dir.relPath)"
          >
            <span class="text-[11px]">📁</span>
            <span class="truncate text-[11px]">{{ dir.name }}</span>
          </button>

          <!-- 选目录模式不渲染任何文件，避免误点到音频 -->
          <template v-if="!picking">
            <button
              v-for="file in state[side].listing?.files ?? []"
              :key="file.relPath"
              type="button"
              class="rounded-card px-2.5 py-2 text-left"
              :class="
                state[side].selected.includes(file.relPath)
                  ? 'bg-primary-container text-on-primary-container'
                  : 'bg-surface-high/30 text-on-surface-variant'
              "
              @click="toggle(side, file.relPath)"
            >
              <p class="truncate text-[10px]">🎵 {{ file.name }}</p>
            </button>
          </template>

          <p v-if="state[side].loading" class="px-1 text-[10px] text-on-surface-variant">读取中…</p>
        </div>
      </section>
    </div>

    <!-- 权限说明 -->
    <div
      v-if="permDialog"
      class="fixed inset-0 z-[60] flex items-center justify-center bg-black/50 p-6"
      @click.self="permDialog = false"
    >
      <div class="w-full max-w-sm rounded-card bg-surface-container p-5 shadow-e3">
        <p class="text-sm font-medium">需要「所有文件访问权限」</p>
        <p class="mt-2 text-xs leading-relaxed text-on-surface-variant">
          要在手机公共目录与 App 私有库之间移动文件，需要这项系统级权限。
        </p>
        <div class="mt-4 flex justify-end gap-2">
          <button type="button" class="rounded-pill px-4 py-2 text-xs text-on-surface-variant" @click="permDialog = false">
            仅应用内
          </button>
          <button type="button" class="rounded-pill bg-primary px-4 py-2 text-xs font-medium text-on-primary" @click="goGrantPermission">
            去设置开启
          </button>
        </div>
      </div>
    </div>

    <!-- 极客风详情 -->
    <div v-if="detail || detailLoading" class="fixed inset-0 z-[60] flex items-end bg-black/50" @click.self="detail = null">
      <div class="glass-sheet max-h-[88vh] w-full overflow-y-auto rounded-t-card p-5 pb-safe shadow-e3">
        <div class="mx-auto mb-3 h-1 w-10 rounded-pill bg-outline-variant" />

        <template v-if="detail">
          <div class="flex items-center gap-4">
            <img
              v-if="detail.cover"
              :src="detail.cover"
              alt=""
              class="size-24 shrink-0 rounded-card object-cover shadow-e2"
            />
            <div v-else class="grid size-24 shrink-0 place-items-center rounded-card bg-surface-highest text-on-surface-variant">
              <span class="text-[10px]">无封面</span>
            </div>

            <div class="min-w-0 flex-1">
              <p class="truncate text-sm font-medium">{{ detail.name }}</p>
              <p class="mt-1 font-mono text-[11px] text-primary">{{ detail.format.toUpperCase() }}</p>
              <p class="mt-1 font-mono text-[11px] text-on-surface-variant">{{ detailSpec }}</p>
            </div>
          </div>

          <div class="mt-4 space-y-2">
            <div class="rounded-card bg-surface-low p-3">
              <p class="text-[10px] text-on-surface-variant">绝对路径</p>
              <p class="mt-0.5 break-all font-mono text-[11px]">{{ detail.path }}</p>
            </div>
            <div class="grid grid-cols-2 gap-2">
              <div class="rounded-card bg-surface-low p-3">
                <p class="text-[10px] text-on-surface-variant">大小</p>
                <p class="mt-0.5 font-mono text-[11px]">{{ formatSize(detail.size) }}</p>
              </div>
              <div class="rounded-card bg-surface-low p-3">
                <p class="text-[10px] text-on-surface-variant">修改时间</p>
                <p class="mt-0.5 font-mono text-[11px]">{{ detailModified }}</p>
              </div>
            </div>
          </div>
        </template>

        <p v-else class="py-6 text-center text-xs text-on-surface-variant">读取中…</p>

        <div v-if="detail?.trackId" class="mt-4 grid grid-cols-2 gap-2">
          <button
            type="button"
            class="rounded-pill bg-primary-container px-4 py-2.5 text-xs font-medium text-on-primary-container"
            @click="requestScrape"
          >
            补全元数据
          </button>
          <button
            type="button"
            class="rounded-pill bg-surface-high px-4 py-2.5 text-xs text-on-surface-variant"
            @click="requestEdit"
          >
            手动编辑
          </button>
        </div>

        <button
          type="button"
          class="mt-4 w-full rounded-card bg-surface-high py-3 text-sm text-on-surface-variant"
          @click="detail = null"
        >
          关闭
        </button>
      </div>
    </div>

    <!-- 重命名 -->
    <div v-if="renameTarget" class="fixed inset-0 z-[60] flex items-center justify-center bg-black/50 p-6" @click.self="renameTarget = null">
      <div class="w-full max-w-sm rounded-card bg-surface-container p-5 shadow-e3">
        <p class="text-sm font-medium">重命名</p>
        <p class="mt-1 text-[11px] text-on-surface-variant">只改文件名，不动音频标签</p>
        <input
          v-model="renameValue"
          class="mt-3 w-full rounded-pill bg-surface-high px-4 py-2.5 text-sm outline-none"
          spellcheck="false"
          @keyup.enter="doRename"
        />
        <div class="mt-4 flex justify-end gap-2">
          <button type="button" class="rounded-pill px-4 py-2 text-xs text-on-surface-variant" @click="renameTarget = null">
            取消
          </button>
          <button
            type="button"
            class="rounded-pill bg-primary px-4 py-2 text-xs font-medium text-on-primary disabled:opacity-50"
            :disabled="renaming || !renameValue.trim()"
            @click="doRename"
          >
            {{ renaming ? "处理中…" : "确定" }}
          </button>
        </div>
      </div>
    </div>

    <!-- Toast -->
    <div v-if="toast" class="pointer-events-none fixed inset-x-0 bottom-32 z-50 flex justify-center px-5">
      <div class="rounded-pill bg-on-surface/90 px-4 py-2 text-xs text-surface shadow-e2">{{ toast }}</div>
    </div>

    <!-- 底部操作栏：随选中数变化 -->
    <div class="pointer-events-none fixed inset-x-0 bottom-0 z-50 flex justify-center px-5 pb-safe">
      <div
        class="glass glass-edge pointer-events-auto flex max-w-full items-center gap-1 overflow-x-auto rounded-pill p-1.5 shadow-e3"
      >
        <!-- 目录选择：固定胶囊，只选当前文件夹 -->
        <template v-if="picking">
          <button
            type="button"
            class="shrink-0 rounded-pill px-4 py-2 text-xs text-on-surface-variant"
            @click="emit('close')"
          >
            取消
          </button>
          <button
            type="button"
            class="shrink-0 rounded-pill bg-primary px-5 py-2 text-xs font-medium text-on-primary disabled:opacity-50"
            :disabled="active.loading"
            :title="absolutePathOf(focused)"
            @click="confirmPick"
          >
            选择此文件夹
          </button>
        </template>

        <template v-else-if="selCount === 0">
          <button type="button" class="rounded-pill px-4 py-2 text-xs text-error" @click="emit('close')">
            退出
          </button>
        </template>

        <template v-else>
          <button
            v-if="selCount === 1"
            type="button"
            class="shrink-0 rounded-pill bg-surface-high px-3.5 py-2 text-xs text-on-surface-variant"
            @click="openDetail"
          >
            详情
          </button>
          <button
            v-if="selCount === 1"
            type="button"
            class="shrink-0 rounded-pill bg-surface-high px-3.5 py-2 text-xs text-on-surface-variant"
            @click="openRename"
          >
            重命名
          </button>
          <button
            type="button"
            class="shrink-0 rounded-pill bg-primary-container px-3.5 py-2 text-xs font-medium text-on-primary-container"
            @click="run('copy')"
          >
            复制
          </button>
          <button
            type="button"
            class="shrink-0 rounded-pill bg-primary px-3.5 py-2 text-xs font-medium text-on-primary"
            @click="run('move')"
          >
            移动
          </button>
          <button
            type="button"
            class="shrink-0 rounded-pill bg-error-container px-3.5 py-2 text-xs text-on-error-container"
            @click="removeSelected"
          >
            删除
          </button>
          <button type="button" class="shrink-0 rounded-pill px-3.5 py-2 text-xs text-error" @click="emit('close')">
            退出
          </button>
        </template>
      </div>
    </div>
  </div>
</template>
