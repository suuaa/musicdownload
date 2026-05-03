import http from 'node:http';
import { URL } from 'node:url';
import { readFileSync, existsSync } from 'node:fs';
import Meting from '@meting/core';
import NCM from 'NeteaseCloudMusicApi';
import { chromium } from 'playwright';
const {
  login_qr_key,
  login_qr_create,
  login_qr_check,
  song_url_v1,
  song_download_url_v1
} = NCM;

const HOST = process.env.METING_HOST || '127.0.0.1';
const PORT = Number(process.env.METING_PORT || 3001);
const cookieStore = new Map();
let qqLoginSession = null;

function sendJson(res, statusCode, payload) {
  res.statusCode = statusCode;
  res.setHeader('Content-Type', 'application/json; charset=utf-8');
  res.setHeader('Access-Control-Allow-Origin', '*');
  res.end(JSON.stringify(payload));
}

function pickServer(server) {
  const allow = new Set(['netease', 'tencent', 'kugou', 'baidu', 'kuwo']);
  return allow.has(server) ? server : 'netease';
}

function normalizeBitrate(server, br) {
  let n = Number(br || 320);
  if (!Number.isFinite(n) || n <= 0) n = 320;
  if (n >= 1000) n = Math.round(n / 1000);
  // Keep lossless/high-tier requests instead of forcing downgrade to 320.
  // For this project, `999` is used as SQ/lossless sentinel.
  if (server === 'netease') {
    if (n >= 999) return 999;
    return [128, 192, 320].includes(n) ? n : 320;
  }
  if (server === 'tencent') {
    if (n >= 999) return 999;
    return [128, 192, 320].includes(n) ? n : 320;
  }
  if (server === 'kugou') return [128, 320].includes(n) ? n : 320;
  return n;
}

function buildCookieString(cookies) {
  return cookies.map((c) => `${c.name}=${c.value}`).join('; ');
}

function mapTencentSong(s) {
  const singers = Array.isArray(s?.singer) ? s.singer : [];
  const artist = singers.map((x) => x?.name || x?.title || '').filter(Boolean);
  const albumObj = s?.album || {};
  const album = s?.albumname || albumObj?.name || albumObj?.title || '';
  const songMid = s?.songmid || s?.mid || '';
  const albumMid = s?.albummid || albumObj?.mid || '';
  return {
    id: String(s?.songid || s?.id || songMid || ''),
    name: s?.songname || s?.name || s?.title || '',
    artist,
    album,
    pic_id: albumMid,
    url_id: songMid,
    lyric_id: songMid,
    source: 'tencent'
  };
}

async function closeQqSession() {
  if (!qqLoginSession) return;
  try {
    await qqLoginSession.context.close();
  } catch {}
  qqLoginSession = null;
}

async function startQqBrowserLogin() {
  await closeQqSession();
  const userDataDir = 'E:/musicdownload/meting-local/.qq-login-profile';
  const context = await chromium.launchPersistentContext(userDataDir, {
    headless: false,
    channel: 'msedge'
  });
  const page = context.pages()[0] || await context.newPage();
  await page.goto('https://y.qq.com/', { waitUntil: 'domcontentloaded' });
  qqLoginSession = {
    id: `${Date.now()}-${Math.random().toString(36).slice(2, 8)}`,
    context,
    createdAt: Date.now()
  };
  return qqLoginSession.id;
}

async function checkQqBrowserLogin(sessionId) {
  if (!qqLoginSession || qqLoginSession.id !== sessionId) {
    return { state: 'expired', message: 'session not found' };
  }
  const cookies = await qqLoginSession.context.cookies(['https://y.qq.com', 'https://qq.com']);
  const cookieStr = buildCookieString(cookies);
  const hasUin = cookies.some((c) => c.name === 'uin' && c.value);
  const hasQqmusicKey = cookies.some((c) => c.name.toLowerCase().includes('qqmusic') || c.name === 'qm_keyst');
  if (hasUin || hasQqmusicKey) {
    cookieStore.set('tencent', cookieStr);
    return { state: 'success', message: 'cookie saved', cookie_saved: true };
  }
  return { state: 'waiting', message: 'waiting for login confirm', cookie_saved: false };
}

