# 发布灵录（Capwise）

灵录通过 GitHub Releases 分发：用户下载 DMG 安装，之后由 App 内置的更新器自动升级。

- 更新清单：`https://github.com/joeseesun/capwise/releases/latest/download/latest.json`（总是指向最新一个**已发布**的 Release，草稿和预发布不算）。
- 更新包：`Capwise_<版本>_<架构>.app.tar.gz`，用 minisign 签名，签名放在 `latest.json` 里。App 只接受 `packaging/updater.pub` 这把公钥签过的包。
- App 启动 10 秒后检查一次，之后每 6 小时检查一次。正在录制、导出、导入或转写时不会弹窗，等任务结束再提示；"更新下载"设为"从不"时只响应菜单里的手动检查。只有一个稳定通道，没有 nightly。

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

4. **检查 DMG**：打开 DMG 看窗口布局，把 App 拖到"应用程序"后启动一次；`spctl -a -vv /Applications/Capwise.app` 应显示 `Notarized Developer ID`。

5. **提交版本号**并打 tag：

   ```bash
   git commit -am "chore: release 0.2.0"
   git tag v0.2.0 && git push origin main v0.2.0
   ```

6. **发布 Release**（脚本最后会打印同样的命令）：

   ```bash
   gh release create v0.2.0 --repo joeseesun/capwise --title "灵录 Capwise 0.2.0" \
     --notes-file apps/desktop-gpui/release-notes/0.2.0.md \
     apps/desktop-gpui/dist/0.2.0/Capwise_0.2.0_aarch64.dmg \
     apps/desktop-gpui/dist/0.2.0/Capwise_0.2.0_aarch64.app.tar.gz \
     apps/desktop-gpui/dist/0.2.0/latest.json \
     apps/desktop-gpui/dist/0.2.0/SHA256SUMS.txt
   ```

   想先检查可以加 `--draft`，确认后在网页上发布；**只有发布后** `releases/latest/download/latest.json` 才会指向它，已安装的 App 才会看到更新。

7. **从外部验证**：

   ```bash
   curl -sL https://github.com/joeseesun/capwise/releases/latest/download/latest.json
   ```

   版本号正确、`url` 能下载。再用一台装着旧版的机器在菜单里"检查更新"，走完下载、重启。

### Intel 版

脚本只构建本机架构。要同时提供 Intel 版，在 Intel Mac 上对同一版本再跑一次，并先把 Apple Silicon 那次的 `dist/<版本>/latest.json` 拷过去：脚本会把 `darwin-x86_64` 合并进同一个清单，然后上传两个架构的文件和合并后的 `latest.json`。

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
