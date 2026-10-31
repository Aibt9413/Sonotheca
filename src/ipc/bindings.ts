import { invoke } from "@tauri-apps/api/core";

/**
 * M1：手写薄封装。M2 引入 tauri-specta 后整体替换为自动生成绑定。
 * 类型必须与 Rust 侧 serde(rename_all = "camelCase") 严格对齐。
 */

export type TaskState =
  | "queued"
  | "preflight"
  | "resolving"
  | "downloading"
  | "processing"
  | "verifying"
  | "completed"
  | "failed"
  | "canceled";

export interface AppErrorPayload {
  kind: string;
  message: string;
}

export interface TaskError {
  kind: string;
  message: string;
  httpStatus: number | null;
}

export interface TaskSnapshot {
  rootId: number;
  dirTemplate: string;
  fileTemplate: string;
  relativeDir: string;
  collision: "skip" | "rename" | "overwrite";
  quality: string;
  coverEmbed: boolean;
  playlistId: number | null;
}

export interface DownloadTask {
  id: string;
  source: string;
  sourceTrackId: string;
  title: string;
  artist: string;
  state: TaskState;
  snapshot: TaskSnapshot;
  bytesDone: number;
  bytesTotal: number | null;
  attempts: number;
  error: TaskError | null;
  tempPath: string | null;
  finalPath: string | null;
  createdAt: number;
  updatedAt: number;
}

export interface Track {
  id: number;
  title: string;
  artist: string;
  album: string;
  durationMs: number | null;
  bitrate: number | null;
  sizeBytes: number | null;
  format: string | null;
  relPath: string;
  pathStatus: "ok" | "missing" | "moved";
  sources: unknown[];
}

export interface AppInfo {
  version: string;
  platform: string;
  dbTracks: number;
  schemaVersion: number;
}

/** download://progress */
export interface ProgressEvent {
  id: string;
  bytesDone: number;
  bytesTotal: number | null;
  state: TaskState;
  speedBps: number;
}

/** download://state */
export interface StateEvent {
  task: DownloadTask;
}

export const TERMINAL_STATES: readonly TaskState[] = ["completed", "failed", "canceled"];

export const isTerminal = (state: TaskState): boolean => TERMINAL_STATES.includes(state);

export const ping = (name: string): Promise<string> => invoke<string>("ping", { name });

export const appInfo = (): Promise<AppInfo> => invoke<AppInfo>("app_info");

export const addDirectDownloadTask = (
  url: string,
  title?: string,
  artist?: string,
): Promise<DownloadTask> =>
  invoke<DownloadTask>("add_direct_download_task", {
    url,
    title: title ?? null,
    artist: artist ?? null,
  });

export const listDownloadTasks = (limit = 50): Promise<DownloadTask[]> =>
  invoke<DownloadTask[]>("list_download_tasks", { limit });

export const cancelDownloadTask = (id: string): Promise<boolean> =>
  invoke<boolean>("cancel_download_task", { id });

/** 删除一条已完成/失败/取消的任务记录；不删除 .part 与已归档音频 */
export const deleteDownloadTask = (id: string): Promise<void> =>
  invoke<void>("delete_download_task", { id });

/** 继续一个已失败的任务：复用同一 id，因此复用同一个 .part 做 Range 续传 */
export const resumeDownloadTask = (id: string): Promise<DownloadTask> =>
  invoke<DownloadTask>("resume_download_task", { id });

/** 重新下载终态任务：重新解析直链、清空进度，完成时覆盖旧归档文件 */
export const restartDownloadTask = (id: string): Promise<DownloadTask> =>
  invoke<DownloadTask>("restart_download_task", { id });

// ───────────────────────────── M9：下载设置 ─────────────────────────────

export interface DownloadSettings {
  /** 1–5 */
  concurrency: number;
  /** bytes/sec，0 = 不限速 */
  speedLimit: number;
  /** 50 / 100 / 200 / 0（无限制） */
  playlistPageSize: number;
}

export const getDownloadSettings = (): Promise<DownloadSettings> =>
  invoke<DownloadSettings>("download_settings");

