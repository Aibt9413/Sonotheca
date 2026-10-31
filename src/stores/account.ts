import { defineStore } from "pinia";
import { ref } from "vue";
import {
  isTauri,
  neteaseLogout,
  neteaseStatus,
  type NeteaseProfile,
} from "@/ipc/bindings";

/** 网易云账号状态（登录后解锁无损音质） */
export const useAccountStore = defineStore("account", () => {
  const loggedIn = ref(false);
  const profile = ref<NeteaseProfile | null>(null);
  const loading = ref(false);
  const ready = ref(false);

  async function refresh() {
    if (!isTauri()) {
      ready.value = true;
      return;
    }
    loading.value = true;
    try {
      const status = await neteaseStatus();
      loggedIn.value = status.loggedIn;
      profile.value = status.profile;
    } catch {
      // 网络异常不该让界面以为已登录
      loggedIn.value = false;
      profile.value = null;
    } finally {
      loading.value = false;
      ready.value = true;
    }
  }

  async function logout() {
    try {
      await neteaseLogout();
    } finally {
      loggedIn.value = false;
      profile.value = null;
    }
  }

  return { loggedIn, profile, loading, ready, refresh, logout };
});
