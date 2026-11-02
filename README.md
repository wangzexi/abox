# abox

极简文本、图片和文件分享。首页是可编辑区域，`Ctrl/Cmd + S` 保存，
`Ctrl/Cmd + Alt + S` 保存纯文本；支持拖入文件，并显示上传进度。
保存后在当前页面用隔离 iframe 预览，左下角显示文件信息与分享胶囊。
地址栏及复制内容都是真实文件短链；刷新或分享后直接打开原文件，不显示胶囊。

## API

`GET /` 直接提供完整 API 说明与 curl 示例：普通 curl 返回纯文本说明；
请求 `Accept: text/html`（如浏览器）时返回编辑页。HTML 中也内嵌同一份说明，浏览器中不显示。
`POST /` 接收原始文件字节，最大 **256 MiB**，返回 `201` JSON，不跳转。

```bash
curl --fail-with-body https://abox.example.com/ \
  -H 'Content-Type: image/png' \
  -H 'Content-Disposition: attachment; filename="photo.png"' \
  --data-binary @image.png
```

创建返回 `{"url":"https://abox.example.com/abc123","key":"…"}`。
`GET /{id}` 自由读取；`PUT /{id}` 更新内容；`DELETE /{id}` 删除。
修改和删除通过 `Authorization: Bearer <key>` 验证，修改保持原链接。
密钥在创建时返回，服务端持久化其 SHA-256 哈希。
更新返回 `200` 和 `{"url":"…"}`，删除返回 `204`。
原始文件名用于下载，Unicode 名称通过 `filename*=UTF-8''` 编码传入。
`POST /save` 保留给旧客户端；未要求 JSON 时仍返回原文件的 303 跳转。

`GET /{id}` 返回原文件；`GET /{id}?download=1` 下载；
知道链接的人可读取，请勿当作私密存储。
上传的 HTML 使用 sandbox，允许脚本与表单，不具有 Abox 应用的同源权限。
需要同源权限的网页功能（例如 localStorage）可能无法在预览中使用。

## 两个独立 LRU

- 小文件桶：**≤100 MiB**，默认总容量 **256 MiB**。
- 大文件桶：**>100 MiB**，默认总容量 **384 MiB**。
- 文件读取及预览页访问更新最近使用时间。一个桶满时，只淘汰该桶里最久未访问、且不在保护期的文件。
- 新文件保护 30 天。访问更新 LRU，保护期由上传时间确定。
- 保护期结束不会立即删除，只会变成可被 LRU 淘汰的文件。桶内空间均受保护时，新上传返回 507。
- 已被淘汰的链接返回 404。

上传和下载都采用流式处理。一次只接受一个上传，其他请求返回 429 和 Retry-After。
容量预算不包含暂存；默认总文件预算 640 MiB，另留最多 256 MiB 暂存及元数据空间，
适配现有 1 GiB 卷。大量小文件会产生额外文件系统和元数据开销，实际磁盘耗尽时返回 507。
请勿将桶容量之和配置到磁盘总容量，必须保留上传暂存空间。

## 存储与兼容

无需数据库。新上传的内容位于 `data/.objects/`，每个文件的 JSON 元数据位于 `data/.metadata/`。
启动时登记历史文件并建立内存索引；元数据通过同目录临时文件 + rename 原子替换。
修改先完整写入新版本，再切换元数据指针。旧版本及崩溃残留在完成或重启时清理。
未完成的上传不可见，中断后清理，崩溃残留在下次启动时清理。

已有文件在首次启动时登记元数据，获得新的 30 天保护期；原内容与短链接继续保留。 元数据损坏或重复 ID 会阻止启动，
避免错误地推断保护状态。应备份整个数据目录，不要只备份内容或元数据中的一部分。
仅支持单进程独占数据目录，Kubernetes 保持一个副本和 Recreate 策略。

## 本地运行

```bash
cargo run
# 默认监听 0.0.0.0:3000；本地测试应覆盖对外 URL，避免跳到生产站点
ABOX_LISTEN=127.0.0.1:9430 \
ABOX_PUBLIC_URL=http://127.0.0.1:9430 \
ABOX_DATA_DIR=data/local-preview cargo run
```

可配置环境变量：

| 变量 | 默认值 |
|---|---|
| `ABOX_LISTEN` | `0.0.0.0:3000` |
| `ABOX_PUBLIC_URL` | `https://abox.example.com` |
| `ABOX_DATA_DIR` | `data` |
| `ABOX_SMALL_CAPACITY_MIB` | `256` |
| `ABOX_LARGE_CAPACITY_MIB` | `384` |

## 验证

```bash
cargo test
cargo build
python3 tests/http_smoke.py
```

HTTP 检查使用临时目录和独立进程，覆盖真实 256 MiB 上传/下载、
分块请求超限、请求中断、密钥鉴权、修改与删除、重启恢复、JSON、兼容接口与原始文件名。

## 容器

```bash
docker run -d --name abox -p 9430:3000 \
  -v ./data:/data --restart unless-stopped ghcr.io/wangzexi/abox:latest
```

数据卷须持久化。容量配置和实际卷容量应同时调整。
