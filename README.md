<p align="center">
  <img src="docs/icon.png" alt="Sonotheca" width="112" />
</p>

<h1 align="center">Sonotheca</h1>

<p align="center">一个面向个人使用的音乐下载、归档与本地播放工具</p>

Sonotheca 把多个音乐来源放在同一个界面里搜索，支持把歌曲下载到本地、整理文件、补全标签、读取内嵌歌词，并用一个尽量简单的播放器试听。

项目最初只是我自己的学习项目，后来慢慢补齐了 Android 端、文件管理和本地音乐库。它不提供在线曲库，也不绕过任何平台的付费内容；所有下载都来自用户自己有权访问的公开链接或已登录账号。

> **项目性质**
>
> 我是一个学生，这个项目是利用课余时间一点一点做出来的。受学业和精力限制，我无法保证固定频率的更新，也不能保证每个平台接口长期可用。项目完全免费，仅供个人学习、研究和自己整理音乐使用。

---

## 功能

### 多来源搜索与下载

- 网易云音乐、QQ 音乐、B 站、Navidrome 四个来源
- 关键词搜索、直接粘贴歌曲链接、歌单/收藏夹链接
- 网易云歌单、B 站收藏夹分页浏览
- 下载任务队列：开始、暂停、继续、取消、删除、重新下载
- 可设置同时下载数量（1–5）和全局限速（不限速 / 1 / 3 / 5 MB/s）
- 下载完成后按规则归档，支持封面写入和标签写入

### 本地音乐库

- 扫描本地目录并登记音频文件
- 分页加载，支持上千首歌曲
- 按歌名、歌手、专辑、路径搜索
- 列表显示内嵌封面缩略图
- 长按多选、批量补全、批量删除
- 详情页查看格式、码率、时长、大小和路径

### 元数据与歌词

- 自动补全元数据时按登录状态选择来源：
  1. 已登录网易云 → 网易云
  2. 已登录 QQ 音乐 → QQ 音乐
  3. 已登录 B 站 → B 站
  4. 都不可用或匹配失败 → iTunes Search API
- 手动编辑标题、歌手、专辑、封面
- 写入 MP3 的 `USLT`、FLAC 的 `LYRICS` 内嵌歌词
- 读取同目录同名 `.lrc` 或文件内嵌歌词
- 支持标准 LRC、前缀多时间戳、内联逐字 LRC
- 全屏播放器按时间逐字高亮

### 播放器

- 本地文件与在线试听共用同一个播放器
- 迷你播放条和全屏播放页
- 封面、标题、歌手、专辑信息
- 歌词滚动与逐字高亮
- 上一首 / 下一首 / 拖动进度
- Android Release 下通过本机回环代理播放，避免 WebView 对本地文件的限制

### 播放列表与文件管理

- 首页「我的歌单」，按网易云 / QQ 音乐 / B 站切换
- 最近打开的歌单
- 双窗格文件管理器
- 焦点驱动：当前窗格选中文件，按钮自动指向另一个窗格
- 复制、移动、重命名、删除、回收站恢复
- 详情、封面、位深、采样率、码率、修改时间

### 外观与图标

- 浅色 / 深色 / 跟随系统
- Material Design 3 大圆角卡片
- 动态取色、HEX 输入、预设色
- 壁纸与液态玻璃（毛玻璃）效果
- 全局动效开关
- 自有薄荷水母图标：`ICON.webp`，并生成 Windows / Android / iOS 全套尺寸
- Android 自适应图标、启动页 Logo 与深海蓝 / 浅灰双主题

---

## 技术栈

| 层 | 使用 |
|---|---|
| 桌面 / Android 外壳 | Tauri 2 |
| 后端 | Rust |
| 前端 | Vue 3 + TypeScript + Pinia |
| 样式 | Tailwind CSS v4 |
| 本地数据库 | SQLite（WAL 模式） |
| 音频标签 | lofty |
| 网络 | reqwest |
| 图标 | Tauri icon + 自有 `ICON.webp` |

