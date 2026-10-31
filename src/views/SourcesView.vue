<script setup lang="ts">
import { onActivated, onDeactivated, onMounted, onUnmounted, ref } from "vue";
import { useRouter } from "vue-router";
import QRCode from "qrcode";
import SCard from "@/ui/SCard.vue";
import { useBackGuard } from "@/composables/useBackGuard";
import { useAccountStore } from "@/stores/account";
import { useToastStore } from "@/stores/toast";
import {
  bilibiliLogout,
  bilibiliQrPoll,
  bilibiliQrStart,
  bilibiliStatus,
  captureWebLogin,
  closeWebLogin,
  describeError,
  isTauri,
  navidromeLogin,
  navidromeStatus,
  neteaseQrCheck,
  neteaseQrKey,
  openWebLogin,
  qqStatus,
  type BilibiliProfile,
  type WebLoginSource,
} from "@/ipc/bindings";

defineOptions({ name: "SourcesView" });

/**
 * P2：账号与数据源聚合页。
 *
 * 设置页只保留入口，避免网易云 / Navidrome 的状态在两处各存一份。
 * 这里复用 account store 与 navidrome_status / navidrome_login，
 * 不新增任何后端命令。
 */
const router = useRouter();
const account = useAccountStore();
const toast = useToastStore();

function backToSettings() {
  void router.push("/settings");
}

// Android 返回手势：回到设置页，而不是直接退出 App
useBackGuard(() => true, backToSettings);

// ── 网易云扫码登录 ─────────────────────────────────────────
const qrImage = ref<string | null>(null);
const qrStatus = ref<string | null>(null);
const loggingIn = ref(false);
let pollTimer: ReturnType<typeof setInterval> | null = null;

function stopPolling() {
  if (pollTimer) {
    clearInterval(pollTimer);
    pollTimer = null;
  }
}

async function startLogin() {
  if (loggingIn.value) return;
  loggingIn.value = true;
  qrStatus.value = "正在获取二维码…";
  qrImage.value = null;
  stopPolling();

  try {
    const key = await neteaseQrKey();
    // 网易云 App 扫码后跳转的地址
    const content = `https://music.163.com/login?codekey=${key}`;
    qrImage.value = await QRCode.toDataURL(content, { width: 240, margin: 1 });
    qrStatus.value = "请用「网易云音乐」App 扫码";

    pollTimer = setInterval(async () => {
      try {
        const state = await neteaseQrCheck(key);
        if (state.code === 800) {
          stopPolling();
          loggingIn.value = false;
          qrStatus.value = "二维码已过期，请重新获取";
        } else if (state.code === 801) {
          qrStatus.value = "等待扫码…";
        } else if (state.code === 802) {
          qrStatus.value = "已扫码，请在手机上确认";
        } else if (state.code === 803) {
          stopPolling();
          loggingIn.value = false;
          qrImage.value = null;
          qrStatus.value = "登录成功";
          await account.refresh();
          toast.show("网易云已登录");
        }
      } catch (e) {
        stopPolling();
        loggingIn.value = false;
        qrStatus.value = describeError(e);
      }
    }, 2500);
  } catch (e) {
    loggingIn.value = false;
    qrStatus.value = describeError(e);
  }
}

async function doLogout() {
  stopPolling();
  qrImage.value = null;
  qrStatus.value = null;
  await account.logout();
  toast.show("已退出网易云");
}

// ── Navidrome（Subsonic） ───────────────────────────────────
const ndUrl = ref("http://127.0.0.1:4533");
const ndUser = ref("");
const ndPassword = ref("");
const ndStatus = ref<string | null>(null);
const ndError = ref<string | null>(null);
const ndSaving = ref(false);
const ndConnected = ref(false);

async function loadNavidrome() {
  if (!isTauri()) return;
  try {
    const status = await navidromeStatus();
    ndConnected.value = status.configured;
    if (status.url) ndUrl.value = status.url;
    if (status.user) ndUser.value = status.user;
    ndStatus.value = status.configured ? `${status.url}（${status.user || "匿名"}）` : null;
  } catch (e) {
    ndError.value = describeError(e);
  }
}

