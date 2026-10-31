import type { QualityFallback } from "@/ipc/bindings";
import type { ToastTone } from "@/stores/toast";

/**
 * 降级原因 → 用户可读的 Toast。
 *
 * 「母带/沉浸声拿不到 → 无损」用黄色警示，其余（未登录）用普通反色提示。
 */
export function fallbackToast(
  fallback: QualityFallback,
): { text: string; tone: ToastTone } | null {
  switch (fallback) {
    case "anonymous":
      return { text: "未登录网易云，已自动降级至极高音质(320K)", tone: "default" };
    case "lossless":
      return { text: "母带/沉浸声获取失败，已自动降级为无损", tone: "warning" };
    case "exhigh":
      return { text: "所选音质不可用，已自动降级至极高 320K", tone: "warning" };
    case "standard":
      return { text: "所选音质不可用，已自动降级为标准 128K", tone: "warning" };
    default:
      return null;
  }
}
