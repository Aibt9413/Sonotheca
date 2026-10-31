<script setup lang="ts">
import { onMounted, ref } from "vue";
import { useRouter } from "vue-router";
import SCard from "@/ui/SCard.vue";
import { useBackGuard } from "@/composables/useBackGuard";
import { useToastStore } from "@/stores/toast";
import {
  describeError,
  getDownloadSettings,
  setDownloadSettings,
  type DownloadSettings,
} from "@/ipc/bindings";

defineOptions({ name: "DownloadSettingsView" });

const router = useRouter();
const toast = useToastStore();

useBackGuard(
  () => true,
  () => void router.push("/settings"),
);

const settings = ref<DownloadSettings | null>(null);
const speedInput = ref("0");
const saving = ref(false);
const error = ref<string | null>(null);

const concurrencyOptions = [1, 2, 3, 4, 5] as const;
const playlistSizeOptions = [
  { value: 50, label: "50" },
  { value: 100, label: "100" },
  { value: 200, label: "200" },
  { value: 0, label: "无限制" },
] as const;

function bytesToMb(value: number): string {
  if (!value) return "0";
  const mb = value / (1024 * 1024);
  return Number.isInteger(mb) ? String(mb) : mb.toFixed(1);
}

async function load() {
  error.value = null;
  try {
    settings.value = await getDownloadSettings();
    speedInput.value = bytesToMb(settings.value.speedLimit);
  } catch (e) {
    error.value = describeError(e);
  }
}

async function update(patch: {
  concurrency?: number;
  speedLimit?: number;
  playlistPageSize?: number;
}) {
  if (saving.value) return;
  saving.value = true;
  error.value = null;
  try {
    settings.value = await setDownloadSettings(patch);
    speedInput.value = bytesToMb(settings.value.speedLimit);
  } catch (e) {
    error.value = describeError(e);
    toast.show(describeError(e), "warning");
  } finally {
    saving.value = false;
  }
}

async function applySpeed() {
  const mb = Number(speedInput.value);
  if (!Number.isFinite(mb) || mb < 0 || mb > 50) {
    error.value = "请输入 0–50 之间的 MB/s 数值";
    return;
  }
  await update({ speedLimit: Math.round(mb * 1024 * 1024) });
}

onMounted(() => {
  void load();
});
</script>

<template>
  <div class="flex w-full flex-col gap-3">
    <SCard>
      <div class="flex items-center justify-between gap-3">
        <p class="text-sm font-medium">同时下载数</p>
        <span class="rounded-pill bg-primary-container px-3 py-1 text-xs font-medium text-on-primary-container">
          {{ settings?.concurrency ?? "—" }}
        </span>
      </div>
      <div class="mt-3 grid grid-cols-5 gap-1 rounded-pill bg-surface-high p-1">
        <button
          v-for="value in concurrencyOptions"
          :key="value"
          type="button"
          class="rounded-pill py-2 text-xs font-medium transition disabled:opacity-50"
          :class="
            settings?.concurrency === value
              ? 'bg-primary text-on-primary shadow-e1'
              : 'text-on-surface-variant'
          "
          :disabled="saving || !settings"
          @click="update({ concurrency: value })"
        >
          {{ value }}
        </button>
      </div>
    </SCard>

    <SCard>
      <div class="flex items-center justify-between gap-3">
        <p class="text-sm font-medium">下载限速</p>
        <span class="text-[11px] text-on-surface-variant">0 = 不限速</span>
      </div>
      <div class="mt-3 flex items-center gap-2">
        <input
          v-model="speedInput"
          type="number"
          inputmode="decimal"
          min="0"
          max="50"
          step="0.1"
          class="min-w-0 flex-1 rounded-pill bg-surface-low px-4 py-2.5 font-mono text-sm outline-none"
          @keyup.enter="applySpeed"
        />
        <span class="shrink-0 text-xs text-on-surface-variant">MB/s</span>
        <button
          type="button"
          class="shrink-0 rounded-pill bg-primary px-5 py-2.5 text-xs font-medium text-on-primary disabled:opacity-50"
          :disabled="saving || !settings"
          @click="applySpeed"
        >
          {{ saving ? "保存中…" : "应用" }}
        </button>
      </div>
      <p v-if="error" class="mt-2 text-xs text-error">{{ error }}</p>
    </SCard>

    <SCard>
      <p class="text-sm font-medium">歌单默认加载数量</p>
      <div class="mt-3 grid grid-cols-4 gap-1 rounded-pill bg-surface-high p-1">
        <button
          v-for="item in playlistSizeOptions"
          :key="item.value"
          type="button"
          class="rounded-pill py-2 text-[11px] font-medium transition disabled:opacity-50"
          :class="
            settings?.playlistPageSize === item.value
              ? 'bg-primary text-on-primary shadow-e1'
              : 'text-on-surface-variant'
          "
          :disabled="saving || !settings"
          @click="update({ playlistPageSize: item.value })"
        >
          {{ item.label }}
        </button>
      </div>
    </SCard>
  </div>
</template>