/** 只传要改的字段；Rust 侧 clamp 后返回最终值 */
export const setDownloadSettings = (params: {
  concurrency?: number;
  speedLimit?: number;
  playlistPageSize?: number;
}): Promise<DownloadSettings> =>
  invoke<DownloadSettings>("set_download_settings", {
    concurrency: params.concurrency ?? null,
    speedLimit: params.speedLimit ?? null,
    playlistPageSize: params.playlistPageSize ?? null,
  });

/**
 * 同步前台服务状态（仅 Android 生效，桌面端是空实现）。
 * count = 0 时关闭 FGS；> 0 时拉起并更新聚合通知。
 */
export const setFgsCount = (count: number): Promise<void> =>
  invoke<void>("set_fgs_count", { count });

// ───────────────────────────── M4：网易云 ─────────────────────────────

export interface SearchHit {
  source: string;
  id: string;
  title: string;
  artist: string;
  album: string;
  coverUrl: string | null;
  durationMs: number | null;
}

export const searchNetease = (keyword: string, offset = 0, limit = 30): Promise<SearchHit[]> =>
  invoke<SearchHit[]>("search_netease", { keyword, offset, limit });

// ── 多源聚合 ─────────────────────────────────────────────

export type SourceId = "netease" | "qq" | "bilibili" | "navidrome" | "direct";

export interface SourceFailure {
  source: string;
  message: string;
}

export interface SearchAggregate {
  hits: SearchHit[];
  errors: SourceFailure[];
}

/** 并发搜索；sources 传 ["all"] 或具体源名 */
export const searchSources = (keyword: string, sources: string[]): Promise<SearchAggregate> =>
  invoke<SearchAggregate>("search_sources", { keyword, sources });

// ───────────────────────────── M8：歌单 ─────────────────────────────

export interface PlaylistPreview {
  source: SourceId;
  id: string;
  title: string;
  coverUrl: string | null;
  total: number;
  description: string | null;
}

export interface PlaylistTrack {
  source: SourceId;
  id: string;
  title: string;
  artist: string;
  album: string;
  coverUrl: string | null;
  durationMs: number | null;
  index: number;
}

export interface PlaylistPage {
  offset: number;
  limit: number;
  total: number;
  tracks: PlaylistTrack[];
  hasMore: boolean;
}

/** 只拉歌单元数据（歌单名 / 封面 / 总数），进页前调用 */
export const playlistPreview = (url: string): Promise<PlaylistPreview> =>
  invoke<PlaylistPreview>("playlist_preview", { url });

/** 分页拉曲目：前端固定 50 首/页，禁止全量 */
export const playlistPage = (
  source: string,
  id: string,
  offset: number,
  limit = 50,
): Promise<PlaylistPage> =>
  invoke<PlaylistPage>("playlist_page", { source, id, offset, limit });

export interface RecentPlaylist {
  source: SourceId;
  id: string;
  name: string;
  coverUrl: string | null;
  total: number;
}

/** 最近预检过的歌单，按更新时间倒序 */
export const recentPlaylists = (limit = 8): Promise<RecentPlaylist[]> =>
  invoke<RecentPlaylist[]>("recent_playlists", { limit });

export interface MyPlaylistItem {
  source: SourceId;
  id: string;
  name: string;
  coverUrl: string | null;
  total: number;
}

/** 当前登录账号的歌单；网易云走 UID，B站走 nav MID */
export const myPlaylists = (source: SourceId): Promise<MyPlaylistItem[]> =>
  invoke<MyPlaylistItem[]>("my_playlists", { source });

/** 按来源路由下载（netease / bilibili / navidrome / qq；QQ 匿名仅部分免费曲目，VIP 需登录） */
export const addSourceDownloadTask = (
  source: string,
  trackId: string,
  title: string | undefined,
  artist: string | undefined,
  quality: NeteaseQuality,
): Promise<NeteaseEnqueue> =>
  invoke<NeteaseEnqueue>("add_source_download_task", {
    source,
    trackId,
    title: title ?? null,
    artist: artist ?? null,
    quality,
  });

export interface NavidromeStatus {
  configured: boolean;
  url: string;
  user: string;
}

export const navidromeStatus = (): Promise<NavidromeStatus> =>
  invoke<NavidromeStatus>("navidrome_status");

/** 登录 Navidrome：ping 通过才落库；成功后密码不再回传 */
export const navidromeLogin = (
  url: string,
  user: string,
  password: string,
): Promise<NavidromeStatus> =>
  invoke<NavidromeStatus>("navidrome_login", { url, user, password });

