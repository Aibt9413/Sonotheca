//! Android 前台服务桥接。
//!
//! 前端只认一个命令 `set_fgs_count`，Rust 侧再转发到 Kotlin 的 `FgsPlugin`：
//!   JS ──invoke──▶ set_fgs_count ──run_mobile_plugin──▶ FgsPlugin.kt ──▶ DownloadService
//!
//! 为什么不让 JS 直接 `invoke("plugin:fgs|start")`：那样需要 Rust 侧存在同名插件
//! 的 IPC 路由。用 `register_android_plugin` 注册后由 Rust 主动调用 Kotlin，
//! 路径更短，也便于在桌面端直接空实现。

use tauri::plugin::{Builder, PluginHandle};
use tauri::Runtime;

#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub struct FgsHandle<R: Runtime>(pub PluginHandle<R>);

pub fn init<R: Runtime>() -> tauri::plugin::TauriPlugin<R> {
    Builder::new("fgs")
        .setup(|app, api| {
            #[cfg(target_os = "android")]
            {
                use tauri::Manager;
                let handle = api.register_android_plugin("com.aibt.sonotheca", "FgsPlugin")?;
                app.manage(FgsHandle(handle));
            }
            #[cfg(not(target_os = "android"))]
            {
                let _ = (app, api);
            }
            Ok(())
        })
        .build()
}

#[cfg(target_os = "android")]
#[tauri::command]
pub fn set_fgs_count(app: tauri::AppHandle, count: u32) -> Result<(), String> {
    use serde_json::json;
    use tauri::Manager;

    let handle = app.state::<FgsHandle<tauri::Wry>>();
    if count > 0 {
        handle
            .0
            .run_mobile_plugin("start", json!({ "count": count }))
            .map_err(|err| err.to_string())
    } else {
        handle
            .0
            .run_mobile_plugin("stop", json!({}))
            .map_err(|err| err.to_string())
    }
}

/// 桌面端空实现：同一套前端代码两端都能跑
#[cfg(not(target_os = "android"))]
#[tauri::command]
pub fn set_fgs_count(_count: u32) -> Result<(), String> {
    Ok(())
}
