import { createRouter, createWebHashHistory, type RouteRecordRaw } from "vue-router";
import { useNavigationStore } from "@/stores/navigation";

const routes: RouteRecordRaw[] = [
  {
    path: "/",
    name: "search",
    component: () => import("@/views/SearchView.vue"),
    meta: { depth: 0, title: "Sonotheca" },
  },
  {
    path: "/library",
    name: "library",
    component: () => import("@/views/LibraryView.vue"),
    meta: { depth: 0, title: "音乐库" },
  },
  {
    path: "/tasks",
    name: "tasks",
    component: () => import("@/views/TasksView.vue"),
    meta: { depth: 0, title: "任务" },
  },
  {
    path: "/settings",
    name: "settings",
    component: () => import("@/views/SettingsView.vue"),
    meta: { depth: 0, title: "设置" },
  },
  {
    path: "/sources",
    name: "sources",
    component: () => import("@/views/SourcesView.vue"),
    meta: { depth: 1, title: "账号与数据源", parent: "/settings" },
  },
  {
    path: "/theme",
    name: "theme",
    component: () => import("@/views/ThemeSettings.vue"),
    meta: { depth: 1, title: "主题外观", parent: "/settings" },
  },
  {
    path: "/playlist",
    name: "playlist",
    component: () => import("@/views/PlaylistView.vue"),
    meta: { depth: 1, title: "歌单", parent: "/" },
  },
  {
    path: "/downloads",
    name: "downloads",
    component: () => import("@/views/DownloadSettingsView.vue"),
    meta: { depth: 1, title: "下载设置", parent: "/settings" },
  },
  {
    path: "/my-playlists",
    name: "my-playlists",
    component: () => import("@/views/MyPlaylistsView.vue"),
    meta: { depth: 1, title: "我的歌单", parent: "/" },
  },
  {
    path: "/about",
    name: "about",
    component: () => import("@/views/AboutView.vue"),
    meta: { depth: 1, title: "关于", parent: "/settings" },
  },
  { path: "/:pathMatch(.*)*", redirect: "/" },
];

// Tauri 下用 hash 模式：避免自定义协议下 history 路由刷新 404
export const router = createRouter({
  history: createWebHashHistory(),
  routes,
});

router.beforeEach((to, from) => {
  const nav = useNavigationStore();
  const toDepth = Number(to.meta.depth ?? 0);
  const fromDepth = Number(from.meta.depth ?? 0);
  // 进入更深的页面 = 前进；回到更浅的页面 = 返回
  nav.setDirection(toDepth < fromDepth ? "back" : "forward");
});

router.afterEach((to) => {
  useNavigationStore().remember(to.path);
});
