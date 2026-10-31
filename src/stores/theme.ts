import { defineStore } from "pinia";
import { computed, ref, watch } from "vue";
import {
  argbFromHex,
  hexFromArgb,
  themeFromSourceColor,
} from "@material/material-color-utilities";

export type ThemeMode = "light" | "dark" | "system";
type Scheme = "light" | "dark";

const MODE_KEY = "sonotheca.theme.mode";
const SEED_KEY = "sonotheca.theme.seed";
const WALLPAPER_KEY = "sonotheca.theme.wallpaper";
const MOTION_KEY = "sonotheca.theme.motion";
const GLASS_KEY = "sonotheca.theme.glass";
/** M7 之前“底栏柔光”存在 settings 里，读取一次做迁移 */
const LEGACY_SETTINGS_KEY = "sonotheca.settings";

/** 与 src-tauri/icons/icon.png 的薄荷色一致 */
export const DEFAULT_SEED = "#2BD4B4";

/** MD3 角色 → CSS 变量后缀（生成 `--md-<token>`） */
const ROLE_TO_TOKEN: ReadonlyArray<readonly [string, string]> = [
  ["primary", "primary"],
  ["onPrimary", "on-primary"],
  ["primaryContainer", "primary-container"],
  ["onPrimaryContainer", "on-primary-container"],
  ["secondary", "secondary"],
  ["onSecondary", "on-secondary"],
  ["secondaryContainer", "secondary-container"],
  ["onSecondaryContainer", "on-secondary-container"],
  ["tertiary", "tertiary"],
  ["error", "error"],
  ["errorContainer", "error-container"],
  ["onErrorContainer", "on-error-container"],
  ["surface", "surface"],
  ["surfaceDim", "surface-dim"],
  ["surfaceContainerLowest", "surface-container-lowest"],
  ["surfaceContainerLow", "surface-container-low"],
  ["surfaceContainer", "surface-container"],
  ["surfaceContainerHigh", "surface-container-high"],
  ["surfaceContainerHighest", "surface-container-highest"],
  ["onSurface", "on-surface"],
  ["onSurfaceVariant", "on-surface-variant"],
  ["outline", "outline"],
  ["outlineVariant", "outline-variant"],
  ["inverseSurface", "inverse-surface"],
  ["inverseOnSurface", "inverse-on-surface"],
  ["inversePrimary", "inverse-primary"],
];

const media = window.matchMedia("(prefers-color-scheme: dark)");
const isHex = (v: string | null): v is string => !!v && /^#[0-9a-fA-F]{6}$/.test(v);
const isWallpaper = (v: string | null): v is string =>
  !!v && /^data:image\/[a-z0-9.+-]+;base64,/i.test(v);

function readMode(): ThemeMode {
  const v = localStorage.getItem(MODE_KEY);
  return v === "light" || v === "dark" || v === "system" ? v : "system";
}

function readSeed(): string {
  const raw = localStorage.getItem(SEED_KEY);
  return isHex(raw) ? raw : DEFAULT_SEED;
}

/** 旧版本只有 settings.bottomBarGlow，没有 key 时回落到它 */
function readGlass(): boolean {
  const raw = localStorage.getItem(GLASS_KEY);
  if (raw === "on") return true;
  if (raw === "off") return false;

  try {
    const legacy = JSON.parse(
      localStorage.getItem(LEGACY_SETTINGS_KEY) ?? "{}",
    ) as { bottomBarGlow?: boolean };
    if (typeof legacy.bottomBarGlow === "boolean") return legacy.bottomBarGlow;
  } catch {
    // 坏 JSON 直接忽略
  }

  return true;
}

function readMotion(): boolean {
  return localStorage.getItem(MOTION_KEY) !== "off";
}