Android 最低版本为 Android 8.0（API 26），目标版本 API 35。

---

## 下载与安装

当前版本：**1.0.19**

- APK：[`release/Sonotheca-1.0.19-release.apk`](release/Sonotheca-1.0.19-release.apk)
- 源码包：[`release/Sonotheca-1.0.19-source.zip`](release/Sonotheca-1.0.19-source.zip)
- 如果 GitHub Releases 已发布，也可以从 Releases 页面下载同一组文件。

安装说明：

1. 下载 APK；
2. 在手机上允许“安装未知来源应用”；
3. 安装后首次启动，按提示授予通知、存储/所有文件访问权限；
4. 如果要管理外部音乐目录，请在设置中把音乐库路径改到自己的文件夹。

> 包名：`com.aibt.sonotheca`。如果之前装过旧包名的测试版，新旧版本的数据目录不互通。

---

## 从源码构建

### 环境

- Node.js 20+
- pnpm 9
- Rust stable（MSVC 工具链）
- JDK 17
- Android SDK Platform 35、Build Tools 35、NDK 27

Windows 下的完整环境安装步骤见 [`docs/SETUP.md`](docs/SETUP.md)。

### 桌面端

```bash
pnpm install
pnpm tauri:dev
```

### Android

```bash
pnpm install
pnpm tauri android build --apk --target aarch64
```

产物默认在：

```text
src-tauri/gen/android/app/build/outputs/apk/universal/release/app-universal-release.apk
```

> 仓库中的 Android 工程不包含签名配置。自己构建正式包时，请使用自己的 keystore；Release 页里的 APK 使用开发测试签名，仅用于直接安装体验，不适合上架应用商店。

### 检查

```bash
pnpm typecheck
cd src-tauri
cargo check --all-targets
cargo test --lib
```

---

## 目录结构

```text
src/
  components/        页面组件与播放器
  views/             搜索、音乐库、任务、设置、歌单、文件管理
  stores/            Pinia 状态
  ipc/bindings.ts    前端调用 Rust 的命令封装
  styles/            MD3 主题、玻璃卡片、动效
  utils/             LRC 解析、格式化

src-tauri/
  src/
    ipc/             Tauri commands
    provider/        网易云、QQ、B站、Navidrome
    pipeline/        下载、标签、路径、限速
    playlist.rs      歌单分页
    scrape.rs        元数据搜索与写回
    proxy/           本机音频代理
    db/              SQLite 与迁移
  gen/android/       Android 原生工程与自定义插件
  icons/             全平台图标
  tauri.conf.json
```

---

## 图标

- 源文件：[`ICON.webp`](ICON.webp)
- 展示用 PNG：[`docs/icon.png`](docs/icon.png)
- 全平台图标：[`src-tauri/icons`](src-tauri/icons)
- Android 自适应图标与启动页资源在 `src-tauri/gen/android/app/src/main/res`

图标由本项目自行设计/生成，不使用任何第三方应用的图标素材。

---

## 隐私与权限

- 没有账号系统，没有自建服务器，没有统计埋点。
- 登录 Cookie 只保存在本机 SQLite，不会上传到我的服务器。
- 下载、扫描、写标签都在本机完成。
- 应用需要存储权限，是因为要读取和整理用户自己选择的音乐目录。
- 卸载应用会删除应用私有数据；外部音乐库中的文件不会被一起删除。
- 本项目不会收集、上传用户的听歌记录或文件内容。

---

## 已知限制

- 各平台接口随时可能变化，搜索、登录或下载失败时请先确认网络和平台状态。
- QQ 音乐的匿名接口只提供部分低音质免费曲目；VIP/版权曲需要登录，且平台策略可能随时调整。
- Android WebView 不能解码所有音频编码，极少数 APE、超高采样率 WAV 等文件可能无法直接播放。
- 歌词逐字效果依赖歌词文件本身包含逐字时间信息；普通 LRC 只能按行显示。
- 项目以个人使用为主，没有做多用户、云同步、账号体系。
- 更新频率不固定，遇到问题欢迎提 Issue，但我不保证及时修复。

