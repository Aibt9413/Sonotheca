import { createApp } from "vue";
import { createPinia } from "pinia";
import App from "@/App.vue";
import { router } from "@/router";
import { useThemeStore } from "@/stores/theme";
import "@/styles/theme.css";

const app = createApp(App);

app.use(createPinia());

// 必须在 mount 之前注入种子色与 data-theme，否则首帧会闪白/闪错色
useThemeStore().init();

app.use(router);
app.mount("#app");