export const setNavidromeSettings = (
  url: string,
  user: string,
  password: string,
): Promise<NavidromeStatus> =>
  invoke<NavidromeStatus>("set_navidrome_settings", { url, user, password });

/** 降级原因：实际档位低于请求档位时由 Rust 返回 */
export type QualityFallback = "anonymous" | "lossless" | "exhigh" | "standard";

/** 网易云入队结果：quality 是实际生效档位 */
export interface NeteaseEnqueue {
  task: DownloadTask;
  quality: ConcreteQuality;
  fallback: QualityFallback | null;
}

/** 试听结果：本地代理地址（127.0.0.1 随机端口） */
export interface NeteaseStream {
  url: string;
  quality: ConcreteQuality;
  fallback: QualityFallback | null;
}

/** 试听：Rust 解析直链并注册本地票据，返回给它 `<audio>` 用的地址 */
export const neteaseStreamUrl = (songId: string): Promise<NeteaseStream> =>
  invoke<NeteaseStream>("netease_stream_url", { songId });

/** 歌词：网易云传 trackId；本地文件传 localPath（相对库根或绝对路径） */
export const getLyrics = (trackId: string, localPath?: string | null): Promise<string> =>
  invoke<string>("get_lyrics", { trackId, localPath: localPath ?? null });

/** 先解析出带签名的 CDN 直链再入队，走同一条多线程下载流水线 */
export const addNeteaseDownloadTask = (
  songId: string,
  title: string | undefined,
  artist: string | undefined,
  quality: NeteaseQuality,
): Promise<NeteaseEnqueue> =>
  invoke<NeteaseEnqueue>("add_netease_download_task", {
    songId,
    title: title ?? null,
    artist: artist ?? null,
    quality,
  });

/** 下载音质档位；`default` 只在弹窗里出现，表示"跟随设置" */
export type NeteaseQuality =
  | "default"
  | "standard"
  | "exhigh"
  | "lossless"
  | "hires"
  | "jymaster"
  | "sky";

/** 弹窗里可持久化的具体档位（不含 default） */
export type ConcreteQuality = Exclude<NeteaseQuality, "default">;

/** 默认下载音质（settings 表 key: netease_default_quality） */
export const neteaseDefaultQuality = (): Promise<ConcreteQuality> =>
  invoke<ConcreteQuality>("netease_default_quality");

export const setNeteaseDefaultQuality = (quality: ConcreteQuality): Promise<ConcreteQuality> =>
  invoke<ConcreteQuality>("set_netease_default_quality", { quality });

export interface NeteaseProfile {
  userId: number;
  nickname: string;
  avatarUrl: string;
}

export interface NeteaseStatus {
  loggedIn: boolean;
  profile: NeteaseProfile | null;
}

export interface QrState {
  /** 800 过期 / 801 待扫码 / 802 已扫码待确认 / 803 成功 */
  code: number;
  message: string;
}

export const neteaseQrKey = (): Promise<string> => invoke<string>("netease_qr_key");

export const neteaseQrCheck = (key: string): Promise<QrState> =>
  invoke<QrState>("netease_qr_check", { key });

export const neteaseStatus = (): Promise<NeteaseStatus> => invoke<NeteaseStatus>("netease_status");

export const neteaseLogout = (): Promise<void> => invoke<void>("netease_logout");

// ───────────────────────────── M8：B 站扫码登录 ─────────────────────────────

export interface QrLoginStart {
  key: string;
  /** 二维码内容，前端用 qrcode 库转 data URL */
  url: string;
  expiresIn: number;
}

export interface QrLoginState {
  /** B 站：0 成功 / 86101 待扫码 / 86090 已扫码待确认 / 86038 已过期 */
  code: number;
  message: string;
}

export interface BilibiliProfile {
  mid: number;
  nickname: string;
  avatarUrl: string;
}

export interface BilibiliStatus {
  loggedIn: boolean;
  profile: BilibiliProfile | null;
}

export const bilibiliQrStart = (): Promise<QrLoginStart> =>
  invoke<QrLoginStart>("bilibili_qr_start");

export const bilibiliQrPoll = (key: string): Promise<QrLoginState> =>
  invoke<QrLoginState>("bilibili_qr_poll", { key });