async function tencentSearchWithBrowser(keyword, limit = 20) {
  if (!qqLoginSession) return [];
  const page = qqLoginSession.context.pages()[0] || await qqLoginSession.context.newPage();
  const list = await page.evaluate(async ({ keyword, limit }) => {
    const payload = {
      comm: { ct: 24, cv: 0 },
      req_0: {
        method: 'DoSearchForQQMusicDesktop',
        module: 'music.search.SearchCgiService',
        param: { num_per_page: limit, page_num: 1, query: keyword, search_type: 0 }
      }
    };
    const res = await fetch('https://u.y.qq.com/cgi-bin/musicu.fcg', {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify(payload)
    });
    const json = await res.json();
    return json?.req_0?.data?.body?.song?.list || [];
  }, { keyword, limit });

  return list.map((s) => mapTencentSong(s));
}

async function tencentSearchFallback(keyword, limit = 20) {
  const payload = {
    comm: { ct: 19, cv: 1859 },
    req: {
      method: 'DoSearchForQQMusicDesktop',
      module: 'music.search.SearchCgiService',
      param: { query: keyword, num_per_page: limit, page_num: 1, search_type: 0 }
    }
  };
  const resp = await fetch('https://u.y.qq.com/cgi-bin/musicu.fcg', {
    method: 'POST',
    headers: {
      'content-type': 'application/json',
      Referer: 'https://y.qq.com/',
      'User-Agent': 'Mozilla/5.0'
    },
    body: JSON.stringify(payload)
  });
  if (!resp.ok) {
    throw new Error(`tencent search fallback http ${resp.status}`);
  }
  const json = await resp.json();
  const list = json?.req?.data?.body?.song?.list || [];
  return list.map((s) => mapTencentSong(s));
}

async function tencentPlaylistFallback(disstid, bitrate = 320) {
  const url = `https://c.y.qq.com/qzone/fcg-bin/fcg_ucc_getcdinfo_byids_cp.fcg?type=1&json=1&utf8=1&onlysong=0&disstid=${encodeURIComponent(disstid)}&format=json`;
  const resp = await fetch(url, {
    headers: {
      Referer: 'https://y.qq.com/',
      'User-Agent': 'Mozilla/5.0'
    }
  });
  if (!resp.ok) {
    throw new Error(`tencent playlist fallback http ${resp.status}`);
  }
  const data = await resp.json();
  const songlist = data?.cdlist?.[0]?.songlist || [];
  return songlist.map((s) => mapTencentSong(s));
}

async function runMeting(server, type, id, bitrate, limit) {
  const realBitrate = normalizeBitrate(server, bitrate);
  const meting = new Meting(server);
  const cookie = cookieStore.get(server);
  if (cookie) meting.cookie(cookie);
  meting.format(true);

  if (type === 'playlist') {
    if (server === 'tencent') {
      try {
        const base = JSON.parse(await meting.playlist(id));
        if (Array.isArray(base) && base.length > 0) return enrichTracks(base, server, realBitrate);
      } catch {}
      const fb = await tencentPlaylistFallback(id, realBitrate);
      return enrichTracks(fb, server, realBitrate);
    }
    return enrichTracks(JSON.parse(await meting.playlist(id)), server, realBitrate);
  }
  if (type === 'search') {
    if (server === 'tencent') {
      try {
        const base = JSON.parse(await meting.search(id, { page: 1, limit }));
        if (Array.isArray(base) && base.length > 0) return enrichTracks(base, server, realBitrate);
      } catch {}
      try {
        const fbHttp = await tencentSearchFallback(id, limit);
        if (Array.isArray(fbHttp) && fbHttp.length > 0) return enrichTracks(fbHttp, server, realBitrate);
      } catch {}
      const fb = await tencentSearchWithBrowser(id, limit);
      if (Array.isArray(fb) && fb.length > 0) return enrichTracks(fb, server, realBitrate);
      return [];
    }
    return enrichTracks(JSON.parse(await meting.search(id, { page: 1, limit })), server, realBitrate);
  }
  if (type === 'song') return enrichTracks(JSON.parse(await meting.song(id)), server, realBitrate);
  if (type === 'url') {
    if (server === 'netease') {
      const resolved = await resolveNeteasePlayableUrl(id, realBitrate, cookie);
      if (resolved?.url) return resolved;
    }
    return JSON.parse(await meting.url(id, realBitrate));
  }
  if (type === 'lrc') return JSON.parse(await meting.lyric(id));
  if (type === 'pic') return JSON.parse(await meting.pic(id, 500));

  throw new Error(`unsupported type: ${type}`);
}

