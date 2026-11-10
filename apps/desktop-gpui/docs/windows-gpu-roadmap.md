# Windows GPU 剩余项：现状与改造点

本文件记录逐条审计的结果：哪些基础设施已经就位、缺的到底是哪一环、
以及每项需要在哪些文件落地。目标是让后续改动有据可依，避免重复摸索。

## 已就绪的基础设施（逐条核对过源码）

| 能力 | 位置 | 状态 |
|---|---|---|
| D3D11 共享纹理 + NT handle 创建 | `crates/frame-converter/src/d3d11.rs:937-1010` | 活跃，`CreateSharedHandle` |
| D3D11→D3D12 导入 wgpu | `crates/rendering/src/d3d_texture.rs:196-280` | 活跃，`OpenSharedHandle` + `texture_from_raw` |
| 格式映射 / NV12 双平面导入 | `d3d_texture.rs:184-193`, `D3D11WgpuInterop::import_nv12_planes` (`yuv_converter.rs:1617`) | 活跃 |
| 硬件解码输出纹理池 | `crates/video-decode/src/media_foundation.rs:177-270`, `FrameTextures` / `MFDecodedFrame.textures` (`L139-145`) | 活跃，带 `Arc` 回收 (`recycle_textures` L852) |
| 渲染层 GPU 纹理消费分支 | `crates/rendering/src/layers/camera.rs:185`, `display.rs:115` → `convert_nv12_with_fallback` | 代码在，但运行时走 CPU 回退 |
| DirectML EP 检测与 CPU 回退 | `crates/camera-effects/src/segmentation.rs:362-402` | 已完成 |
| 多 GPU 设备对齐 | `crates/d3d-adapter` + `crates/rendering/src/adapter_luid.rs` | 本轮已接线 |

## 项目 1：硬解输出 D3D11 共享纹理

**缺口精确定位**：`crates/video-decode/src/media_foundation.rs:816-846`
`map_staging` 把 staging 纹理 `Map` 出来拷贝进 `NV12Data { data: Arc<Vec<u8>>, ... }`；
`crates/rendering/src/decoder/media_foundation.rs:185-205` 的 `answer` 闭包只拿到
`NV12Data`，调用 `DecodedFrame::new_nv12_with_arc`（纯 CPU）。

同时 `DecodedFrame::new_nv12_with_d3d11_texture*` (`decoder/mod.rs:517-557`)
**零调用者**，导致 `d3d11_texture_backing()` 恒为 `None`，camera/display 两处
零拷贝分支永远不进入。

**改造清单**：
1. `MFDecodedFrame` 已经携带 `Arc<FrameTextures>`，把 `nv12.texture` /
   `nv12.handle` / `y.handle` / `uv.handle` 通过 `NV12Data` 或新增字段透传到
   `answer` 闭包；`FrameTexture` 与 `FrameTextures` 需可跨线程持有（已有
   `unsafe impl Send/Sync`）。
2. `answer` 里改用 `new_nv12_with_d3d11_texture_and_handle`，并把
   `Arc<FrameTextures>` 挂到 `DecodedFrame` 上，保证 MF 复用 slice 前纹理不被
   释放；渲染完成后调 `recycle_textures`。
3. `SendableD3D11Texture` 目前只存一个 handle，需扩展为 Y/UV 双 handle
   （`d3d11_y_handle()` / `d3d11_uv_handle()` getter 已存在，`decoder/mod.rs:565+`）。
4. **生命周期是主要风险**：MF 在 sample 释放后立即复用输出 slice，纹理必须
   持有到 GPU 采样完成；需要 fence 或 `Arc` 计数 + `recycle_textures` 配合。
   这一环必须有真机 GPU 验证（不同 NVIDIA/AMD/Intel 驱动行为不同）。

## 项目 2：预览零拷贝阶段 2

**阶段 1 现状**：`apps/desktop-gpui/src/editor_window/streaming.rs` 的
`PreviewStream` 用 `StreamingImage(Rgba8)` 常驻一张 D3D11 纹理，消除了
unpad / BGRA 交换 / 每帧 atlas 打包，但每帧仍有一次
`zed-cap/crates/gpui_windows/src/directx_atlas.rs:373` 的 `UpdateSubresource`
（CPU→GPU 拷贝）。