export const bilibiliStatus = (): Promise<BilibiliStatus> =>
  invoke<BilibiliStatus>("bilibili_status");

export const bilibiliLogout = (): Promise<void> => invoke<void>("bilibili_logout");

export interface BiliPage {
  cid: string;
  page: number;
  title: string;
  durationMs: number | null;
}

/** 多 P 选集：单 P 时数组长度 1 */
export const bilibiliPages = (bvid: string): Promise<BiliPage[]> =>
  invoke<BiliPage[]>("bilibili_pages", { bvid });

// ───────────────────────────── Web 登录兜底 ─────────────────────────────

export interface WebLoginState {
  loggedIn: boolean;
  message: string;
}

export type WebLoginSource = "netease" | "bilibili" | "qq";

/** 打开官方登录页的独立 WebView */
export const openWebLogin = (source: WebLoginSource): Promise<void> =>
  invoke<void>("open_web_login", { source });

/** 从 WebView CookieManager 抓核心 Cookie；需轮询到 loggedIn=true */
export const captureWebLogin = (source: WebLoginSource): Promise<WebLoginState> =>
  invoke<WebLoginState>("capture_web_login", { source });

export const closeWebLogin = (source: WebLoginSource): Promise<void> =>
  invoke<void>("close_web_login", { source });

export interface QqLoginStatus {
  loggedIn: boolean;
}

export const qqStatus = (): Promise<QqLoginStatus> => invoke<QqLoginStatus>("qq_status");

// ───────────────────────────── M5：文件管理与刮削 ─────────────────────────────

export interface DirEntry {
  name: string;
  relPath: string;
  isDir: boolean;
  size: number | null;
  trackId: number | null;
}

export interface DirListing {
  relPath: string;
  parent: string | null;
  dirs: DirEntry[];
  files: DirEntry[];
}

export interface ScanProgress {
  scanned: number;
  inserted: number;
  current: string;
  done: boolean;
}

export interface TrashEntry {
  id: number;
  name: string;
  originalRelPath: string;
  sizeBytes: number | null;
  deletedAt: number;
}

export interface MetaView {
  title: string;
  artist: string;
  album: string;
  coverUrl: string | null;
}

export interface ScrapeCandidate extends MetaView {
  source: string;
  id: string;
  durationMs: number | null;
  score: number;
}

export interface ScrapePreview {
  current: MetaView;
  candidates: ScrapeCandidate[];
}

export type StorageSpace = "app" | "global";

export interface StorageStatus {
  granted: boolean;
}

export const storageStatus = (): Promise<StorageStatus> => invoke<StorageStatus>("storage_status");

/** 跳系统"所有文件访问权限"页；回来后前端需重新查一次 status */
export const storageRequest = (): Promise<void> => invoke<void>("storage_request");

export const listLibraryDir = (space: StorageSpace = "app", relPath = ""): Promise<DirListing> =>
  invoke<DirListing>("list_library_dir", { space, relPath });

/** 应用私有库根目录的绝对路径 */
export const libraryRootPath = (): Promise<string> => invoke<string>("library_root_path");

/** 本地播放绝对路径，交给 convertFileSrc 生成 asset URL */
export const trackPlayPath = (trackId: number): Promise<string> =>
  invoke<string>("track_play_path", { trackId });

/** 本地播放 127.0.0.1 代理 URL（Android Release 更可靠） */
export const trackPlayUrl = (trackId: number): Promise<string> =>
  invoke<string>("track_play_url", { trackId });

export interface LocalPlayInfo {
  url: string;
  title: string;
  artist: string;
  album: string;
  coverUrl: string | null;
  lyrics: string;
  durationMs: number | null;
}

/** 本地播放完整载荷：URL + 标签 + 内嵌封面 + LRC */
export const trackPlayInfo = (trackId: number): Promise<LocalPlayInfo> =>
  invoke<LocalPlayInfo>("track_play_info", { trackId });

export const getTrack = (trackId: number): Promise<Track> =>
  invoke<Track>("get_track", { trackId });

/** 修改音乐库根目录（绝对路径，Rust 侧会校验可写）；返回规范化后的路径 */
export const setLibraryRootPath = (path: string): Promise<string> =>
  invoke<string>("set_library_root_path", { path });