export const useThemeStore = defineStore("theme", () => {
  const mode = ref<ThemeMode>(readMode());
  const seed = ref<string>(readSeed());
  const systemDark = ref(media.matches);
  const wallpaper = ref<string | null>(
    isWallpaper(localStorage.getItem(WALLPAPER_KEY))
      ? localStorage.getItem(WALLPAPER_KEY)
      : null,
  );
  const motion = ref<boolean>(readMotion());
  const glass = ref<boolean>(readGlass());

  const resolved = computed<Scheme>(() =>
    mode.value === "system" ? (systemDark.value ? "dark" : "light") : mode.value,
  );

  /** 把种子色算成两套 scheme，写进 <style id="md-dynamic">，覆盖静态兜底 */
  function injectSeed(hex: string) {
    const { schemes } = themeFromSourceColor(argbFromHex(hex));
    const blocks: string[] = [];

    for (const key of ["light", "dark"] as const) {
      // material-color-utilities 的 Scheme 类型未导出全部 surface 角色，这里按字典读
      const scheme = schemes[key] as unknown as Record<string, number>;
      const decls = ROLE_TO_TOKEN.map(([role, token]) => {
        const argb = scheme[role];
        return typeof argb === "number" ? `--md-${token}:${hexFromArgb(argb)};` : "";
      }).join("");
      blocks.push(`[data-theme="${key}"]{${decls}}`);
    }

    let el = document.getElementById("md-dynamic") as HTMLStyleElement | null;
    if (!el) {
      el = document.createElement("style");
      el.id = "md-dynamic";
      document.head.appendChild(el);
    }
    el.textContent = blocks.join("\n");
  }

  /**
   * 一次性把所有外观状态写到 <html>：
   * - data-theme / color-scheme：明暗
   * - data-motion：全局动效
   * - data-glass：全局柔光
   * - data-wallpaper + --app-wallpaper：根元素壁纸
   */
  function applyAppearance() {
    const root = document.documentElement;
    root.dataset.theme = resolved.value;
    root.style.colorScheme = resolved.value;
    root.dataset.motion = motion.value ? "on" : "off";
    root.dataset.glass = glass.value ? "on" : "off";
    root.dataset.wallpaper = wallpaper.value ? "on" : "off";

    if (wallpaper.value) {
      root.style.setProperty("--app-wallpaper", `url("${wallpaper.value}")`);
    } else {
      root.style.removeProperty("--app-wallpaper");
    }
  }

  function setMode(next: ThemeMode) {
    mode.value = next;
    localStorage.setItem(MODE_KEY, next);
  }

  function setSeed(hex: string) {
    const normalized = hex.trim().toUpperCase();
    if (!isHex(normalized)) return;
    seed.value = normalized;
    localStorage.setItem(SEED_KEY, normalized);
  }

  /**
   * 壁纸写入 localStorage。图片已在 utils/wallpaper 里压到 1600px JPEG，
   * 这里仍兜住配额异常：写失败时回滚状态，让页面提示换图。
   */
  function setWallpaper(dataUrl: string | null): boolean {
    if (dataUrl !== null && !isWallpaper(dataUrl)) return false;

    const previous = wallpaper.value;
    try {
      if (dataUrl) localStorage.setItem(WALLPAPER_KEY, dataUrl);
      else localStorage.removeItem(WALLPAPER_KEY);
    } catch {
      return false;
    }

    wallpaper.value = dataUrl;
    applyAppearance();
    return true;
  }

  function clearWallpaper() {
    setWallpaper(null);
  }

  function setMotion(next: boolean) {
    motion.value = next;
    localStorage.setItem(MOTION_KEY, next ? "on" : "off");
  }

  function setGlass(next: boolean) {
    glass.value = next;
    localStorage.setItem(GLASS_KEY, next ? "on" : "off");
    // 顺手清掉旧设置里的 bottomBarGlow，避免以后读回陈旧值
    try {
      const legacy = JSON.parse(
        localStorage.getItem(LEGACY_SETTINGS_KEY) ?? "{}",
      ) as Record<string, unknown>;
      if ("bottomBarGlow" in legacy) {
        delete legacy.bottomBarGlow;
        localStorage.setItem(LEGACY_SETTINGS_KEY, JSON.stringify(legacy));
      }
    } catch {
      // 旧数据坏了不影响新开关
    }
  }

  /** 只允许调用一次：main.ts 在 mount 前调用 */
  function init() {
    injectSeed(seed.value);
    applyAppearance();

    media.addEventListener("change", (e) => {
      systemDark.value = e.matches;
    });

    watch([resolved, seed], () => {
      injectSeed(seed.value);
      applyAppearance();
    });

    watch([motion, glass, wallpaper], applyAppearance);
  }

  return {
    mode,
    seed,
    resolved,
    wallpaper,
    motion,
    glass,
    init,
    setMode,
    setSeed,
    setWallpaper,
    clearWallpaper,
    setMotion,
    setGlass,
  };
});
