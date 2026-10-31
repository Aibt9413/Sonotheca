//! Sonotheca 后端入口。
//!
//! M0：SQLite + WAL + IPC 冒烟。
//! M1：直链下载 → .part → 标签 → 分类器 → 原子 rename → 入库。

mod db;
mod error;
mod events;
mod fgs;
mod ipc;
mod library;
mod model;
mod paths;
mod pipeline;
mod playlist;
mod provider;
mod proxy;
mod scrape;
mod state;
mod storage;
mod util;

use std::sync::{Arc, Mutex};

use tauri::Manager;

/// 全局唯一的 HTTP 客户端。
///
/// - 连接池跨任务共享，`pool_max_idle_per_host(10)` 让分块并发不会反复握手
/// - 浏览器 UA 规避源站对非浏览器客户端的限速/拦截
/// - 总超时给到 1 小时，覆盖大文件
fn build_http_client() -> Result<reqwest::Client, Box<dyn std::error::Error>> {
    Ok(reqwest::Client::builder()
        .user_agent(pipeline::download::BROWSER_UA)
        .connect_timeout(std::time::Duration::from_secs(15))
        .timeout(std::time::Duration::from_secs(3600))
        .pool_max_idle_per_host(10)
        .redirect(reqwest::redirect::Policy::limited(8))
        .build()?)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(fgs::init())
        .plugin(storage::init())
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;

            let db_path = data_dir.join("sonotheca.db");
            let connection = db::open(&db_path)?;

            // 首次启动就把默认归档根与 .temp 落到磁盘（失败不阻塞启动，加任务时会重试）
            let _ = paths::library_root(app.handle(), &connection);

            // 上次被强杀时，任务会永远停在 downloading；这里统一收口成 failed，
            // 并保留 .part，用户点「继续」即可断点续传。
            let recovered = db::repo::mark_interrupted(&connection, util::now_ms()).unwrap_or(0);
            if recovered > 0 {
                eprintln!("[sonotheca] 恢复了 {recovered} 个上次中断的任务（.part 已保留）");
            }

            // 上次的网易云 Cookie（明文存 settings 表；M5 再上系统密钥库）
            let cookie = db::repo::get_setting(&connection, "netease_cookie")
                .ok()
                .flatten();
            // B 站扫码登录态
            let bilibili_cookie = db::repo::get_setting(
                &connection,
                provider::auth::COOKIE_KEY,
            )
            .ok()
            .flatten();

            // 本地试听代理：绑定 127.0.0.1:0，随机端口
            let proxy = proxy::spawn().map_err(|err| {
                std::io::Error::other(format!("本地试听代理启动失败: {err}"))
            })?;

            let client = Arc::new(build_http_client()?);

            // M9：下载设置（并发 1–5 / 全局限速）
            let download_concurrency = db::repo::get_setting(
                &connection,
                state::CONCURRENCY_KEY,
            )
            .ok()
            .flatten()
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(3)
            .clamp(1, 5);
            let download_speed_limit = db::repo::get_setting(
                &connection,
                state::SPEED_LIMIT_KEY,
            )
            .ok()
            .flatten()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(0);

            // QQ 网页登录 Cookie（若存在，所有 QqAdapter 请求都会带上）
            if let Some(cookie) = db::repo::get_setting(&connection, "qq_cookie")
                .ok()
                .flatten()
            {
                provider::qq::set_login_cookie(Some(cookie));
            }

            app.manage(state::AppState {
                db_path,
                db: Mutex::new(connection),
                tasks: Arc::new(state::TaskManager::new(download_concurrency)),
                bilibili: Arc::new(provider::auth::BilibiliAuth::new(
                    client.as_ref().clone(),
                    bilibili_cookie,
                )),
                client,
                netease: Arc::new(provider::netease::NeteaseAdapter::new(cookie)),
                proxy,
                throttle: Arc::new(pipeline::throttle::TokenBucket::new(
                    download_speed_limit,
                )),
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            ipc::ping,
            ipc::app_info,
            ipc::add_direct_download_task,
            ipc::search_netease,
            ipc::search_sources,
            ipc::playlist_preview,
            ipc::playlist_page,
            ipc::recent_playlists,
            ipc::my_playlists,
            ipc::add_netease_download_task,
            ipc::add_source_download_task,
            ipc::navidrome_status,
            ipc::navidrome_login,
            ipc::set_navidrome_settings,
            ipc::netease_stream_url,
            ipc::get_lyrics,
            ipc::set_library_root_path,
            ipc::netease_qr_key,
            ipc::netease_qr_check,
            ipc::netease_status,
            ipc::netease_logout,
            ipc::bilibili_qr_start,
            ipc::bilibili_qr_poll,
            ipc::bilibili_status,
            ipc::bilibili_logout,
            ipc::bilibili_pages,
            ipc::open_web_login,
            ipc::capture_web_login,
            ipc::close_web_login,
            ipc::qq_status,
            ipc::netease_default_quality,
            ipc::set_netease_default_quality,
            ipc::list_library_dir,
            ipc::library_root_path,
            ipc::track_play_path,
            ipc::track_play_url,
            ipc::track_play_info,
            ipc::get_track,
            ipc::transfer_entries,
            ipc::scan_library_dir,
            ipc::trash_track,
            ipc::list_trash,
            ipc::restore_trash,
            ipc::purge_trash,
            ipc::scrape_preview,
            ipc::scrape_apply,
            ipc::update_track_metadata,
            ipc::read_track_lyrics,
            ipc::sync_track_lyrics,
            ipc::batch_scrape_metadata,
            ipc::rename_track,
            ipc::file_details,
            ipc::trash_paths,
            ipc::rename_path,
            storage::storage_status,
            storage::storage_request,
            ipc::list_download_tasks,
            ipc::cancel_download_task,
            ipc::delete_download_task,
            ipc::resume_download_task,
            ipc::restart_download_task,
            ipc::download_settings,
            ipc::set_download_settings,
            ipc::list_tracks,
            fgs::set_fgs_count,
        ])
        .run(tauri::generate_context!())
        .expect("Sonotheca 启动失败");
}
