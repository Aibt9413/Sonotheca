import { defineStore } from "pinia";
import { ref } from "vue";

/** default = 反色 Snackbar；warning = 黄色警示（音质降级等） */
export type ToastTone = "default" | "warning";

/**
 * 全局轻提示（MD3 Snackbar 风格）。
 *
 * 只承载「不阻断操作」的一句话反馈（音质自动降级、路径保存成功等）；
 * 需要用户确认的流程仍然走 Bottom Sheet。
 */
export const useToastStore = defineStore("toast", () => {
  const message = ref<string | null>(null);
  const tone = ref<ToastTone>("default");
  let timer: ReturnType<typeof setTimeout> | null = null;

  function clearTimer() {
    if (timer) {
      clearTimeout(timer);
      timer = null;
    }
  }

  /** 同一条消息重复触发时重置计时，而不是叠加 */
  function show(text: string, nextTone: ToastTone = "default", duration = 3400) {
    clearTimer();
    message.value = text;
    tone.value = nextTone;
    timer = setTimeout(() => {
      message.value = null;
      timer = null;
    }, duration);
  }

  function hide() {
    clearTimer();
    message.value = null;
  }

  return { message, tone, show, hide };
});
