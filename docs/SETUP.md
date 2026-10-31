# 环境安装与校验

目标：Windows（主）+ Android；macOS 仅桌面。

## Windows

### 1. Rust（用户级，无需管理员）

```powershell
winget install --id Rustlang.Rustup -e
# 新开一个终端，让 PATH 生效
rustup default stable-msvc
rustup component add rustfmt clippy
```

### 2. MSVC 生成工具 + Windows SDK

Tauri 在 Windows 上**必须**有 MSVC 链接器，否则报 `link.exe not found`：

```powershell
winget install --id Microsoft.VisualStudio.2022.BuildTools -e --override "--wait --quiet --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
```

装完校验（应能打印 `link.exe` 路径）：

```powershell
& "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe" -latest -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
```

### 3. Node + pnpm

```powershell
winget install --id OpenJS.NodeJS.LTS -e
corepack enable
corepack prepare pnpm@9.15.0 --activate
node -v; pnpm -v
```

### 4. JDK 17

Tauri 2 的 Android 构建用 Gradle 8，**不要用 JDK 21+**。

```powershell
winget install --id EclipseAdoptium.Temurin.17.JDK -e
java -version   # 必须出现 17.x
```

### 5. Android SDK + NDK

```powershell
winget install --id Google.AndroidStudio -e
# 装完后打开一次 Android Studio 完成 SDK 初始化，然后：
$sdk = "$env:LOCALAPPDATA\Android\Sdk"
& "$sdk\cmdline-tools\latest\bin\sdkmanager.bat" `
  "platform-tools" "platforms;android-35" "build-tools;35.0.0" "ndk;27.2.12479018"
```

环境变量（**新终端才生效**）：

```powershell
setx ANDROID_HOME "$env:LOCALAPPDATA\Android\Sdk"
setx NDK_HOME    "$env:LOCALAPPDATA\Android\Sdk\ndk\27.2.12479018"
setx JAVA_HOME   "C:\Program Files\Eclipse Adoptium\jdk-17.0.x-hotspot"
```

### 6. Rust 的 Android target

```powershell
rustup target add aarch64-linux-android armv7-linux-androideabi i686-linux-android x86_64-linux-android
```

### 7. 一次性校验

```powershell
rustc -V; cargo -V
rustup target list --installed
node -v; pnpm -v; java -version
echo $env:ANDROID_HOME; echo $env:NDK_HOME; echo $env:JAVA_HOME
pnpm tauri info          # 期望 Environment 段全部 ✔
```

`pnpm tauri info` 是唯一可信的全量体检；`✘ Couldn't detect any Visual Studio...` 表示第 2 步没做完。

## macOS（仅桌面）

```bash
xcode-select --install
brew install rustup-init && rustup-init
rustup default stable
brew install node@22 && brew link --overwrite node@22
corepack enable && corepack prepare pnpm@9.15.0 --activate
```

macOS 的 `.icns` 需要单独生成（`pnpm tauri icon src-tauri/icons/icon.png`），
本仓库当前只提交 png/ico，macOS 打包前必须补跑。

## WSL：不建议

Android 构建在 WSL 里能用，但 `adb` 连 USB 设备、`.temp` 与 Windows 侧路径互通都更麻烦。
桌面端与后端开发用原生 Windows，CI 再用 Linux 容器。

## 常见坑

| 现象 | 原因 | 处理 |
|---|---|---|
| `link.exe not found` | 缺 MSVC 生成工具 | 第 2 步 |
| `NDK_HOME` 未设置 | tauri android init 失败 | 第 5 步 + 重开终端 |
| Gradle 报 JDK 版本 | 用了 JDK 21+ | 第 4 步，且 `JAVA_HOME` 指向 17 |
| `pnpm` 命令找不到 node | pnpm 是 npm 全局脚本，node 不在 PATH | 先把 node 的 bin 目录放进 PATH |
| Android 打包双重圆角 | `ICON.webp` 自带圆角底 | 另出透明前景层 + 纯色背景 |
