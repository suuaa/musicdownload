# Meting + Rust 歌单播放器/下载器

## 依赖与引用
- 本项目采用并引用了 [Meting](https://github.com/metowolf/Meting) 作为核心音乐聚合能力来源。
- 仓库内以 Git 子模块方式引入：`third_party/Meting`

## 免责声明

本站仅供学习交流使用，所有音乐数据均来自第三方平台，不在本服务器存储任何音频文件。
请在获取后 24 小时内删除，切勿用于商业或违法用途。

使用本站即表示您已知晓并同意：本人不对因使用本站产生的任何后果承担责任，包括但不限于版权纠纷、法律责任等。
请遵守当地法律法规及音乐平台的用户协议。

如有异议请联系：2426159506@qq.com

疑问及使用咨询：Telegram@yus710

请使用最新的 Chromium 内核浏览器以获得最佳体验。
（如 Chrome/Edge，iOS 建议使用 Edge 浏览器）
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

