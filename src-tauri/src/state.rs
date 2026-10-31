//! 全局状态。
//!
//! - `db`：命令层同步用（短临界区，绝不跨 await）
//! - `db_path`：流水线用，按需开短连接
//! - `tasks`：取消标志 + 并发闸门

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use rusqlite::Connection;
use tokio::sync::Semaphore;
use uuid::Uuid;

use crate::pipeline::throttle::TokenBucket;

pub const CONCURRENCY_KEY: &str = "download_concurrency";
pub const SPEED_LIMIT_KEY: &str = "download_speed_limit";
pub const PLAYLIST_PAGE_SIZE_KEY: &str = "playlist_page_size";

pub struct TaskManager {
    cancels: Mutex<HashMap<Uuid, Arc<AtomicBool>>>,
    /// 运行中可替换的 Semaphore：改设置后新任务用新并发，旧任务自然结束
    permits: RwLock<Arc<Semaphore>>,
}

impl TaskManager {
    pub fn new(concurrency: usize) -> Self {
        Self {
            cancels: Mutex::new(HashMap::new()),
            permits: RwLock::new(Arc::new(Semaphore::new(concurrency.clamp(1, 5)))),
        }
    }

    /// 返回 `None` 表示同一个 id 已经在跑 —— 这是"同一任务不被两个 runner 同时写"
    /// 的唯一闸门。并发写同一个 `.part` 会产生交错字节，比下载失败更糟。
    pub fn register(&self, id: Uuid) -> Option<Arc<AtomicBool>> {
        let mut map = self.cancels.lock().ok()?;
        if map.contains_key(&id) {
            return None;
        }
        let flag = Arc::new(AtomicBool::new(false));
        map.insert(id, flag.clone());
        Some(flag)
    }

    pub fn finish(&self, id: Uuid) {
        if let Ok(mut map) = self.cancels.lock() {
            map.remove(&id);
        }
    }

    /// 返回 false 表示任务已结束或不存在
    pub fn cancel(&self, id: Uuid) -> bool {
        match self.cancels.lock() {
            Ok(map) => match map.get(&id) {
                Some(flag) => {
                    flag.store(true, Ordering::Relaxed);
                    true
                }
                None => false,
            },
            Err(_) => false,
        }
    }

    pub fn permits(&self) -> Arc<Semaphore> {
        self.permits
            .read()
            .map(|guard| guard.clone())
            .unwrap_or_else(|_| Arc::new(Semaphore::new(1)))
    }

    pub fn set_concurrency(&self, concurrency: usize) {
        if let Ok(mut guard) = self.permits.write() {
            *guard = Arc::new(Semaphore::new(concurrency.clamp(1, 5)));
        }
    }
}

pub struct AppState {
    pub db_path: PathBuf,
    pub db: Mutex<Connection>,
    pub tasks: Arc<TaskManager>,
    /// 全局 HTTP 客户端：连接池跨任务复用（pool_max_idle_per_host=10）
    pub client: Arc<reqwest::Client>,
    /// 网易云适配器（匿名态）。M5 接入 Cookie 后在这里换实例
    pub netease: Arc<crate::provider::netease::NeteaseAdapter>,
    /// B 站扫码登录态（M8）
    pub bilibili: Arc<crate::provider::auth::BilibiliAuth>,
    /// 本地试听代理（127.0.0.1 随机端口）
    pub proxy: crate::proxy::ProxyHandle,
    /// 全局下载限速（所有任务共享）
    pub throttle: Arc<TokenBucket>,
}
