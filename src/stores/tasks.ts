import { defineStore } from "pinia";
import { computed, reactive, ref, watch } from "vue";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import {
  addDirectDownloadTask,
  addNeteaseDownloadTask,
  addSourceDownloadTask,
  cancelDownloadTask,
  deleteDownloadTask,
  describeError,
  isTauri,
  isTerminal,
  listDownloadTasks,
  resumeDownloadTask,
  restartDownloadTask,
  setFgsCount,
  type DownloadTask,
  type NeteaseEnqueue,
  type NeteaseQuality,
  type ProgressEvent,
  type StateEvent,
} from "@/ipc/bindings";
import { useLibraryStore } from "@/stores/library";

export const useTasksStore = defineStore("tasks", () => {
  const library = useLibraryStore();

  const items = ref<DownloadTask[]>([]);
  /** 实时进度：事件高频、DB 低频，界面优先读这里 */
  const live = reactive<Record<string, ProgressEvent>>({});
  const loading = ref(false);
  const error = ref<string | null>(null);

  let unlisten: UnlistenFn[] = [];
  let bootstrapped = false;
  /** 已删除的历史记录 id：挡住迟到的 download://state 事件把它重新 upsert 回来 */
  const dismissed = new Set<string>();

  /** 非终态任务数 —— 决定 FGS 的起停（0 停，>0 起） */
  const activeCount = computed(() => items.value.filter((item) => !isTerminal(item.state)).length);

  watch(activeCount, (count) => {
    void syncForeground(count);
  });

  async function syncForeground(count: number) {
    try {
      await setFgsCount(count);
    } catch {
      // 桌面端是空实现；Android 上若插件未就绪也不该阻断下载
    }
  }

  function upsert(task: DownloadTask) {
    if (dismissed.has(task.id)) return;
    const index = items.value.findIndex((item) => item.id === task.id);
    if (index >= 0) items.value[index] = task;
    else items.value.unshift(task);
  }

  async function refresh() {
    if (!isTauri()) return;
    loading.value = true;
    error.value = null;
    try {
      items.value = await listDownloadTasks();
    } catch (e) {
      error.value = describeError(e);
    } finally {
      loading.value = false;
    }
  }

  async function bootstrap() {
    if (!isTauri() || bootstrapped) return;
    bootstrapped = true;

    unlisten.push(
      await listen<ProgressEvent>("download://progress", ({ payload }) => {
        live[payload.id] = payload;
        const row = items.value.find((item) => item.id === payload.id);
        if (row) {
          row.bytesDone = payload.bytesDone;
          row.bytesTotal = payload.bytesTotal ?? row.bytesTotal;
          row.state = payload.state;
        }
      }),
    );

    unlisten.push(
      await listen<StateEvent>("download://state", ({ payload }) => {
        upsert(payload.task);
        // 完成即刷新管理页卡片列表
        if (payload.task.state === "completed") void library.refresh();
      }),
    );

    await refresh();
  }

  async function start(url: string) {
    error.value = null;
    try {
      const task = await addDirectDownloadTask(url);
      upsert(task);
      return task;
    } catch (e) {
      error.value = describeError(e);
      throw e;
    }
  }

  /** 多源统一入队：按 source 路由到 B站 / Navidrome / 网易云 */
  async function startSource(
    source: string,
    hit: { id: string; title: string; artist: string },
    quality: NeteaseQuality = "lossless",
  ): Promise<NeteaseEnqueue> {
    error.value = null;
    try {
      const result = await addSourceDownloadTask(source, hit.id, hit.title, hit.artist, quality);
      upsert(result.task);
      return result;
    } catch (e) {
      error.value = describeError(e);
      throw e;
    }
  }

  /** 网易云专用入口（保留给旧调用点） */
  async function startNetease(
    hit: { id: string; title: string; artist: string },
    quality: NeteaseQuality = "lossless",
  ): Promise<NeteaseEnqueue> {
    error.value = null;
    try {
      const result = await addNeteaseDownloadTask(hit.id, hit.title, hit.artist, quality);
      upsert(result.task);
      return result;
    } catch (e) {
      error.value = describeError(e);
      throw e;
    }
  }

  async function cancel(id: string) {
    try {
      await cancelDownloadTask(id);
    } catch (e) {
      error.value = describeError(e);
    }
  }

  /** 删除历史记录：只允许终态任务，后端会再校验一次 */
  async function remove(id: string) {
    error.value = null;
    try {
      await deleteDownloadTask(id);
      dismissed.add(id);
      items.value = items.value.filter((item) => item.id !== id);
      delete live[id];
    } catch (e) {
      error.value = describeError(e);
      throw e;
    }
  }

  /** 续传：Rust 侧会复用同一 task id 与同一个 .part */
  async function resume(id: string) {
    error.value = null;
    try {
      const task = await resumeDownloadTask(id);
      // 旧的实时进度作废，等新事件覆盖
      delete live[id];
      upsert(task);
      return task;
    } catch (e) {
      error.value = describeError(e);
      throw e;
    }
  }

  /** 重新下载：重新解析直链、清空进度，完成时 Overwrite 覆盖旧文件 */
  async function restart(id: string) {
    error.value = null;
    try {
      const task = await restartDownloadTask(id);
      dismissed.delete(id);
      delete live[id];
      upsert(task);
      return task;
    } catch (e) {
      error.value = describeError(e);
      throw e;
    }
  }

  /** 0-100，未知总量时返回 null（界面显示不确定态进度条） */
  function percent(task: DownloadTask): number | null {
    const p = live[task.id];
    const done = p?.bytesDone ?? task.bytesDone;
    const total = p?.bytesTotal ?? task.bytesTotal;
    if (!total || total <= 0) return null;
    return Math.min(100, Math.max(0, Math.round((done / total) * 100)));
  }

  function speed(task: DownloadTask): number {
    return live[task.id]?.speedBps ?? 0;
  }

  function dispose() {
    unlisten.forEach((fn) => fn());
    unlisten = [];
    bootstrapped = false;
  }

  return {
    items,
    live,
    loading,
    error,
    activeCount,
    bootstrap,
    refresh,
    start,
    startNetease,
    startSource,
    cancel,
    remove,
    resume,
    restart,
    percent,
    speed,
    dispose,
  };
});
