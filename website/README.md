# 灵录 Capwise 官网

静态官网，源文件在本目录。生产域名 `capwise.qiaomu.ai` 直连洛杉矶 VPS，由 Nginx 提供静态文件和 HTTPS。

## 构建与部署

```bash
cd website
npm run build
```

VPS 使用 `qiaomu-la` SSH 别名，发布目录为 `/var/www/capwise/releases/`，Nginx 通过 `/var/www/capwise/current` 提供当前版本。站点配置见 `deploy/nginx-capwise.conf`，部署入口为 `scripts/deploy-vps.sh`。

`site.config.json` 只记录公开的 Umami 站点 ID。构建检查本地资源存在性、结构化数据和统计 ID，并生成缓存版本及 JSON-LD 的 CSP hash。`dist/` 为生成目录。

## 图片与演示工程

- 工作台截图来自真实安装版与公开演示工程；不发布私人录屏或账户配置。
- NOVA 是原创虚构演示内容，不代表真实第三方产品。`scripts/demo/create-demo.py` 可重新生成可编辑工程，需要 Pillow、FFmpeg、Noto Sans CJK SC 字体以及作为输入的原生 `.cap` 模板与自有音乐。
- 演示视频由应用自带 `cap-cli export` 导出，1080p、30 fps、24 秒。原生文字、字幕、缩放与配乐继续保存在 `.cap` 工程中。
- 截图只进行局部裁切、等比缩放和 WebP 压缩，没有重绘应用控件。
- 图标来自项目已有资源与 Lucide，许可见 `LUCIDE-LICENSE`。打赏和公众号二维码来自维护者提供的个人资料资源。

## 发布检查

检查桌面与手机排版、全部图库标签的键盘切换、放大图、打赏和关注弹窗、复制安装提示词、FAQ，以及浏览器控制台、PWA 和离线页。统计接入需从真实生产浏览器触发唯一事件，并在 Umami 对应站点中确认接收，不能只检查脚本加载。

下载始终使用官方 GitHub 的 `releases/latest/download/Capwise.dmg`。更新图片、演示文件时保留对应的验证记录。