async function loginNavidrome() {
  ndSaving.value = true;
  ndError.value = null;
  try {
    const status = await navidromeLogin(
      ndUrl.value.trim(),
      ndUser.value.trim(),
      ndPassword.value,
    );
    ndConnected.value = true;
    ndStatus.value = `${status.url}（${status.user || "匿名"}）`;
    ndPassword.value = "";
    toast.show("Navidrome 已连接");
  } catch (e) {
    ndError.value = describeError(e);
  } finally {
    ndSaving.value = false;
  }
}

function reloginNavidrome() {
  ndConnected.value = false;
  ndPassword.value = "";
  ndError.value = null;
}

// ── B 站扫码（M8） ─────────────────────────────────────────
const biliProfile = ref<BilibiliProfile | null>(null);
const biliLoggedIn = ref(false);
const biliQrImage = ref<string | null>(null);
const biliQrStatus = ref<string | null>(null);
const biliLoggingIn = ref(false);
let biliPollTimer: ReturnType<typeof setInterval> | null = null;

function stopBiliPolling() {
  if (biliPollTimer) {
    clearInterval(biliPollTimer);
    biliPollTimer = null;
  }
}

async function loadBilibili() {
  if (!isTauri()) return;
  try {
    const status = await bilibiliStatus();
    biliLoggedIn.value = status.loggedIn;
    biliProfile.value = status.profile;
  } catch {
    // 网络失败不把界面锁死，保持未登录
    biliLoggedIn.value = false;
    biliProfile.value = null;
  }
}

async function startBiliLogin() {
  if (biliLoggingIn.value) return;
  biliLoggingIn.value = true;
  biliQrStatus.value = "正在获取二维码…";
  biliQrImage.value = null;
  stopBiliPolling();

  try {
    const qr = await bilibiliQrStart();
    biliQrImage.value = await QRCode.toDataURL(qr.url, { width: 240, margin: 1 });
    biliQrStatus.value = "请用「哔哩哔哩」App 扫码";

    biliPollTimer = setInterval(async () => {
      try {
        const state = await bilibiliQrPoll(qr.key);
        if (state.code === 0) {
          stopBiliPolling();
          biliLoggingIn.value = false;
          biliQrImage.value = null;
          biliQrStatus.value = "登录成功";
          await loadBilibili();
          toast.show("B站已登录");
        } else if (state.code === 86038) {
          stopBiliPolling();
          biliLoggingIn.value = false;
          biliQrImage.value = null;
          biliQrStatus.value = state.message || "二维码已过期";
        } else {
          biliQrStatus.value = state.message;
        }
      } catch (e) {
        stopBiliPolling();
        biliLoggingIn.value = false;
        biliQrStatus.value = describeError(e);
      }
    }, 2200);
  } catch (e) {
    biliLoggingIn.value = false;
    biliQrStatus.value = describeError(e);
  }
}

async function doBiliLogout() {
  stopBiliPolling();
  biliQrImage.value = null;
  biliQrStatus.value = null;
  try {
    await bilibiliLogout();
  } finally {
    biliLoggedIn.value = false;
    biliProfile.value = null;
    toast.show("已退出 B站");
  }
}

// ── 全平台网页登录兜底 ─────────────────────────────────────
const qqLoggedIn = ref(false);
const webLoggingIn = ref<WebLoginSource | null>(null);
let webLoginTimer: ReturnType<typeof setInterval> | null = null;
let webLoginAttempts = 0;

function stopWebLoginPolling() {
  if (webLoginTimer) {
    clearInterval(webLoginTimer);
    webLoginTimer = null;
  }
  webLoggingIn.value = null;
  webLoginAttempts = 0;
}

async function loadQqStatus() {
  if (!isTauri()) return;
  try {
    qqLoggedIn.value = (await qqStatus()).loggedIn;
  } catch {
    qqLoggedIn.value = false;
  }
}

