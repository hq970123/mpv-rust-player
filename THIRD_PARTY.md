# Third-party components

- **libmpv** (mpv) — LGPL-2.1+ when built with `-Dgpl=false`, otherwise GPL-2.0+. Loaded dynamically at runtime.
- **FFmpeg** — LGPL-2.1+, or GPL when built with GPL parts (x264, x265, etc.).
- **libplacebo, libass, libbluray, libdvdnav, libarchive, dav1d, libvpx** — various LGPL / BSD licences, shipped inside the libmpv build.
- **whisper.cpp** (via whisper-rs) — MIT. Speech models from huggingface.co/ggerganov/whisper.cpp (MIT, OpenAI Whisper weights).
- **egui / eframe** — MIT or Apache-2.0.
- **Lucide icons** — ISC (assets/LUCIDE-LICENSE).

## Licence note

The CI downloads prebuilt libmpv: shinchiro's Windows builds and Homebrew's mpv on macOS. Both are
built with GPL components enabled. That is fine for direct distribution with source available.
For a Microsoft Store / closed-source release, swap in an LGPL-only libmpv instead: mpv built with
`-Dgpl=false` and FFmpeg without `--enable-gpl` (a player needs no encoders such as x264/x265). The app
only dlopen()s `libmpv-2.dll` / `libmpv.2.dylib`, so swapping the library needs no code change.
