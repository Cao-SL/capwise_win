# 发布灵录（Capwise）

灵录通过 GitHub Releases 分发。App 内置更新器按系统、芯片架构和 Linux 安装包格式选择更新。

- 更新清单：`https://github.com/joeseesun/capwise/releases/latest/download/latest.json`（总是指向最新一个**已发布**的 Release，草稿和预发布不算）。
- 更新包：Mac 使用 `.app.tar.gz`，Windows x64 使用 NSIS `.exe`，Ubuntu 24.04 x64 使用 `.deb`。每个包用 minisign 签名，签名放在 `latest.json` 里。App 只接受 `packaging/updater.pub` 这把公钥签过的包。
- App 启动 10 秒后检查一次，之后每 6 小时检查一次。正在录制、导出、导入或转写时不会弹窗，等任务结束再提示；"更新下载"设为"从不"时只响应手动检查。可在「关于」页检查更新。只有一个稳定通道，没有 nightly。

`latest.json` 的格式：

```json
{
  "version": "0.2.0",
  "notes": "本次更新的说明",
  "pub_date": "2026-11-08T08:00:00Z",
  "platforms": {
    "darwin-aarch64": {
      "url": "https://github.com/joeseesun/capwise/releases/download/v0.2.0/Capwise_0.2.0_aarch64.app.tar.gz",
      "signature": "<.minisig 文件内容的 base64>"
    }
  }
}
```

没有当前平台条目的清单会被当成"没有更新"。

## 一次性配置

以下凭据都只存在钥匙串或本机私密目录里，**不要**写进仓库、脚本或 `.env`。

### 1. Developer ID 证书

