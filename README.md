# Meting + Rust 歌单播放器/下载器

## 架构
- 本地 Meting 服务（Node + @meting/core）：`http://127.0.0.1:3001/api`
- Rust 后端：`http://127.0.0.1:8080`
- 前端页面：由 Rust 后端静态托管

## 启动步骤
1. 启动本地 Meting

```bash
cd meting-local
"C:\Program Files\nodejs\node.exe" server.mjs
```

2. 启动 Rust 后端

```bash
cargo run
```

3. 打开页面

- `http://127.0.0.1:8080`

## 环境变量（可选）
- `METING_API_BASE`：默认 `http://127.0.0.1:3001/api`
- `METING_SERVER`：默认 `netease`

## API
- `GET /api/playlists/:playlist_id?server=netease|tencent|kugou`
- `GET /api/search?server=...&keyword=...&limit=...`
- `GET /api/lrc?lrc=<lrc_url>`
- `POST /api/download-batch`
