# abox

临时文本 / 文件分享服务（pastebin 类小工具）。

## 特性

- 📝 按内容哈希生成链接，支持富文本 / 纯文本 / JSON
- 📎 支持拖放任意文件上传分享
- 💾 无数据库，内容直接落盘到 `data/`

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

```bash
docker run -d --name abox \
  -p 9430:3000 \
  -v ./data:/data \
  --restart unless-stopped \
  ghcr.io/wangzexi/abox:latest
```

`data/` 为数据目录（用户上传内容），请持久化挂载。
