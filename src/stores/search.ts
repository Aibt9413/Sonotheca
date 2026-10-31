import { defineStore } from "pinia";
import { ref } from "vue";
import type { SearchHit, SourceFailure } from "@/ipc/bindings";

/**
 * 搜索页状态。
 *
 * 配合 App.vue 的 `<KeepAlive include="SearchView">`：切到别的 Tab 再切回来，
 * 关键词与结果列表原封不动；「一键清空」也只动这里。
 *
 * `error` 只放整次搜索失败（invoke 本身异常）；单源失败进 `failures`，
 * 由 SearchView 按音源分行展示，不能因为 QQ 挂了就让整页变成一片红。
 */
export const useSearchStore = defineStore("search", () => {
  const keyword = ref("");
  const results = ref<SearchHit[]>([]);
  const searched = ref(false);
  const error = ref<string | null>(null);
  const failures = ref<SourceFailure[]>([]);

  function reset() {
    keyword.value = "";
    results.value = [];
    searched.value = false;
    error.value = null;
    failures.value = [];
  }

  return { keyword, results, searched, error, failures, reset };
});