function neteaseLevelsForBitrate(bitrate) {
  if (bitrate >= 999) return ['jymaster', 'dolby', 'sky', 'jyeffect', 'hires', 'lossless', 'exhigh', 'higher', 'standard'];
  if (bitrate >= 320) return ['exhigh', 'higher', 'standard'];
  if (bitrate >= 192) return ['higher', 'standard'];
  return ['standard'];
}

async function resolveNeteasePlayableUrl(id, bitrate, cookie) {
  const levels = neteaseLevelsForBitrate(bitrate);
  const cookieWithOs = cookie && cookie.includes('os=') ? cookie : `${cookie ? `${cookie}; ` : ''}os=pc`;
  for (const level of levels) {
    try {
      if (typeof song_download_url_v1 === 'function') {
        const d = await song_download_url_v1({ id, level, cookie: cookieWithOs });
        const item = d?.body?.data || d?.data;
        const url = item?.url || item?.downloadUrl;
        if (url) return { url, br: item?.br || 0, type: item?.type || '', level, source: 'ncm_download_url_v1' };
      }
    } catch {}
    try {
      if (typeof song_url_v1 === 'function') {
        const r = await song_url_v1({ id, level, cookie: cookieWithOs });
        const item = Array.isArray(r?.body?.data) ? r.body.data[0] : (Array.isArray(r?.data) ? r.data[0] : null);
        const url = item?.url;
        if (url) return { url, br: item?.br || 0, type: item?.type || '', level, source: 'ncm_song_url_v1' };
      }
    } catch {}
  }
  return null;
}

function enrichTracks(data, server, bitrate = 320) {
  const base = `http://${HOST}:${PORT}/api?server=${encodeURIComponent(server)}`;
  const streamBase = `http://${HOST}:${PORT}/stream?server=${encodeURIComponent(server)}`;
  const list = Array.isArray(data) ? data : [data];

  return list.map((item) => {
    const artist = Array.isArray(item.artist) ? item.artist.join(' / ') : (item.artist || '');
    const out = { ...item };
    out.artist = artist;

    if (!out.url && out.url_id) out.url = `${streamBase}&id=${encodeURIComponent(String(out.url_id))}&br=${encodeURIComponent(String(bitrate))}`;
    if (!out.lrc && out.lyric_id) out.lrc = `${base}&type=lrc&id=${encodeURIComponent(String(out.lyric_id))}`;
    if (out.pic_id) out.pic = `${base}&type=pic&id=${encodeURIComponent(String(out.pic_id))}`;
    return out;
  });
}

