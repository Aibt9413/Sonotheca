import { defineStore } from "pinia";
import { ref } from "vue";

/**
 * 全局浮层状态。
 *
 * FileManager 和全屏播放器有自己的底栏，挂载时必须把全局 Tab 栏藏掉，
 * 否则玻璃层下会透出第二套导航，也会挡住「选择此文件夹」。
 */
export const useUiStore = defineStore("ui", () => {
  const managerOpen = ref(false);
  /** 任意 Bottom Sheet / 全屏浮层打开：底栏必须让位 */
  const sheetOpen = ref(false);

  function setManagerOpen(open: boolean) {
    managerOpen.value = open;
  }

  function setSheetOpen(open: boolean) {
    sheetOpen.value = open;
  }

  return { managerOpen, sheetOpen, setManagerOpen, setSheetOpen };
});
