# 参与灵录 Capwise

这个仓库维护 Capwise（灵录）的 macOS 桌面版本。请将本项目的问题和 PR 提交到 [joeseesun/capwise](https://github.com/joeseesun/capwise)，与上游 Cap 的产品支持分开。

报告问题时附上应用版本、macOS 版本、复现步骤和预期结果；截图与日志应移除 API Key、私人录屏和个人信息。提交改动前运行相关 crate 的格式检查、编译和针对性测试；界面改动请附实际运行截图。不要提交密钥、用户项目、构建缓存或私有签名文件。

Capwise is the macOS-focused fork maintained here. Send issues and pull requests to this repository, include reproducible steps, and redact credentials and private media. Run focused formatting, compilation, and tests for your change. The inherited setup reference follows below.

---

## Running Cap

### Development Requirements

Before anything else, make sure you have the following installed:

- Node Version 24+
- Rust via [rustup](https://rustup.rs) (the workspace pins 1.88.0; the desktop app pins its own toolchain, which rustup installs on first build)
- Bun 1.4.4
- Docker ([OrbStack](https://orbstack.dev/) recommended)

### General Setup

Keep the root `react` and `react-dom` development dependencies aligned with `apps/mobile`. They keep Expo native dependencies deduplicated under Bun’s hoisted linker, while the web app retains its separately resolved React version.

Run `bun install`, then run `bun run env-setup` to generate a `.env` file configured for your environment.
It will ask you which apps you intend to run, whether you'd like to use Docker to run S3 (MinIO) and MySQL locally,
and allow you to provide overrides as needed.

Then run `bun run cap-setup` to install native dependencies such as FFmpeg.

On Windows, llvm, clang, and VCPKG must be installed.
On MacOS, cmake must be installed.
`bun run cap-setup` does not yet install these dependencies for you.

To run both `@cap/desktop` and `@cap/web` together, use `bun run dev`.
To run only one of them, use `bun run dev:desktop` or `bun run dev:web` respectively.

### `@cap/desktop` (desktop app)

The desktop app lives in `apps/desktop-gpui`. It is native Rust with a [GPUI](https://www.gpui.rs/) interface,
and [its README](apps/desktop-gpui/README.md) covers how it is built, packaged, and updated.

`bun run dev:desktop` checks out Cap's patched gpui next to this repository (`../zed-cap`) on first run,
then starts the app. On macOS it rebuilds on save and relaunches the app where you left it (`apps/desktop-gpui/dev.sh`);
on Windows and Linux it builds and runs the app once. The first build takes around ten minutes.

When running the desktop app from a terminal on macOS,
you will need to grant permissions (screen recording, microphone, etc.) to the terminal, not the Cap app.
For example, if you run `bun run dev:desktop` in the macOS `Terminal.app`, grant the permissions to `Terminal.app`.

#### Where are my recordings stored?

Development builds use the same data directory as an installed Cap:
`~/Library/Application Support/so.cap.desktop/recordings` on macOS,
`%APPDATA%\so.cap.desktop\recordings` on Windows,
and `~/.local/share/so.cap.desktop/recordings` on Linux.
Set `CAP_GPUI_APP_DATA_DIR` to keep a development build's recordings and settings separate.

### `@cap/web` (cap.so website)

When running `bun run dev` or `bun run dev:web`, a MySQL database and MinIO S3 server will also be using Docker.
If you want to _only_ run the `@cap/web` NextJS app, `cd` into `./apps/web` and run `bun run dev`.
