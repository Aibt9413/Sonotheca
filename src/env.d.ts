/// <reference types="vite/client" />

interface ImportMetaEnv {
  /** Tauri CLI 在 dev/build 时注入：windows | android | darwin | linux */
  readonly TAURI_ENV_PLATFORM?: string;
  readonly TAURI_ENV_DEBUG?: string;
}

interface ImportMeta {
  readonly env: ImportMetaEnv;
}
