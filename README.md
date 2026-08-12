# abox

临时文本 / 文件分享服务（pastebin 类小工具）。

基于内容哈希寻址：内容不变，链接不变；无数据库，纯文件系统存储。

## 特性

- 📝 按内容哈希（SHA-256 前 10 位）生成链接，支持富文本 / 纯文本 / JSON
- 📎 支持拖放任意文件上传分享
- 💾 无数据库，内容直接落盘到 `data/`
- 🦀 基于 [Rust](https://www.rust-lang.org/)（axum），静态编译单二进制，镜像 ~5MB

## 使用

打开首页，输入内容后：

| 操作 | 效果 |
|---|---|
| `Ctrl/Cmd + S` | 保存富文本 / JSON |
| `Ctrl/Cmd + Alt + S` | 保存纯文本 |
| 拖放文件 | 上传文件分享 |

保存后会跳转到 `/{hash}` 链接，可直接分享给他人。

## 本地运行

```bash
cargo run --release   # 监听 3000 端口
```

## 部署

镜像由 GitHub Actions 自动构建并推送到 [ghcr.io](https://github.com/wangzexi/abox/pkgs/container/abox)：

```yaml
services:
  abox:
    image: ghcr.io/wangzexi/abox:latest
    restart: unless-stopped
    ports:
      - 9430:3000
    volumes:
      - ./data:/app/data
```

`data/` 为数据目录（用户上传内容），请持久化挂载。

## 技术说明

- 内容 ID 为内容本身的 SHA-256 哈希前 10 位，天然去重
- 重新上传相同内容会覆盖旧文件（幂等）
- 不支持的内容类型会自动以附件形式下载（`Content-Disposition: attachment`）
- 前端（`src/index.html`）编译时内嵌进二进制，单文件部署
