//! 前端事件契约。事件名是不变量，载荷随阶段演进只加字段不删字段。

use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::model::task::{DownloadTask, TaskState};

pub const PROGRESS: &str = "download://progress";
pub const STATE: &str = "download://state";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProgressPayload {
    pub id: String,
    pub bytes_done: u64,
    pub bytes_total: Option<u64>,
    pub state: TaskState,
    pub speed_bps: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatePayload {
    pub task: DownloadTask,
}

pub fn emit_progress(app: &AppHandle, payload: ProgressPayload) {
    let _ = app.emit(PROGRESS, payload);
}

pub fn emit_state(app: &AppHandle, task: &DownloadTask) {
    let _ = app.emit(STATE, StatePayload { task: task.clone() });
}
