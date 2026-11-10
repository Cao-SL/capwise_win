# 灵录 Capwise

**中文** · [English](#english)

**录下来，讲清楚。**

免费的开源 Mac 录屏与剪辑工作台。字幕、画面、配音、封面与 AI 助手，从录制到成片，在一个窗口里完成。
A native Mac studio for screen recordings, captions, and AI-assisted editing.

[官网与界面预览](https://capwise.qiaomu.ai/) · [下载 macOS 版](https://github.com/joeseesun/capwise/releases/latest/download/Capwise.dmg) · [报告问题](https://github.com/joeseesun/capwise/issues) · [构建文档](apps/desktop-gpui/README.md) · [许可证](LICENSE)

> **本仓库是灵录的 Windows 版本**（fork 自 [joeseesun/capwise](https://github.com/joeseesun/capwise)）：跟踪上游更新，并在此基础上提供 Windows 原生支持。macOS 用户请直接使用上游官方版本。

![真实灵录编辑器：原创演示工程、双语字幕、可编辑文字与多轨时间线](docs/assets/workbench.webp)

_真实安装版界面。演示中的 NOVA 是原创虚构素材，不包含私人录屏；标题、字幕和缩放均在灵录中编辑。_

产品演示、操作教程、课程讲解，不必在录屏、字幕、配音和封面工具之间来回搬运。灵录把这些步骤放在一个本地编辑器里：先录下来，再慢慢打磨。

[![最新版本](https://img.shields.io/github/v/release/joeseesun/capwise?label=macOS)](https://github.com/joeseesun/capwise/releases/latest) [![AGPLv3](https://img.shields.io/badge/license-AGPLv3-586B59)](LICENSE)

适合做**产品演示、软件教程、课程录制和知识分享**。当前发布包支持 macOS 13.3+、Apple Silicon，经过 Developer ID 签名与 Apple 公证。

## Windows 版本

灵录 Windows 版在本仓库构建与维护：上游的录制、时间线、字幕与 AI 助手能力，加上 Windows 原生 shell 集成。以下截图来自本仓库 0.3.10 构建，Windows 11 实机运行。

![Windows 版设置页：外观、语言与录制行为](docs/assets/windows-settings.png)

| 录制主窗口 | 欢迎引导 |
| --- | --- |
| ![Windows 录制主窗口](docs/assets/windows-main.png) | ![Windows 欢迎引导](docs/assets/windows-welcome.jpg) |

### Windows 原生集成

- **任务栏进度**：导出时任务栏按钮显示实时进度（`ITaskbarList3`）
- **跳转列表**：右键任务栏图标直达最近 / 常用工程
- **系统通知**：导出完成后弹 Windows 通知，可在设置里开关
- **Win11 视觉**：圆角窗口、Mica/Acrylic 背景材质、跟随系统强调色
- **多 GPU 对齐**：录制、编码、解码自动与渲染 GPU 对齐，混合显卡机型不再选错卡
- **MSIX 分发**：安装包之外另提供 MSIX，适合企业侧载
- **密钥本地保存**：AI 密钥保存在本机存储，重启后保留

### 获取与构建

当前通过本仓库源码构建（CI 与 Release 产物陆续完善中）：

```powershell
git clone https://github.com/Cao-SL/capwise_win.git
cd capwise_win
bun install
bun run cap-setup
bun run gpui:build x86_64-pc-windows-msvc
./scripts/assemble-windows-payload.ps1 -OutDir ./dist/Capwise
```

签名、更新分发、winget 与 MSIX 的完整链路见 `packaging/windows/RELEASE.md`；手工准备原生依赖的流程见 `apps/desktop-gpui/docs/windows-gpu-roadmap.md`。

> 首次运行若遇 SmartScreen 提示，点「更多信息 → 仍要运行」（每台机器一次）。想完全避开：下载 zip 后先执行 `Unblock-File .\Capwise-*.zip` 再解压。

## 录完之后，少折腾几步

- **操作太细，看不清？** 自动缩放、光标效果与标注，把注意力带到重点。
- **字幕太长，改不动？** 在文字列表里校对、查找替换，再翻译成双语字幕。
- **画面做好了，还得换工具？** 文字、贴纸、配乐、配音和片头封面继续在同一工程里加工。
- **不知道从哪调参数？** 告诉助手你想改什么，审阅后应用；也能直接操作画面。

## 你可以用它做什么

| 能力                   | 你得到的结果                                                             |
| ---------------------- | ------------------------------------------------------------------------ |
| 屏幕、摄像头与声音录制 | 把操作过程和讲解录进同一个项目                                           |
| 时间线编辑             | 裁剪、缩放、转场、文字与形状标注，突出观众该看的地方                     |
| 字幕工作台             | 在「内容 / 翻译 / 样式」中分别校对文字、审阅译文、调整外观               |
| 双语字幕               | 选择常见语言或自定义目标语言，原文与译文分别设置颜色、描边、阴影与字体   |
| 音乐与配音             | 从音频面板直接添加配乐或录制配音，调整音量与淡入淡出                     |
| AI 剪辑助手            | 用一句话提出需求，审阅修改方案后应用，也可以撤销                         |
| 助手内制作封面         | 从描述到背景生成、标题和时长设置、预览应用，在同一工作区完成             |
| 模型与 ACP 接入        | 配置自己的模型服务，或连接本机已安装并登录的 ACP 助手                    |
| 共享素材库与在线搜索   | 图片、音乐、视频以缩略图管理，跨项目复用；图片与视频搜索分别进入对应来源 |
| 文字与贴纸             | 直接编辑文字、设置动效，使用可搜索的 Emoji 和 Lucide 图标                |
| 提词器                 | 写稿与读稿分开，支持滚动、调速、跳行和录制联动                           |
| 本地项目与导出         | 保存可继续编辑的项目，再导出成视频或字幕文件                             |

## 五分钟开始

1. 下载 Apple Silicon 版 [Capwise.dmg](https://github.com/joeseesun/capwise/releases/latest/download/Capwise.dmg)。
2. 打开 DMG，将 **Capwise** 拖到 **Applications**，再启动「灵录」。
3. 按引导授权屏幕录制；需要录人声或摄像头时，再授予相应权限。
4. 录制一段视频，或导入已有视频。在编辑器中处理字幕、画面和声音。
5. 点击「导出」，选择输出设置和保存位置。

基础录制与编辑不需要注册 Cap 云服务账号。AI 功能可选：请在「设置 → AI 模型」中配置自己的服务。

上游当前提供 **macOS 13.3+ / Apple Silicon** 安装包；Intel 与 Linux 暂无发布包。**Windows 版本由本仓库提供**，见上文「Windows 版本」。

## 先打开一份好看的工程

[下载「灵感成片」演示工程](https://capwise.qiaomu.ai/downloads/capwise-demo.zip) · [看 24 秒导出视频](https://capwise.qiaomu.ai/downloads/capwise-demo.mp4)

解压后在灵录中打开 `.cap` 文件：米白与墨绿配色、三个章节、8 个可编辑文字图层、6 条双语字幕、手动缩放，以及带淡入淡出的轻配乐。无需配置 AI 或开始录制，就能试试修改标题、翻译样式和时间线。

## 看看实际工作区

### 字幕与文字剪辑

![字幕内容与双语预览](docs/assets/captions.webp)

「内容 / 翻译 / 样式」分开操作。原文与译文字体、颜色、描边和阴影独立设置；翻译可选择常见语言，也支持自定义。审阅后导出视频或单独导出 SRT / VTT。

普通字幕删除只修改文字；明确进入「按文字剪辑视频」，才会剪掉对应的音视频。两种操作的结果不同，应用前请检查范围。

### AI 助手

在声音标签右侧打开助手，选择模型、发送快捷任务或输入自己的要求。字幕与编辑方案可以审阅、应用和撤销。**制作封面会直接调用所配置的图片服务并应用结果**，对话内可预览、继续编辑、换一张或撤销；生成可能收费。

### 素材、文字与提词器

![项目素材、共享素材库与在线素材的真实界面](docs/assets/assets.webp)

素材集中管理，常用内容跨项目复用。文字支持标题、字幕条、标签、高亮等样式，以及独立入场、持续和退场动效。提词器支持写稿和读稿，录制时不用来回找稿子。

## AI 助手如何工作

```text
描述目标 → 生成方案 → 查看修改前后 → 应用 → 继续调整 / 撤销
```

可以这样说：

- 「润色这些字幕，保留人名、数字和原意。」
- 「隐藏字幕中的口头禅。」
- 「把字幕翻译成法语，并显示双语。」
- 「把第一条背景音乐调低 6 dB。」
- 「做一个浅绿色的产品演示封面，时长 1 秒。」

助手会先展示方案。对话中的模型选择只影响当前助手会话；你可以搜索模型，也可以进入模型设置管理服务。输入框支持 **⌘ Enter 发送**，运行中可以停止。

封面背景通过你配置的图片服务生成并应用，可以继续编辑、换图或撤销。新封面默认 1 秒，时长可编辑；标题仍然是可修改的文字。请先确认服务配置和费用。

### 配置你的模型

| 设置页                         | 用途                                  |
| ------------------------------ | ------------------------------------- |
| 字幕识别                       | 把音频识别为字幕                      |
| 内容生成                       | 字幕校对、翻译和编辑助手              |
| 封面生成                       | 生成封面背景，支持自定义地址和模型 ID |
| 内容生成 → 本地 AI 助手（ACP） | 检测并连接已安装的助手，读取可用模型  |

需要 API Key 的服务先填写密钥，再获取并选择模型；提供商没有开放模型列表时，可以手动填写模型 ID。火山方舟也支持以 `ep-…` 开头的推理接入点。

快捷按钮会直接发送任务，显示简短名称，背后使用完整的任务提示词。助手会把允许隐藏的口头禅范围一并交给模型；无效方案会自动修正一次，仍失败时保留原项目。

输入框旁的「技能」可搜索本机常见技能目录中的 `SKILL.md`，或导入指定文件。只发送当前选中的技能正文给所选 API / ACP 模型；技能脚本和引用文件不会自动执行或读取。修改仍通过灵录的预览、应用和撤销流程。

ACP 使用本机已有的登录状态。不同适配器的安装、登录和模型支持不同；检测到命令不代表已经登录成功。灵录中的 ACP 编辑助手仅提出可审阅的修改方案，不授予它终端或文件读写权限。

## 数据与费用

- 项目和媒体保存在本机，基础编辑不依赖 Cap 云服务。
- macOS 上的 API Key 保存在登录钥匙串；Windows 版保存在本机共享存储文件（不上传）；更新应用不会主动清空密钥。
- 使用云端字幕识别时，音频会发送给所选识别服务。
- 字幕校对、翻译和助手请求会发送相关字幕文字与编辑上下文给你选择的模型。助手不会把整个录屏上传给文本模型。
- 封面生图会发送图片描述给图片服务商。模型服务可能收费，费用与额度由服务商决定。
- 使用本地模型仍可能需要先下载模型文件；ACP 服务是否联网取决于所连接的助手。

## 当前边界

- 通用字幕改写一次使用最多 80 条字幕的上下文；长视频需要分批处理。明确的关键词查找替换会在本地搜索全部字幕。
- 「去口头禅」默认处理字幕显示，不自动剪除语音。只有明确要求剪掉对应语音 / 视频并审阅方案，才会执行媒体裁剪。
- AI 可能改错专有名词、语气或翻译，请在应用和导出前审阅。
- 配音质量也取决于麦克风、输入电平和环境；增益调整不能恢复原始录音中缺失的细节。
- 本项目仍在持续迭代。重要项目建议保留原始媒体与备份。

## 开发与验证

灵录基于 [Cap](https://github.com/CapSoftware/Cap)，桌面界面使用 Rust + GPUI，媒体管线使用项目中的录制、渲染和导出 crates。

<details>
<summary>从源码构建</summary>

需要 macOS、Xcode Command Line Tools、Rust、Bun 及项目的原生媒体依赖。桌面应用拥有独立 Cargo workspace。首次准备会下载原生依赖及打过补丁的 GPUI 源码。

```bash
git clone https://github.com/joeseesun/capwise.git
cd capwise
bun install
bun run cap-setup
bash scripts/prepare-gpui-dependency.sh
cd apps/desktop-gpui
cargo build --bin cap-gpui
```

详细说明见 [桌面开发文档](apps/desktop-gpui/README.md)；分发签名、公证和更新包流程见 [发布文档](apps/desktop-gpui/docs/release.md)。开发环境安装不等于已公证的分发包。

</details>

验证重点包括：修改方案的范围校验、字幕时间保留、撤销恢复、模型接入，以及安装包签名。每个版本的实际验证结果与已知限制写在 [Release 说明](https://github.com/joeseesun/capwise/releases) 中。

## 反馈与来源

遇到问题请提交 [Issue](https://github.com/joeseesun/capwise/issues)，附上应用版本、macOS 版本和复现步骤。**请勿上传 API Key 或未经处理的私人录屏。**

- 基于 Cap，保留上游版权与许可；主要代码为 AGPLv3，部分录制与相机 crates 为 MIT，详见 [LICENSE](LICENSE)。
- 助手交互参考 [Qiaomu Agent](https://github.com/joeseesun/Qiaomu-agent)，在原生 GPUI 界面中实现。
- 图标使用项目已有图标与 [Lucide](https://lucide.dev/) 风格资源；字体、媒体与第三方组件遵循各自许可。
- 随附配乐的来源记录见 [音乐说明](apps/desktop-gpui/assets/music/NOTICE.md)。

由 [向阳乔木](https://qiaomu.ai) 维护 · [博客](https://blog.qiaomu.ai) · [乔木推荐](https://tuijian.qiaomu.ai) · [X @vista8](https://x.com/vista8) · [GitHub](https://github.com/joeseesun) · 公众号「向阳乔木推荐看」

---

<a id="english"></a>

## English

**Capwise (灵录)** is a local-first Mac screen recording and editing studio, based on Cap. Record a demo, refine captions, add narration or music, create a cover, and export your video in one workspace.

### Install

Download the Apple Silicon [Capwise.dmg](https://github.com/joeseesun/capwise/releases/latest/download/Capwise.dmg), drag Capwise to Applications, and follow the recording permission guide. This release targets macOS 13.3+ on Apple Silicon. Intel, Windows, and Linux packages have not been verified for this fork.

Core recording and editing do not require a Cap Cloud account. Optional AI features use your own model providers or installed ACP agents. Configure speech recognition, text generation, and image generation separately in Settings → AI Models.

Published macOS packages are Developer ID signed and Apple notarized. See the [release notes](https://github.com/joeseesun/capwise/releases) for version-specific verification. Try the [editable 24-second demo project](https://capwise.qiaomu.ai/downloads/capwise-demo.zip) without configuring any AI provider.

### Highlights

- Native Rust + GPUI editor with a timeline, zooms, captions, music, narration, and annotations.
- Separate content, translation, and style workspaces; independent original/translated subtitle styling.
- An editing assistant that proposes changes for review before applying them, with undo support.
- In-assistant cover generation and application, editable titles and cover duration (1 second for a new cover).
- Shared thumbnail-based assets, searchable Emoji/Lucide stickers, and an editable teleprompter.
- Searchable model selection and local ACP connections.
- Quick actions with task-specific instructions, plus a local SKILL.md picker and import. Only the selected skill body is sent; scripts and referenced files are not run automatically.

### Privacy, costs, and limits

Projects remain local. Cloud transcription sends audio to the selected service; AI text features send relevant text and editing context. Cover generation sends the image description. API keys are stored in the macOS login Keychain. Provider charges may apply.

General AI rewrites use up to 80 captions of context per request; explicit keyword search/replacement searches the complete caption list locally. Filler hiding changes the displayed text. Explicit speech/video removal is a separate operation and must be reviewed. Image generation directly calls your configured provider and applies a cover, with edit and undo controls; provider charges may apply. Always review AI output. ACP integrations require their own installation and login; the in-app agent does not receive filesystem or terminal permissions.

See the [desktop development guide](apps/desktop-gpui/README.md), [release notes](https://github.com/joeseesun/capwise/releases), and [license](LICENSE). Capwise retains Cap's upstream licensing: primarily AGPLv3, with MIT and third-party exceptions as documented. [Report a bug](https://github.com/joeseesun/capwise/issues) without including credentials or private recordings.
