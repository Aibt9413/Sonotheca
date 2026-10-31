//! 下载流水线。
//!
//! 阶段顺序（也是 TaskState 的顺序）：
//!   Queued → Downloading → Processing → Completed
//!                        ↘ (失败) Failed / Canceled

pub mod commit;
pub mod download;
pub mod magic;
pub mod meta;
pub mod ncm;
pub mod path;
pub mod runner;
pub mod throttle;