const server = http.createServer(async (req, res) => {
  if (req.method === 'OPTIONS') {
    res.statusCode = 204;
    res.setHeader('Access-Control-Allow-Origin', '*');
    res.setHeader('Access-Control-Allow-Methods', 'GET,POST,OPTIONS');
    res.setHeader('Access-Control-Allow-Headers', 'Content-Type');
    res.end();
    return;
  }

  try {
    const url = new URL(req.url || '/', `http://${HOST}:${PORT}`);

    if (url.pathname === '/admin/cookies' && req.method === 'GET') {
      const out = {};
      for (const [k, v] of cookieStore.entries()) out[k] = v;
      sendJson(res, 200, out);
      return;
    }

    if (url.pathname === '/admin/cookie' && req.method === 'POST') {
      const body = await readJsonBody(req);
      const serverName = pickServer(String(body.server || 'netease'));
      const cookie = String(body.cookie || '');
      cookieStore.set(serverName, cookie);
      sendJson(res, 200, { ok: true, server: serverName });
      return;
    }

    if (url.pathname === '/admin/netease/qr/create' && req.method === 'POST') {
      const keyResp = await login_qr_key({});
      const key = keyResp.body?.data?.unikey;
      if (!key) return sendJson(res, 502, { error: 'failed to fetch unikey' });
      const qrResp = await login_qr_create({ key, qrimg: true });
      const qrimg = qrResp.body?.data?.qrimg || '';
      sendJson(res, 200, { key, qrimg });
      return;
    }

    if (url.pathname === '/admin/netease/qr/check' && req.method === 'GET') {
      const key = (url.searchParams.get('key') || '').trim();
      if (!key) return sendJson(res, 400, { error: 'key is required' });
      const checkResp = await login_qr_check({ key, noCookie: true });
      const body = checkResp.body || {};
      if (body.code === 803 && checkResp.cookie) {
        cookieStore.set('netease', checkResp.cookie);
      }
      sendJson(res, 200, { code: body.code, message: body.message || '', cookie_saved: body.code === 803 });
      return;
    }

    if (url.pathname === '/admin/tencent/browser-login/start' && req.method === 'POST') {
      const sessionId = await startQqBrowserLogin();
      sendJson(res, 200, {
        session_id: sessionId,
        message: 'Edge 已打开，请在 QQ 音乐页面扫码并完成登录'
      });
      return;
    }

    if (url.pathname === '/admin/tencent/browser-login/status' && req.method === 'GET') {
      const sessionId = (url.searchParams.get('session_id') || '').trim();
      if (!sessionId) return sendJson(res, 400, { error: 'session_id is required' });
      const data = await checkQqBrowserLogin(sessionId);
      sendJson(res, 200, data);
      return;
    }

    if (url.pathname === '/admin/tencent/browser-login/stop' && req.method === 'POST') {
      await closeQqSession();
      sendJson(res, 200, { ok: true });
      return;
    }

    if (url.pathname === '/stream' && req.method === 'GET') {
      const serverName = pickServer(url.searchParams.get('server') || 'netease');
      const id = (url.searchParams.get('id') || '').trim();
      if (!id) return sendJson(res, 400, { error: 'id is required' });
      const br = Number(url.searchParams.get('br') || 320);
      const data = await runMeting(serverName, 'url', id, br, 1);
      const realUrl = data?.url;
      if (!realUrl) return sendJson(res, 404, { error: 'stream url not found' });
      res.statusCode = 302;
      res.setHeader('Location', realUrl);
      res.end();
      return;
    }

    if (url.pathname !== '/api') return sendJson(res, 404, { error: 'not found' });

    const serverName = pickServer(url.searchParams.get('server') || 'netease');
    const type = (url.searchParams.get('type') || '').trim();
    const id = (url.searchParams.get('id') || url.searchParams.get('keyword') || '').trim();
    const bitrate = Number(url.searchParams.get('br') || 320);
    const limit = Number(url.searchParams.get('limit') || 20);

    if (!type) return sendJson(res, 400, { error: 'type is required' });
    if (!id) return sendJson(res, 400, { error: 'id/keyword is required' });

    if (type === 'pic') {
      const picData = await runMeting(serverName, type, id, bitrate, limit);
      const picUrl = typeof picData === 'string' ? picData : (picData && (picData.url || picData.pic));
      if (!picUrl) return sendJson(res, 404, { error: 'pic url not found' });
      res.statusCode = 302;
      res.setHeader('Location', picUrl);
      res.setHeader('Access-Control-Allow-Origin', '*');
      res.end();
      return;
    }

    const data = await runMeting(serverName, type, id, bitrate, limit);
    sendJson(res, 200, data);
  } catch (err) {
    sendJson(res, 502, { error: String(err?.message || err) });
  }
});

server.listen(PORT, HOST, () => {
  const cookieFile = 'E:/musicdownload/meting-local/cookie_netmusic.txt';
  if (existsSync(cookieFile)) {
    try {
      const cookie = readFileSync(cookieFile, 'utf8').trim();
      if (cookie) cookieStore.set('netease', cookie);
    } catch {}
  }
  console.log(`Local Meting API running at http://${HOST}:${PORT}/api`);
});

function readJsonBody(req) {
  return new Promise((resolve, reject) => {
    let raw = '';
    req.on('data', (chunk) => {
      raw += chunk.toString('utf8');
      if (raw.length > 2 * 1024 * 1024) reject(new Error('body too large'));
    });
    req.on('end', () => {
      if (!raw.trim()) return resolve({});
      try { resolve(JSON.parse(raw)); } catch (err) { reject(err); }
    });
    req.on('error', reject);
  });
}






