# Winget 清单模板

`CaoSL.Capwise` 的三文件清单，按 [winget 1.6.0 多文件格式](https://github.com/microsoft/winget-pkgs/blob/master/doc/manifest/schema/1.6.0/singleton.md) 写就。模板不能直接提交，发布前需要替换占位值。

## 每次发版要改的

1. 三个 YAML 里的 `PackageVersion` 改成 `apps/desktop-gpui/Cargo.toml` 的版本号。
2. `CaoSL.Capwise.installer.yaml`：
   - `InstallerUrl` 改成 Release 里 NSIS 安装包的 HTTPS 直链，例如
     `https://github.com/Cao-SL/capwise_win/releases/download/v0.3.9/Capwise-Windows-x64-Setup.exe`。
   - `InstallerSha256` 改成安装包的 SHA256（大写十六进制，64 字符）。
     `apps/desktop-gpui/scripts/build-windows-update-manifest.ps1` 会打印它并写入 `SHA256SUMS.txt`。
3. 若改了包名/发布者，同步改 `PackageIdentifier` 和三个文件名。

## 校验与提交

```powershell
winget validate --manifest .
winget hash <installer.exe>
```

本地源可直接用：

```powershell
winget install --manifest .
```

提交到社区仓库 `microsoft/winget-pkgs` 时，目录结构须为
`manifests/c/CaoSL/Capwise/<PackageVersion>/`，且安装包必须已做
Authenticode 签名（见 `scripts/sign-windows-release.ps1` 与
`.github/workflows/capwise-platform-build.yml` 的签名步骤）。

## 许可证说明

主代码是 AGPLv3，`cap-camera*` / `scap-*` 家族为 MIT，第三方组件各有
许可（见仓库根的 `LICENSE` 与 `licenses/`）。winget 只允许填一个
`License`，这里填 `AGPL-3.0-only` 并指向完整许可证文件。

## 与更新器的关系

winget 是分发渠道，和 App 内置更新器是两条独立通道：内置更新器读
`latest.json`（`installer.rs::RemoteManifest`），由
`build-windows-update-manifest.ps1` 生成。若同时启用两者，需保证版本号
一致，避免用户被两个来源反复提示升级。
