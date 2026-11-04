# Filmora Player (mpv + Rust)

高性能本地视频播放器：libmpv 内核 + Rust/egui 原生界面，内置可下载的 whisper 语音模型，实时识别语音并生成翻译字幕（原文 / 译文 / 双语）。

## 功能（MVP）

- **格式**：交给 libmpv + 全功能 FFmpeg（dav1d、libvpx、HEVC、AV1、VP9、ProRes、Opus、FLAC、DTS…），libass 字幕，libbluray/libdvdnav 光盘，libarchive 压缩包内播放，m3u/m3u8 播放列表。
- **硬解**：设置里可切换 自动 / 关闭 / 自动(回拷) / D3D11VA / NVDEC / DXVA2（Windows）/ VideoToolbox（macOS），并显示当前生效的解码器。
- **控制**：播放暂停、进度拖动（悬停预览时间）、音量、静音、倍速、字幕轨/外挂字幕、音轨、清晰度（降采样）、截图、播放统计、上一个/下一个（同目录自然排序）、画中画（置顶小窗）、全屏、拖放打开、右键菜单、快捷键。
- **实时字幕**：第二个无界面 mpv 实例把同一文件的音频提前解码成 16 kHz 单声道 → 能量 VAD 分句 → whisper.cpp（whisper-rs）识别 → 翻译 → 叠加显示；拖动进度会自动从新位置继续。可导出 SRT。
- **翻译来源**：Bilibili Index-Translate（免费免 Key，默认）、任意 OpenAI 兼容接口（OpenAI / DeepSeek / Ollama…）、Whisper 内置离线翻译（仅译英文）、仅转写。
- **模型管理**：tiny / base / small / medium / large-v3-turbo(q5)，断点续传、可走 hf-mirror.com 镜像；未下载模型时 CC 菜单只显示“下载模型”。
- 界面中英双语，Lucide 图标。

## 快捷键

空格 播放/暂停 · ←/→ 5 秒 · ↑/↓ 音量 · F/回车 全屏 · Esc 退出全屏/画中画 · M 静音 · S 截图 · [ ] 调速 · PgUp/PgDn 上/下一个 · C 实时字幕 · Ctrl/⌘+I 统计 · Ctrl/⌘+O 打开

## 构建

依赖：Rust stable、CMake、Clang（whisper-rs 编译 whisper.cpp 用）。libmpv 运行时动态加载，编译不需要 mpv 开发文件。

```bash
cargo build --release
```

运行时需要 libmpv：

- **Windows**：把 [shinchiro/mpv-winbuild-cmake](https://github.com/shinchiro/mpv-winbuild-cmake/releases) 的 `mpv-dev-x86_64-*.7z` 里的 `libmpv-2.dll` 放在 exe 旁边。
- **macOS**：`brew install mpv`（开发时直接用 Homebrew 的 libmpv；发布包由 `scripts/macos-bundle.sh` 把 libmpv 及依赖打进 .app）。
- 也可以用环境变量 `MPV_LIBRARY_PATH` 指定库文件。

## CI

`.github/workflows/build.yml`：

- Windows：下载 shinchiro 最新 `mpv-dev-x86_64` → 编译 → 便携 zip + Inno Setup 安装包。
- macOS：arm64（macos-14）与 x64（macos-15-intel）分别 `brew install mpv dylibbundler` → 编译 → 打包 .app（依赖全部内置、ad-hoc 签名）→ dmg。
- push 到 main 发布 `build-N` 预发布；推 `v*` 标签发布正式版。

## 架构

```
src/mpv.rs      libmpv 动态加载 + client API / render API 绑定
src/render.rs   mpv OpenGL render API → 离屏 FBO → blit 进 egui 画面（零拷贝，同一 GL 上下文）
src/app.rs      界面：标题栏、控制条、菜单、字幕叠加、统计、偏好设置
src/live.rs     实时字幕管线：音频旁路 → VAD → whisper → 翻译
src/models.rs   whisper 模型目录与断点续传下载
src/settings.rs 设置持久化（数据目录/FilmoraPlayer/settings.json）
```

## 许可

见 THIRD_PARTY.md。