**阶段 2 目标**：wgpu 渲染结果直接作为共享纹理交给 gpui D3D11，连
`UpdateSubresource` 都省掉。

**改造清单**：
1. `crates/rendering/src/frame_pipeline.rs:2040-2120`：输出纹理创建时加
   D3D12 `D3D12_HEAP_FLAG_SHARED`，用 `ID3D12Device::CreateSharedHandle`
   导出 NT handle。可参考 `frame-converter/d3d11.rs:979-1010` 的
   `extract_shared_handle`（D3D11 版）。
2. `RenderedFrame` 携带 handle + fence 值；`app_windows.rs` 的帧泵把 handle
   交给 `StreamingImage`。
3. `zed-cap/crates/gpui_windows/src/directx_atlas.rs`：给 `new_stream`
   (`L387`) 增加一条 `ID3D11Device::OpenSharedResource` 路径，用共享纹理
   代替 `CreateTexture2D` + `UpdateSubresource`；同时保留现有路径作回退。
   补丁在 `apps/desktop-gpui/patches/`，需同步更新
   `scripts/prepare-gpui-dependency.sh`。
4. **同步**：D3D12 与 D3D11 跨设备必须用共享 fence（`ID3D12Fence` +
   `ID3D11Fence` / `D3D11_FENCE_FLAG_SHARED`），否则会出现撕裂或读到半帧。
5. **回退条件**：handle 创建失败、`OpenSharedResource` 失败、驱动不支持
   共享 fence 时，自动回到阶段 1 的 `UpdateSubresource` 路径；用户无感知。

这是跨仓库改动（本仓库 + zed-cap 补丁），且 GPU 同步语义必须实测。
建议单独分支，先在多台不同 GPU 上验证阶段 2 与回退分支，再合入。

## 项目 3：DirectML 与 wgpu 共设备

**结论：已实现**。`crates/camera-effects/src/segmentation.rs:362-402` 用
`OnceLock` 做运行时 DirectML 能力检测，注册 `ort` 的
`DirectMLExecutionProvider`；失败时打印原因并回退 CPU。之前本机
`ort` 预编译库要求 VS2022 17.12+ 是构建链问题，不影响这段代码。

「共设备」的进一步含义是让 ONNX Runtime 的 D3D12 设备与 wgpu 的
D3D12 设备是同一个实例，避免两份 command queue。这需要把
`OrtSession` 的 device 指针注入，属于上游 `ort` crate 的能力边界，
当前版本没有公开注入点。记为上游依赖限制，不在本仓库实现。

## 项目 4：AV1 硬件编码

**结论：产品编码路径是 H.264-only**。
`crates/enc-ffmpeg/src/video/h264.rs` 的候选列表
(`L1446-1533`) 只有 `h264_nvenc` / `h264_qsv` / `h264_amf` /
`h264_mf` / `libx264`；全仓无 `av1` 相关代码。`VideoCodec` 枚举
只存在于 `crates/verify`（测试工具），不参与产品编码。

**改造清单**（属于独立工程，非小改动）：
1. 抽出一个 codec 抽象，把 `h264.rs` 的编码器选择、码率/帧率上限、
   hwaccel 检测泛化到 `av1_nvenc` / `av1_qsv` / `av1_amf` / `libaom-av1`。
2. 容器封装（MP4 里 AV1 是 `av01`）、解码端兼容性、播放器范围都要一起改。
3. 运行时能力检测 + 自动回退到 H.264（老 GPU 没有 AV1 编码器）。
4. 需要真实 NVIDIA RTX 40 / Intel Arc / AMD RX 7000 验证。

## 高刷屏播放计时

`max_fps` (`settings_window.rs:283-289`, 24/25/30/60/120) 是**录制质量**设定，
不是播放刷新率；播放帧率由录制源决定。Windows 没有独立缺口，按
「Win 不需要时就不考虑」跳过。若将来要加「预览跟随显示器刷新率节流」，
入口是 `app_windows.rs` 的帧泵 + `EnumDisplaySettingsW` 的
`dmDisplayFrequency`。