---

## 参考与致谢

Sonotheca 是在很多开源项目和优秀产品的启发下完成的。这里列出的参考，主要是**产品思路、交互方式和架构取舍**，不是复制它们的代码。

### 1. 开源项目与数据接口思路

- **洛雪音乐（LX Music）**  
  参考了它“界面尽量简单、把多个音乐来源放在一起、某个来源失败时自动换一个”的做法。  
  <https://github.com/lyswhut/lx-music-desktop>

- **HotDownloader**  
  参考了多任务下载队列、多来源聚合搜索、从搜索到下载再到归档的完整流程。  
  <https://github.com/lerdb/HotDownloader>

- **Melodex**  
  参考了本地音乐库与在线来源结合、播放列表与播放体验的组织方式。  
  <https://github.com/Cliff-Lee/melodex>

- **LRCGET**  
  参考了本地音频标签读取、内嵌歌词读取、同名字幕文件匹配的交互设计。  
  <https://github.com/tranxuanthang/lrcget>

- **NeteaseCloudMusicApi**  
  参考了网易云音乐数据接口的调用方式与参数组织。  
  <https://github.com/Binaryify/NeteaseCloudMusicApi>

- **Tauri / Vue / Vite / Pinia / Tailwind CSS**  
  提供了跨平台外壳、前端组件、状态管理和样式基础。  
  <https://tauri.app> · <https://vuejs.org> · <https://pinia.vuejs.org> · <https://tailwindcss.com>

- **lofty / rusqlite / reqwest / qrcode / ncm-api-rs / ncmdump**  
  分别用于音频标签、SQLite、网络请求、二维码、NCM 容器处理。  
  <https://github.com/Serial-ATA/lofty-rs> · <https://github.com/rusqlite/rusqlite> · <https://github.com/seanmonstar/reqwest>

### 2. 产品交互与视觉参考

- **MT 管理器**  
  参考了双窗格文件浏览、当前窗格决定操作方向、批量整理文件的交互方式。只参考操作方式，界面和代码都是独立实现的。

- **Material Design 3**  
  参考了它的圆角卡片、色彩角色、明暗模式和动态取色规范。  
  <https://m3.material.io>

- **现代液态玻璃 / 毛玻璃界面趋势**  
  参考了半透明卡片、背景模糊、细高光边框的视觉方向，具体样式由本项目自行实现。

### 3. 关于代码原创

本项目的核心业务逻辑、界面组件和底层处理代码，是在 AI 辅助下从零编写和反复调试完成的。开发过程中没有对任何闭源商业软件进行反编译，也没有复制其代码。上面列出的项目和产品，仅作为功能思路、交互方式和视觉方向的参考。

---

## 免责声明

- 本项目完全免费，禁止任何形式的倒卖、付费代下载或商业分发。
- 本项目不提供音乐内容，不存储任何在线曲库，所有下载行为由用户自行发起。
- 请只下载你有权访问、保存和使用的内容，并遵守所在地区的法律法规以及各平台的服务条款。
- 因使用本项目产生的任何版权、账号或数据问题，由使用者自行承担。
- 项目按“现状”提供，不承诺可用性、稳定性或长期维护。

---

## 许可

代码以 MIT License 发布，详见 [`LICENSE`](LICENSE)。

第三方项目、接口、图标与设计语言分别遵循其各自的许可；本仓库不重新分发它们的代码或资源。

---

## 版本

- **1.0.19** — 当前发布版
  - 修复 Android Release 下本地播放与在线试听
  - 修复内联逐字 LRC 解析与逐字高亮
  - 标签写入增加临时副本、回退覆盖与落盘校验
  - 音乐库封面缩略图、分页加载、详情编辑
  - 动态元数据来源与 iTunes 兜底