async function startWebLogin(source: WebLoginSource) {
  if (webLoggingIn.value) return;
  webLoggingIn.value = source;
  webLoginAttempts = 0;

  try {
    await openWebLogin(source);
    toast.show("请在弹出的网页中完成登录");

    webLoginTimer = setInterval(async () => {
      webLoginAttempts += 1;
      try {
        const state = await captureWebLogin(source);
        if (state.loggedIn) {
          stopWebLoginPolling();
          await closeWebLogin(source);
          toast.show("网页登录成功");
          if (source === "netease") await account.refresh();
          if (source === "bilibili") await loadBilibili();
          if (source === "qq") qqLoggedIn.value = true;
        } else if (webLoginAttempts >= 90) {
          stopWebLoginPolling();
          await closeWebLogin(source);
          toast.show("网页登录超时，请重试", "warning");
        }
      } catch {
        // 用户可能还没登录完，继续轮询
      }
    }, 2000);
  } catch (e) {
    stopWebLoginPolling();
    toast.show(describeError(e), "warning");
  }
}

onMounted(() => {
  void account.refresh();
  void loadNavidrome();
  void loadBilibili();
  void loadQqStatus();
});

onActivated(() => {
  void account.refresh();
  void loadBilibili();
  void loadQqStatus();
});

onDeactivated(() => {
  stopPolling();
  stopBiliPolling();
  stopWebLoginPolling();
  loggingIn.value = false;
  biliLoggingIn.value = false;
  qrImage.value = null;
  qrStatus.value = null;
  biliQrImage.value = null;
  biliQrStatus.value = null;
});

onUnmounted(() => {
  stopPolling();
  stopBiliPolling();
  stopWebLoginPolling();
});
</script>

