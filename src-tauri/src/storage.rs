//! Android 全局存储权限（MANAGE_EXTERNAL_STORAGE）桥接。
//!
//! 与 `fgs.rs` 同一套模式：Rust 持 `PluginHandle`，前端调 Rust 命令，
//! Rust 再 `run_mobile_plugin` 到 Kotlin。
//!
//! 注意：这是 Android 的**特殊权限**，不能弹运行时对话框，
//! 只能跳系统设置页让用户手动打开"所有文件访问权限"。

use serde::{Deserialize, Serialize};
use tauri::plugin::{Builder, PluginHandle};
use tauri::Runtime;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageStatus {
    pub granted: bool,
}

#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub struct StorageHandle<R: Runtime>(pub PluginHandle<R>);

pub fn init<R: Runtime>() -> tauri::plugin::TauriPlugin<R> {
    Builder::new("storage")
        .setup(|app, api| {
            #[cfg(target_os = "android")]
            {
                use tauri::Manager;
                let handle = api.register_android_plugin("com.aibt.sonotheca", "StoragePlugin")?;
                app.manage(StorageHandle(handle));
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
pub fn storage_status(app: tauri::AppHandle) -> Result<StorageStatus, String> {
    use tauri::Manager;

    let handle = app.state::<StorageHandle<tauri::Wry>>();
    handle
        .0
        .run_mobile_plugin("status", serde_json::json!({}))
        .map_err(|err| err.to_string())
}

/// 跳系统设置页；用户回来后前端重新查 status
#[cfg(target_os = "android")]
#[tauri::command]
pub fn storage_request(app: tauri::AppHandle) -> Result<(), String> {
    use tauri::Manager;

    let handle = app.state::<StorageHandle<tauri::Wry>>();
    handle
        .0
        .run_mobile_plugin("request", serde_json::json!({}))
        .map_err(|err| err.to_string())
}

#[cfg(not(target_os = "android"))]
#[tauri::command]
pub fn storage_status() -> Result<StorageStatus, String> {
    // 桌面端没有这个权限模型；全局空间在 UI 上会被隐藏
    Ok(StorageStatus { granted: false })
}

#[cfg(not(target_os = "android"))]
#[tauri::command]
pub fn storage_request() -> Result<(), String> {
    Ok(())
}
