import { defineStore } from "pinia";
import { ref } from "vue";

export type NavDirection = "forward" | "back";

const LAST_SETTINGS_KEY = "sonotheca.nav.lastSettingsPath";
const SETTINGS_PATHS = ["/settings", "/sources", "/theme", "/downloads"] as const;

function isSettingsPath(path: string): boolean {
  return SETTINGS_PATHS.includes(path as (typeof SETTINGS_PATHS)[number]);
}

function readLastSettingsPath(): string {
  const raw = localStorage.getItem(LAST_SETTINGS_KEY);
  return raw && isSettingsPath(raw) ? raw : "/settings";
}

/**
 * 导航状态：
 * - direction：决定页面转场方向（二级页进入 = forward，返回 = back）
 * - lastSettingsPath：设置 Tab 的记忆点，从搜索切回来仍停在 /theme 或 /sources
 */
export const useNavigationStore = defineStore("navigation", () => {
  const direction = ref<NavDirection>("forward");
  const lastSettingsPath = ref(readLastSettingsPath());

  function setDirection(next: NavDirection) {
    direction.value = next;
  }

  function remember(path: string) {
    if (!isSettingsPath(path)) return;
    lastSettingsPath.value = path;
    localStorage.setItem(LAST_SETTINGS_KEY, path);
  }

  /** 设置 Tab 的目标：已经处在设置子页时留在原地，否则回到上次的子页 */
  function settingsTarget(currentPath: string): string {
    if (isSettingsPath(currentPath)) return currentPath;
    return lastSettingsPath.value;
  }

  return {
    direction,
    lastSettingsPath,
    setDirection,
    remember,
    isSettingsPath,
    settingsTarget,
  };
});