<template>
  <div class="flex w-full flex-col gap-3">
  <!-- 网易云 -->
  <SCard class="order-1">
    <div class="flex items-center gap-2">
      <span
        class="grid size-8 shrink-0 place-items-center rounded-pill bg-[#c62f2f] text-[10px] font-semibold text-white"
        aria-hidden="true"
      >
        云
      </span>
      <p class="min-w-0 flex-1 text-sm font-medium">网易云音乐</p>
      <span
        class="shrink-0 rounded-pill px-2 py-0.5 text-[9px]"
        :class="
          account.loggedIn
            ? 'bg-primary-container text-on-primary-container'
            : 'bg-surface-high text-on-surface-variant'
        "
      >
        {{ account.loggedIn ? "已登录" : "未登录" }}
      </span>
    </div>

    <!-- 已登录：头像 + 昵称 + 退出 -->
    <div v-if="account.loggedIn" class="mt-3 flex items-center gap-3">
      <img
        v-if="account.profile?.avatarUrl"
        :src="account.profile.avatarUrl"
        alt=""
        referrerpolicy="no-referrer"
        class="size-11 rounded-pill object-cover"
      />
      <div
        v-else
        class="grid size-11 shrink-0 place-items-center rounded-pill bg-surface-highest text-on-surface-variant"
      >
        <span class="text-xs">♪</span>
      </div>
      <div class="min-w-0 flex-1">
        <p class="truncate text-sm font-medium">{{ account.profile?.nickname || "已登录" }}</p>
        <p class="text-xs text-on-surface-variant">已解锁无损及以上音质</p>
      </div>
      <button
        type="button"
        class="shrink-0 rounded-pill bg-surface-high px-4 py-2 text-xs text-on-surface-variant"
        @click="doLogout"
      >
        退出
      </button>
    </div>

    <!-- 未登录：二维码 -->
    <div v-else class="mt-3">
      <div class="flex items-center justify-between gap-2">
        <p class="text-xs text-on-surface-variant">扫码或网页登录</p>
        <div class="flex shrink-0 gap-1">
          <button
            v-if="false"
            type="button"
            class="rounded-pill bg-surface-high px-3 py-2 text-[11px] text-on-surface-variant"
            :disabled="!!webLoggingIn"
            @click="startWebLogin('netease')"
          >
            网页登录
          </button>
          <button
            type="button"
            class="rounded-pill bg-primary-container px-3 py-2 text-[11px] font-medium text-on-primary-container"
            @click="startLogin"
          >
            {{ loggingIn ? "登录中…" : "扫码" }}
          </button>
        </div>
      </div>

      <div v-if="qrImage" class="mt-3 flex flex-col items-center gap-2">
        <img :src="qrImage" alt="登录二维码" class="rounded-card bg-white p-2" />
        <p class="text-xs text-on-surface-variant">{{ qrStatus }}</p>
      </div>
      <p v-else-if="qrStatus" class="mt-2 text-xs text-on-surface-variant">{{ qrStatus }}</p>
    </div>
  </SCard>

  <!-- Navidrome -->
  <SCard class="order-4">
    <div class="flex items-center gap-2">
      <span
        class="grid size-8 shrink-0 place-items-center rounded-pill bg-[#4a90d9] text-[10px] font-semibold text-white"
        aria-hidden="true"
      >
        N
      </span>
      <p class="min-w-0 flex-1 text-sm font-medium">Navidrome 音乐库</p>
      <span
        class="shrink-0 rounded-pill px-2 py-0.5 text-[9px]"
        :class="
          ndConnected
            ? 'bg-primary-container text-on-primary-container'
            : 'bg-surface-high text-on-surface-variant'
        "
      >
        {{ ndConnected ? "已连接" : "未配置" }}
      </span>
    </div>

    <!-- 已连接：只显示地址与用户名，密码永不回显 -->
    <div v-if="ndConnected" class="mt-3 flex items-center gap-3 rounded-card bg-surface-low p-3">
      <div
        class="grid size-9 shrink-0 place-items-center rounded-pill bg-primary-container text-on-primary-container"
      >
        ✓
      </div>
      <div class="min-w-0 flex-1">
        <p class="truncate text-xs font-medium">{{ ndStatus }}</p>
        <p class="text-[10px] text-on-surface-variant">已连接 · 密码已隐藏</p>
      </div>
      <button
        type="button"
        class="shrink-0 rounded-pill bg-surface-high px-3 py-1.5 text-[11px] text-on-surface-variant"
        @click="reloginNavidrome"
      >
        重新登录
      </button>
    </div>

    <!-- 未连接：登录表单 -->
    <div v-else class="mt-3 flex flex-col gap-2">
      <input
        v-model="ndUrl"
        type="text"
        spellcheck="false"
        class="rounded-pill bg-surface-low px-4 py-2 font-mono text-xs outline-none"
        placeholder="http://127.0.0.1:4533"
      />
      <div class="flex gap-2">
        <input
          v-model="ndUser"
          type="text"
          spellcheck="false"
          class="min-w-0 flex-1 rounded-pill bg-surface-low px-4 py-2 text-xs outline-none"
          placeholder="用户名"
        />
        <input
          v-model="ndPassword"
          type="password"
          class="min-w-0 flex-1 rounded-pill bg-surface-low px-4 py-2 text-xs outline-none"
          placeholder="密码"
          @keyup.enter="loginNavidrome"
        />
      </div>
      <button
        type="button"
        class="rounded-pill bg-primary-container px-4 py-2 text-xs font-medium text-on-primary-container disabled:opacity-50"
        :disabled="ndSaving || !ndUrl.trim()"
        @click="loginNavidrome"
      >
        {{ ndSaving ? "连接中…" : "登录" }}
      </button>
    </div>

    <p v-if="ndError" class="mt-2 text-xs text-error">{{ ndError }}</p>
  </SCard>

  <!-- QQ 音乐：网页登录兜底；扫码暂不支持 -->
  <SCard class="order-2">
    <div class="flex items-center gap-2">
      <span
        class="grid size-8 shrink-0 place-items-center rounded-pill bg-[#31c27c] text-[10px] font-semibold text-white"
        aria-hidden="true"
      >
        QQ
      </span>
      <p class="min-w-0 flex-1 text-sm font-medium">QQ 音乐</p>
      <span
        class="shrink-0 rounded-pill px-2 py-0.5 text-[9px]"
        :class="
          qqLoggedIn
            ? 'bg-primary-container text-on-primary-container'
            : 'bg-surface-high text-on-surface-variant'
        "
      >
        {{ qqLoggedIn ? "已登录" : "未登录" }}
      </span>
    </div>
    <button
      v-if="false"
      type="button"
      class="mt-3 w-full rounded-pill bg-surface-high py-2.5 text-xs text-on-surface-variant disabled:opacity-50"
      :disabled="!!webLoggingIn || qqLoggedIn"
      @click="startWebLogin('qq')"
    >
      {{ qqLoggedIn ? "已登录（网页）" : "网页登录" }}
    </button>
  </SCard>

  <!-- B 站：扫码登录 + 匿名可用 -->
  <SCard class="order-3">
    <div class="flex items-center gap-2">
      <span
        class="grid size-8 shrink-0 place-items-center rounded-pill bg-[#fb7299] text-[10px] font-semibold text-white"
        aria-hidden="true"
      >
        B
      </span>
      <p class="min-w-0 flex-1 text-sm font-medium">B 站</p>
      <span
        class="shrink-0 rounded-pill px-2 py-0.5 text-[9px]"
        :class="
          biliLoggedIn
            ? 'bg-primary-container text-on-primary-container'
            : 'bg-surface-high text-on-surface-variant'
        "
      >
        {{ biliLoggedIn ? "已登录" : "未登录" }}
      </span>
    </div>

    <div v-if="biliLoggedIn" class="mt-3 flex items-center gap-3">
      <img
        v-if="biliProfile?.avatarUrl"
        :src="biliProfile.avatarUrl"
        alt=""
        referrerpolicy="no-referrer"
        class="size-11 rounded-pill object-cover"
      />
      <div
        v-else
        class="grid size-11 shrink-0 place-items-center rounded-pill bg-surface-highest text-on-surface-variant"
      >
        <span class="text-xs">B</span>
      </div>
      <div class="min-w-0 flex-1">
        <p class="truncate text-sm font-medium">{{ biliProfile?.nickname || "B站用户" }}</p>
        <p class="text-[10px] text-on-surface-variant">UID {{ biliProfile?.mid || "—" }}</p>
      </div>
      <button
        type="button"
        class="shrink-0 rounded-pill bg-surface-high px-4 py-2 text-xs text-on-surface-variant"
        @click="doBiliLogout"
      >
        退出
      </button>
    </div>

    <div v-else class="mt-3">
      <div class="flex items-center justify-between gap-2">
        <p class="text-xs text-on-surface-variant">扫码或网页登录</p>
        <div class="flex shrink-0 gap-1">
          <button
            v-if="false"
            type="button"
            class="rounded-pill bg-surface-high px-3 py-2 text-[11px] text-on-surface-variant"
            :disabled="!!webLoggingIn"
            @click="startWebLogin('bilibili')"
          >
            网页登录
          </button>
          <button
            type="button"
            class="rounded-pill bg-primary-container px-3 py-2 text-[11px] font-medium text-on-primary-container"
            @click="startBiliLogin"
          >
            {{ biliLoggingIn ? "登录中…" : "扫码" }}
          </button>
        </div>
      </div>

      <div v-if="biliQrImage" class="mt-3 flex flex-col items-center gap-2">
        <img :src="biliQrImage" alt="B站登录二维码" class="rounded-card bg-white p-2" />
        <p class="text-xs text-on-surface-variant">{{ biliQrStatus }}</p>
      </div>
      <p v-else-if="biliQrStatus" class="mt-2 text-xs text-on-surface-variant">{{ biliQrStatus }}</p>
    </div>
  </SCard>

  <SCard tone="low" class="order-5">
    <p class="text-xs text-on-surface-variant">凭据仅保存在本机。</p>
  </SCard>
  </div>
</template>
