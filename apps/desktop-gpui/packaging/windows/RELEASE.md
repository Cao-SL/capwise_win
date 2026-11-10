# Windows 发版

一条命令产出全部待上传产物：

```powershell
./scripts/publish-windows-release.ps1 `
    -Installer ./candidate/Capwise-Windows-x64-Setup.exe `
    -InstallerUrl https://github.com/Cao-SL/capwise_win/releases/download/v0.3.9/Capwise-Windows-x64-Setup.exe `
    -Version 0.3.9 `
    -MinisignSecretKey J:/Projects/ScreenVideo/capwise-win-update.key `
    -PfxBase64 $env:WINDOWS_SIGNING_PFX
```

产物落在 `dist/<version>/`：`latest.json`、`SHA256SUMS.txt`、
`capwise-release.json`、`PUBLISH-CHECKLIST.txt`、`winget/*.yaml`。
脚本最后会扫描所有生成文件，发现 `REPLACE_WITH_` / `example.com`
残留就中止。

## 更新签名

`installer.rs` 用 `minisign_verify` 校验更新包，公钥由
`include_str!("../packaging/updater.pub")` 编译进二进制。签名格式是
**整个 `.minisig` 文本文件的 base64**（`decode_base64_text` →
`Signature::decode`），不是把 `.minisig` 内容直接当 base64 透传。

`scripts/minisign-keytool.py` 负责密钥与签名（依赖
`pip install py-minisign cryptography`）：

```bash
python scripts/minisign-keytool.py keygen --public <path> --secret <path>
python scripts/minisign-keytool.py sign --secret <path> --file <installer>
python scripts/minisign-keytool.py verify --public <path> --file <installer>
```

`publish-windows-release.ps1 -MinisignSecretKey` 会自动签名并用
`packaging/updater.pub` 回验；也可以用 `-SignatureFile` 传外部
`minisign -S` 产出的 `.minisig`。

**私钥绝不进仓库**。`.gitignore` 已忽略 `*.key` / `*.pfx` /
`*.minisig`。本 fork 的私钥保存在仓库外的
`J:\Projects\ScreenVideo\capwise-win-update.key`；公钥已写入
`packaging/updater.pub` 和 `capwise-release.example.json`。
换密钥时三处要同步改，已发布版本只认旧公钥，新签名会被判为不匹配。

## 代码签名（生产）

`sign-windows-release.ps1` 支持三种密钥源，按证书形态二选一：

| 参数 | 适用形态 | 场景 |
|---|---|---|
| `-PfxBase64`（+`-PfxPassword`） | 可导出 PFX | CI secret、云签名导出 |
| `-CertificateThumbprint` | 证书已在用户/机器存储 | **推荐**：USB eToken / 云 HSM 驱动安装的正式证书，确定性匹配 |
| `-CertificateSubject` | 按 Subject 匹配 | 由 CA 官方安装器装好的证书 |

正式证书的三条路线（2023 年起 CA/B 论坛要求代码签名私钥必须存硬件，
经典「拿一个 PFX 文件」的年代已结束）：

1. **个人 / 开源**：Certum 的个人代码签名证书（SimplySign 云 HSM，
   个人身份即可申请，年费几十欧级别）。装上其驱动后 signtool 直接可用，
   用 `-CertificateThumbprint` 或 `-CertificateSubject` 签名。
2. **微软云签名**：Azure Trusted Signing（按月订阅）。不需要硬件，
   signtool 有原生集成，CI 友好；需要能访问国际版 Azure 并通过身份验证。
3. **企业 OV / EV**：Sectigo / DigiCert / 国内 CFCA、亚数等代理，
   需要营业执照，证书装在 USB eToken 上（附 SafeNet 中间件）。
   插上 token 后用 `-CertificateThumbprint` 或 `-CertificateSubject` 本机签名。

SmartScreen 提示与证书等级无关地取决于下载量信誉，新证书初期都会有
提示，随安装量累积消失。

拿到证书后：

```powershell
# 找到证书指纹
Get-ChildItem Cert:\CurrentUser\My | Where-Object { $_.HasPrivateKey }

# 本机重签三个 exe 与 MSIX（证书装好即可，无需 PFX）
./scripts/sign-windows-release.ps1 -Paths "dist/Capwise-0.3.10/Capwise.exe,dist/Capwise-0.3.10/cap-cli.exe,dist/Capwise-0.3.10/cap-muxer.exe" `
    -CertificateThumbprint <指纹>
./apps/desktop-gpui/scripts/build-capwise-msix.ps1 -Payload dist/Capwise-0.3.10 `
    -Version 0.3.10 -Publisher "CN=你的证书Subject" -CertificateThumbprint <指纹>
```

**MSIX 的 `Identity.Publisher` 必须与证书 Subject 完全一致**，
替换后用 `-Publisher` 传入重新打包。签名完成后把 Release 资产替换
（`gh release upload v0.3.10 --clobber` 或 API）。

## 更新端点

`installer.rs::UPDATE_MANIFEST_URL` 指向
`https://github.com/Cao-SL/capwise_win/releases/latest/download/latest.json`，
`RELEASES_PAGE_URL` 指向本 fork 的 Releases 页。也就是说 `latest.json`
要作为 **latest Release 的 asset** 上传，而不是某个带版本 tag 的
asset。`capwise-release.example.json` 的 `plugins.updater.endpoints`
与之保持一致。

Tauri 的 `plugins.updater.endpoints` 和 `installer.rs` 的原生更新器是
两条独立通道：前者是 Tauri 插件（Windows 构建里通过 `--config` 覆盖），
后者是 app 自己 `fetch_release()` 读的 `latest.json`。两者版本号要一致。

## 其他渠道

| 渠道 | 脚本 |
|---|---|
| NSIS | CI `capwise-platform-build.yml` |
| winget | `packaging/winget/` 模板 + publish 脚本生成 |
| MSIX | `packaging/windows/msix/README.md` |

## 完整验证记录

- `build-windows-update-manifest.ps1`：产出无 BOM 的 `latest.json`，
  `signature` 字段等于 `base64(.minisig)`，可用 `minisign_verify` 解码
- `installer.rs` 单测 `a_minisign_keytool_signature_verifies_against_the_shipped_key`
  用真实 `minisign_verify` 验证固件，并断言篡改与错误公钥都被拒
- publish 端到端：自动签名 → 回验 → manifest → winget → 占位符防呆，
  6 个产物全部干净
