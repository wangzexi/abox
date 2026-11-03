# Abox — share text, images, HTML and files

一个轻量便签、临时剪贴板和文件床。打开页面输入或粘贴内容，也可以直接拖入文件。

## 浏览器使用

- **保存**：自动识别 HTML、JSON 和文本。
- **保存纯文本**：原样保存输入的文字。
- **拖放文件**：直接上传，显示进度。

快捷键为 `Ctrl/Cmd + S` 和 `Ctrl/Cmd + Alt + S`。
保存后原地预览，左下角悬浮胶囊提供复制链接和删除操作。
地址栏与复制的链接都指向原文件；刷新或分享后直接打开内容。

单文件最大 256 MiB。文件默认在 7 天后过期，访问不会改变到期时间。上传后可在胶囊中调整期限，最长 365 天。

## API

以下示例使用 `https://abox.example.com` 作为你的部署地址。获取操作示例：

```bash
curl https://abox.example.com/
```

上传：

```bash
curl https://abox.example.com/ \
  -H 'Content-Type: image/png' \
  -H 'Content-Disposition: attachment; filename="photo.png"' \
  --data-binary @photo.png
```

响应：

```json
{"url":"https://abox.example.com/abc123","key":"YOUR_KEY","expires_at":"2026-11-10T12:00:00Z"}
```

分享 `url`，保管 `key`。文件名随下载保留。

修改同一个链接的内容：

```bash
curl -X PUT https://abox.example.com/abc123 \
  -H 'Authorization: Bearer YOUR_KEY' \
  -H 'Content-Type: text/plain; charset=utf-8' \
  --data-binary 'updated text'
```

续期（从当前时间起重新计算，最长 365 天）：

```bash
curl -X POST https://abox.example.com/abc123/renew \
  -H 'Authorization: Bearer YOUR_KEY' \
  -H 'Content-Type: application/json' \
  --data '{"expires_in_seconds":2592000}'
```

删除：

```bash
curl -X DELETE https://abox.example.com/abc123 \
  -H 'Authorization: Bearer YOUR_KEY'
```

## 本地运行

```bash
ABOX_LISTEN=127.0.0.1:3000 \
ABOX_PUBLIC_URL=http://127.0.0.1:3000 \
cargo run --release
```

## 容器运行

```bash
docker run -d --name abox \
  -p 3000:3000 \
  -e ABOX_PUBLIC_URL=http://localhost:3000 \
  -v "$(pwd)/data:/data" \
  --restart unless-stopped \
  ghcr.io/wangzexi/abox:latest
```

## 配置

| 环境变量 | 默认值 |
|---|---|
| `ABOX_LISTEN` | `0.0.0.0:3000` |
| `ABOX_PUBLIC_URL` | `http://localhost:3000`（部署时设置为自己的公开地址） |
| `ABOX_DATA_DIR` | `data` |
| `ABOX_CAPACITY_MIB` | `640` |

数据目录包含文件内容与 JSON 元数据，备份和恢复时保留整个目录。服务由单进程独占数据目录。
全部文件共享容量限制。容量预算之外，为上传暂存预留至少 256 MiB，并为元数据保留额外空间。
未过期文件不会被自动挤掉；空间不足时，上传返回 507。旧文件首次升级时至少保留 7 天；原有保护期更长时继续保留至原保护期结束。

上传、下载采用流式处理。一个上传进行时，其他上传返回 429 和 `Retry-After`。
HTML 预览使用隔离 iframe，支持脚本、表单和页面交互。管理密钥在创建时返回，服务端保存其哈希。

## 开发与验证

```bash
cargo fmt --check
cargo test --locked
cargo build --locked
python3 tests/http_smoke.py
```

测试覆盖统一容量、过期与续期、原始文件名、管理密钥、修改与删除、重启恢复，以及实际 256 MiB 上传和下载。
GitHub Actions 在检查通过后构建镜像，提供完整源码 commit SHA 标签。
