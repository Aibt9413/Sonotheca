import { onActivated, onDeactivated, onMounted, onUnmounted, watch } from "vue";
import { onBackButtonPress } from "@tauri-apps/api/app";

/**
 * Android 返回手势守卫。
 *
 * 关键：
 * - `onBackButtonPress` 一旦注册就接管默认行为，所以只在需要时注册、用完立刻注销；
 * - 被 `<KeepAlive>` 缓存的页面在 deactivated 时必须注销，否则切到别的 Tab
 *   还会被这个页面的返回逻辑拦截。
 */
export function useBackGuard(enabled: () => boolean, handler: () => void) {
  type Listener = { unregister: () => void | Promise<void> };

  let unlisten: Listener | null = null;
  let registered = false;
  let active = true;

  function detach() {
    if (!registered) return;
    registered = false;
    void unlisten?.unregister();
    unlisten = null;
  }

  async function sync() {
    const want = active && enabled();
    if (want === registered) return;

    if (want) {
      try {
        const listener = await onBackButtonPress(() => handler());
        // 注册是异步的：await 期间页面可能已经被 deactivate
        if (!active || !enabled()) {
          void listener.unregister();
          return;
        }
        unlisten = listener;
        registered = true;
      } catch {
        // 桌面端/浏览器没有返回键，忽略
        registered = false;
      }
    } else {
      detach();
    }
  }

  onMounted(() => {
    void sync();
    watch(enabled, () => void sync());
  });

  onActivated(() => {
    active = true;
    void sync();
  });

  onDeactivated(() => {
    active = false;
    detach();
  });

  onUnmounted(() => {
    active = false;
    detach();
  });
}