export const transferEntries = (params: {
  fromSpace: StorageSpace;
  relPaths: string[];
  toSpace: StorageSpace;
  targetDir: string;
  mode: "move" | "copy";
}): Promise<number> =>
  invoke<number>("transfer_entries", {
    fromSpace: params.fromSpace,
    relPaths: params.relPaths,
    toSpace: params.toSpace,
    targetDir: params.targetDir,
    mode: params.mode,
  });

export const scanLibraryDir = (relPath = ""): Promise<void> =>
  invoke<void>("scan_library_dir", { relPath });

export const trashTrack = (id: number): Promise<void> => invoke<void>("trash_track", { id });

export const listTrash = (): Promise<TrashEntry[]> => invoke<TrashEntry[]>("list_trash");

export const restoreTrash = (id: number): Promise<void> => invoke<void>("restore_trash", { id });

export const purgeTrash = (id?: number): Promise<number> =>
  invoke<number>("purge_trash", { id: id ?? null });

export const scrapePreview = (trackId: number): Promise<ScrapePreview> =>
  invoke<ScrapePreview>("scrape_preview", { trackId });

export const scrapeApply = (trackId: number, candidate: ScrapeCandidate): Promise<Track> =>
  invoke<Track>("scrape_apply", { trackId, candidate });

/** 手动编辑标签；coverDataUrl/lyrics 传 null 表示保留原值 */
export const updateTrackMetadata = (
  trackId: number,
  absolutePath: string | null,
  title: string,
  artist: string,
  album: string,
  coverDataUrl: string | null = null,
  lyrics: string | null = null,
): Promise<Track> =>
  invoke<Track>("update_track_metadata", {
    trackId,
    absolutePath,
    title,
    artist,
    album,
    coverDataUrl,
    lyrics,
  });

export const readTrackLyrics = (trackId: number): Promise<string> =>
  invoke<string>("read_track_lyrics", { trackId });

/** 调网易云搜索 + 歌词接口，返回 LRC */
export const syncTrackLyrics = (trackId: number): Promise<string> =>
  invoke<string>("sync_track_lyrics", { trackId });

export interface MetadataProgress {
  current: number;
  total: number;
}

export interface BatchScrapeResult {
  applied: number;
  skipped: number;
  failed: number;
}

/** 批量补全：Rust 顺序执行并逐首发 metadata_progress */
export const batchScrapeMetadata = (trackIds: number[]): Promise<BatchScrapeResult> =>
  invoke<BatchScrapeResult>("batch_scrape_metadata", { trackIds });

/** 重命名磁盘文件（自动保留扩展名），并同步库内记录 */
export const renameTrack = (id: number, newName: string): Promise<Track> =>
  invoke<Track>("rename_track", { id, newName });

export interface FileDetails {
  name: string;
  path: string;
  size: number;
  format: string;
  bitrate: number | null;
  durationMs: number | null;
  bitDepth: number | null;
  sampleRate: number | null;
  modified: number | null;
  trackId: number | null;
  cover: string | null;
}

/** 读单个文件的详情（含内嵌封面 data URL） */
export const fileDetails = (space: StorageSpace, relPath: string): Promise<FileDetails> =>
  invoke<FileDetails>("file_details", { space, relPath });

/** 删除：fs::rename 到 .temp/trash */
export const trashPaths = (space: StorageSpace, relPaths: string[]): Promise<number> =>
  invoke<number>("trash_paths", { space, relPaths });

/** 按路径重命名（文件可能不在库内） */
export const renamePath = (
  space: StorageSpace,
  relPath: string,
  newName: string,
): Promise<string> => invoke<string>("rename_path", { space, relPath, newName });

export interface TrackPage {
  tracks: Track[];
  total: number;
}

/** 音乐库分页：默认 50 首/页，offset = 已加载数量 */
export const listTracks = (limit = 50, offset = 0): Promise<TrackPage> =>
  invoke<TrackPage>("list_tracks", { limit, offset });

/** 浏览器里跑 `pnpm dev`（无 Tauri 运行时）时用于降级 */
export const isTauri = (): boolean => "__TAURI_INTERNALS__" in window;

export function describeError(error: unknown): string {
  if (typeof error === "string") return error;
  if (error && typeof error === "object" && "message" in error) {
    return String((error as AppErrorPayload).message);
  }
  return String(error);
}
