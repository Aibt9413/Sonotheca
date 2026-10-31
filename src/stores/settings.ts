import { defineStore } from "pinia";
import { ref, watch } from "vue";
import {
  isTauri,
  neteaseDefaultQuality,
  setNeteaseDefaultQuality,
  type ConcreteQuality,
} from "@/ipc/bindings";

/**
 * 设置的真源分两层：
 *   - 与 Rust 有关的（默认音质）落 settings 表，启动时 hydrate；
 *   - 纯行为偏好的（下载并发）仍落 localStorage。
 * 外观（明暗 / 主色 / 壁纸 / 动效 / 柔光）已整体迁移到 stores/theme.ts。
 * localStorage 同时充当启动缓存，读库失败时 UI 不至于闪回默认值。
 */
export interface AppSettings {
  /** 下载并发数，M1 才开始生效 */
  downloadConcurrency: number;
  /** 默认下载音质 */
  defaultQuality: ConcreteQuality;
}

const KEY = "sonotheca.settings";
const DEFAULTS: AppSettings = {
  downloadConcurrency: 3,
  defaultQuality: "lossless",
};

export const useSettingsStore = defineStore("settings", () => {
  const settings = ref<AppSettings>({ ...DEFAULTS });

  try {
    const raw = localStorage.getItem(KEY);
    if (raw) settings.value = { ...DEFAULTS, ...(JSON.parse(raw) as Partial<AppSettings>) };
  } catch {
    // 本地存储损坏 → 静默回落默认值，不阻断启动
  }

  watch(
    settings,
    (v) => localStorage.setItem(KEY, JSON.stringify(v)),
    { deep: true },
  );

  /** 启动时从 settings 表拉一次默认音质；失败就沿用本地缓存 */
  async function hydrateQuality() {
    if (!isTauri()) return;
    try {
      settings.value.defaultQuality = await neteaseDefaultQuality();
    } catch {
      // 数据库暂时读不到：不阻断 UI，保持缓存值
    }
  }

  async function setDefaultQuality(value: ConcreteQuality) {
    settings.value.defaultQuality = value;
    if (!isTauri()) return;
    // 以 Rust 侧解析结果为准（顺带过滤脏字符串）
    settings.value.defaultQuality = await setNeteaseDefaultQuality(value);
  }

  return { settings, hydrateQuality, setDefaultQuality };
});