1. 在 [Apple Developer → Certificates](https://developer.apple.com/account/resources/certificates/list) 创建 **Developer ID Application** 证书，下载后双击装进"登录"钥匙串。
2. 确认能看到：

   ```bash
   security find-identity -v -p codesigning
   ```

   应有一行 `Developer ID Application: <名字> (<Team ID>)`。有多个时用 `CAPWISE_SIGN_IDENTITY` 指定。

### 2. 公证（notarytool）凭据

在 [appleid.apple.com](https://appleid.apple.com) → 登录与安全 → App 专用密码，生成一个密码，然后**自己**在终端运行（会提示输入那个密码，存进钥匙串）：

```bash
xcrun notarytool store-credentials capwise-notary --apple-id <你的 Apple ID> --team-id <Team ID>
```

profile 名默认是 `capwise-notary`，换名字时设 `CAPWISE_NOTARY_PROFILE`。验证：

```bash
xcrun notarytool history --keychain-profile capwise-notary
```

### 3. 更新签名密钥（minisign）

私钥决定谁能给所有已安装的灵录推送代码，**丢了就无法再推送更新，泄露了别人就能推送**。

```bash
brew install minisign
mkdir -p ~/.config/capwise && chmod 700 ~/.config/capwise
minisign -G -p apps/desktop-gpui/packaging/updater.pub -s ~/.config/capwise/updater.key
```

- 设置一个强密码。把密码存进钥匙串，发布脚本会自动读取：

  ```bash
  security add-generic-password -a "$USER" -s capwise-updater-key -w
  ```

- 把私钥文件和密码各自备份到密码管理器（例如 1Password）。
- 提交 `packaging/updater.pub`（公钥，可以公开）。只有包含这把公钥的版本才会自动更新，所以**第一个公开版本之前**就要完成这一步；在此之前构建的版本不会检查更新。
- 私钥放在别处时设 `CAPWISE_UPDATER_KEY=<路径>`；CI 等无法访问钥匙串的环境用 `CAPWISE_UPDATER_KEY_PASSWORD`。

### 4. 工具

```bash
brew install minisign
bun install && bun run cap-setup   # 准备 target/native-deps、onnxruntime 等原生依赖
```

DMG 用 `dmgbuild 1.6.5`，脚本第一次运行时会自动装到 `apps/desktop-gpui/target/release-tools`。

## 每次发版

1. **试跑**（不公证、不需要正式密钥，几分钟内发现签名或打包问题）：

   ```bash
   apps/desktop-gpui/scripts/release.sh 0.2.0 --dry-run
   ```

   没有正式私钥时会用临时密钥签名更新包，产物只能用来检查，不能发布。

2. **写更新说明**，例如 `apps/desktop-gpui/release-notes/0.2.0.md`：先中文、后英文，写用户能感知的变化，不贴提交记录。

3. **正式构建**：

   ```bash
   CAPWISE_RELEASE_NOTES=apps/desktop-gpui/release-notes/0.2.0.md \
     apps/desktop-gpui/scripts/release.sh 0.2.0
   ```

   脚本依次：改 `Cargo.toml` 和 `Info.plist` 的版本号 → release 构建 → 由内到外用 Developer ID 签名（安全时间戳 + hardened runtime，并检查每个可执行文件都带 runtime 标志）→ 公证并 staple App → 生成并签名更新包，用 `packaging/updater.pub` 验签 → 生成 DMG → 签名、公证、staple DMG → 写 `latest.json` 和 `SHA256SUMS.txt`。

   产物在 `apps/desktop-gpui/dist/<版本>/`。`--skip-build` 可跳过编译、复用上次的 release 二进制。

4. **检查分发包**：脚本在签名前比对壁纸、音乐、字幕字体和必要运行时；签名、公证后从 DMG 和更新归档各取出一份 App，核对资源、版本、架构、签名和公证票据。打开 DMG 看窗口布局，把 App 拖到"应用程序"后启动一次；`spctl -a -vv /Applications/Capwise.app` 应显示 `Notarized Developer ID`。

5. **通过 PR 合并发布源码**：在 `codex/` 功能分支提交版本号、公钥、源码和说明，推送该分支后向 `joeseesun/capwise` 的 `main` 创建 PR。验证通过后合并，再在合并提交上创建版本 tag。不要直接推送 `main`。打包源码必须与合并后的源码一致。

   ```bash
   git push -u origin HEAD
   gh pr create --repo joeseesun/capwise --base main --body-file /tmp/release-pr.md
   ```

6. **发布 Release**（脚本最后会打印同样的命令）：

   ```bash
   gh release create v0.2.0 --repo joeseesun/capwise --title "灵录 Capwise 0.2.0" \
     --notes-file apps/desktop-gpui/release-notes/0.2.0.md \
     apps/desktop-gpui/dist/0.2.0/Capwise_0.2.0_aarch64.dmg \
     apps/desktop-gpui/dist/0.2.0/Capwise.dmg \
     apps/desktop-gpui/dist/0.2.0/Capwise_0.2.0_aarch64.app.tar.gz \
     apps/desktop-gpui/dist/0.2.0/latest.json \
     apps/desktop-gpui/dist/0.2.0/SHA256SUMS.txt
   ```

   想先检查可以加 `--draft`，确认后在网页上发布；**只有发布后** `releases/latest/download/latest.json` 才会指向它，已安装的 App 才会看到更新。

7. **从外部验证**：

   ```bash
   curl -sL https://github.com/joeseesun/capwise/releases/latest/download/latest.json
   ```

   版本号正确、`url` 能下载。再用一台装着旧版的机器验证自动检查、下载和重启。从 0.3.5 起，也可以在「关于」页手动检查。

### 仓库分享图

README 使用 `docs/assets/assistant-workbench.png` 的真实运行截图。GitHub 仓库的 Social preview 需要另到 **Settings → General → Social preview → Upload an image** 手动上传；本次 CLI 发布不会修改这项设置。

### Intel、Linux 和 Windows 候选包

GitHub Actions 的 `Capwise platform candidates` 在原生 Intel Mac、Ubuntu 24.04 x64 和 Windows x64 环境构建候选包。下载安装包与启动验证完成后，再决定哪些产物可以公开分发。工作流不需要生产签名凭据。

Intel 候选包从工作流下载并解开后，可在持有签名凭据的 Mac 上用 `CAPWISE_PREBUILT_APP=/path/Capwise.app apps/desktop-gpui/scripts/release.sh <版本>` 签名和公证。脚本从主程序确定目标架构，并逐个核对内嵌二进制。将 Apple Silicon 的 `latest.json` 先复制到同一版本的产物目录，可以保留其更新条目。固定文件名 `Capwise.dmg` 由 Apple Silicon 构建生成。

发布前先在原生系统验证候选包。Windows 预览版的 minisign 签名用于 App 内的下载验签。Windows 系统代码签名独立于 minisign，正式发行包还需 Windows 代码签名。Ubuntu 安装更新时会请求系统管理员授权。

将同版本的两个 Mac 更新归档、`Capwise-Windows-x64-Setup.exe` 和 `Capwise-linux-x86_64.deb` 收到同一个目录。每个包旁边须有正式密钥生成的 `.minisig`。用 Apple Silicon 的 `latest.json` 作为基底生成完整清单：

```bash
node apps/desktop-gpui/scripts/assemble-update-manifest.mjs \
  <apple-silicon-latest.json> <signed-packages-dir> <output/latest.json>
```

脚本验证四个包的文件格式和签名，并保留 Apple Silicon 原有更新条目。默认所有包从同一正式版本的 Release 下载。Windows 和 Ubuntu 预览安装器在同版本预发布 Release 时，可将该预发布标签作为第四个参数。Intel Mac 更新需要签名并公证的 `.app.tar.gz`，官网安装使用 `.dmg`。

上传清单前逐一核对发布资产的 SHA-256 与候选包一致，再用上一版在各平台验证更新。四个平台都已接入时，正式发布必须带上这份完整清单。单平台构建生成的草稿应先补齐清单再设为最新版本。

### GitHub Actions

`.github/workflows/capwise-release.yml` 可以手动触发，在 `macos-15` 上跑同一个脚本并创建**草稿** Release。需要的仓库 secrets：

| Secret | 内容 |
| --- | --- |
| `CAPWISE_DEVELOPER_ID_P12` | Developer ID 证书导出的 .p12，base64 |
| `CAPWISE_DEVELOPER_ID_P12_PASSWORD` | .p12 密码 |
| `CAPWISE_NOTARY_API_KEY_P8` | App Store Connect API 密钥 .p8，base64 |
| `CAPWISE_NOTARY_API_KEY_ID` / `CAPWISE_NOTARY_API_ISSUER` | API 密钥 ID 和 Issuer ID |
| `CAPWISE_UPDATER_KEY` | minisign 私钥文件内容 |
| `CAPWISE_UPDATER_KEY_PASSWORD` | 私钥密码 |

这个 workflow 还没在 GitHub 上跑过，第一次用时要盯着日志。

## 出问题时

- `notarytool` 不通过：脚本会打印 `notarytool log`。常见原因是某个嵌套二进制没签名、没带安全时间戳或没开 hardened runtime。
- `stapler` 报错 65：刚公证完偶尔查不到票据，脚本会自动重试 5 次。
- "the update signature does not match"：私钥和提交的 `packaging/updater.pub` 不是一对，不要发布。
- 已安装的 App 提示签名校验失败：`latest.json` 里的签名不是用正式私钥签的（比如误传了试跑产物），重新生成后替换 Release 里的文件。
- 发错了版本：把那个 Release 改回草稿或删掉，`latest/download` 会回到上一个已发布的版本。已经升级的用户不会被降级。
