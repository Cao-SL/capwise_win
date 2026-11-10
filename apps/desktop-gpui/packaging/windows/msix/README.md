# MSIX 分发

Tauri 只产出 NSIS 安装包，MSIX 是额外的分发渠道（适合企业侧载、Intune、
Microsoft Store 提交流）。这里提供 `AppxManifest.xml` 模板和
`apps/desktop-gpui/scripts/build-capwise-msix.ps1` 打包脚本。

## 用法

```powershell
./apps/desktop-gpui/scripts/build-capwise-msix.ps1 `
    -Payload ./candidate/installed `
    -Version 0.3.9 `
    -Publisher "CN=Your Code Signing Subject" `
    -PfxBase64 $env:WINDOWS_SIGNING_PFX `
    -PfxPassword $env:WINDOWS_SIGNING_PFX_PASSWORD
```

`-Payload` 是**已安装的程序目录**（含 `Capwise.exe`、`cap-cli.exe`、
`cap-muxer.exe`、FFmpeg DLL 与资源）。CI 里可以直接拿
`verify-capwise-windows.ps1` 用 `/D=` 装出来的目录，或在 `tauri bundle`
之后从 `target/<triple>/release/` 组装。脚本会复制它、填入
`AppxManifest.xml` 的 `Identity.Version` / `Publisher`、从
`icons/icon.png` 生成 44/150/50 三种 logo，然后 `makeappx pack`
并用 `makeappx unpack` 往返校验包结构与 identity 版本。

产物：`dist/msix/<version>/Capwise-<version>.msix`。

## 必需条件

- Windows 10 SDK 的 `makeappx.exe`（脚本会按 PATH → 最新 SDK 顺序查找，
  也可用 `-MakeAppxPath` 指定）
- Python + Pillow（生成 logo；脚本会依次找 `python` / `py` / 本机捆绑 Python）
- 可选：`signtool.exe`（脚本复用 `scripts/sign-windows-release.ps1`）

## Publisher 与签名

`Identity.Publisher` 必须和签名证书的 **Subject 完全一致**，否则 Windows
拒绝安装。默认模板是 `CN=Capwise`（占位），正式发布前用 `-Publisher` 换成
证书 subject，并用 `-PfxBase64` 签名。未签名包只能用于开发侧载
（`Add-AppxPackage -AllowUnsigned` 或开启开发人员模式）。

## 与 NSIS / winget 的关系

三个渠道互相独立，版本号要保持一致：

| 渠道 | 产物 | 安装范围 |
|---|---|---|
| NSIS | `Capwise-Windows-x64-Setup.exe` | currentUser，内置更新器 |
| winget | 三文件 YAML 清单 | 指向上面的 NSIS 包 |
| MSIX | `Capwise-<version>.msix` | 每用户 AppX，更新走 AppInstaller/Store |

MSIX 是容器化的，程序不能写自己的安装目录；Capwise 的数据都在
`%APPDATA%`/`%LOCALAPPDATA%`，这点已兼容。内置 Tauri 更新器在 MSIX 里
会尝试覆盖自身目录，如需 MSIX 分发，建议用 `capwise-preview.json` 那种
`"updater": null` 的配置另出一个构建。

## 清单里的能力

`runFullTrust`（Win32 全信任）、`internetClient`、`microphone`、
`webcam`。屏幕捕获用的是 Desktop Duplication / Graphics Capture，不需要
额外清单能力。`uap3:FileTypeAssociation` 注册 `.cap`，与 NSIS 的
`fileAssociations` 对应。
