use std::io::Write;
use std::sync::Arc;
use std::time::Duration;
use std::{fs, path::PathBuf};

use anyhow::{anyhow, Context, Result};
use axum::body::Body;
use axum::extract::{Path, Query};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use bytes::Bytes;
use id3::frame::{Lyrics, Picture, PictureType};
use id3::{Tag as Id3Tag, TagLike, Version};
use metaflac::{block::PictureType as FlacPictureType, Tag as FlacTag};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tower_http::services::ServeDir;
use tracing::{error, info, warn};

#[derive(Clone)]
struct AppState {
    client: Client,
    meting_base: String,
    meting_admin_base: String,
    default_server: Arc<RwLock<String>>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct Track {
    id: String,
    name: String,
    artist: String,
    album: Option<String>,
    url: String,
    pic: Option<String>,
    lrc: Option<String>,
}

#[derive(Debug, Deserialize)]
struct BatchDownloadRequest {
    tracks: Vec<Track>,
}

#[derive(Debug, Deserialize)]
struct PlatformQuery {
    server: Option<String>,
    br: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct SearchQuery {
    server: Option<String>,
    keyword: String,
    limit: Option<usize>,
    br: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct LrcQuery {
    lrc: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct MetingSettings {
    meting_base: String,
    default_server: String,
    cookies: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct UpdateDefaultServerRequest {
    server: String,
}

#[derive(Debug, Deserialize)]
struct UpdateCookieRequest {
    server: String,
    cookie: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct QrCreateResponse {
    key: String,
    qrimg: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct QrCheckResponse {
    code: i32,
    message: String,
    cookie_saved: bool,
}

#[derive(Debug, Deserialize)]
struct QrCheckQuery {
    key: String,
}

#[derive(Debug, Deserialize)]
struct QqLoginStatusQuery {
    session_id: String,
}

#[derive(Debug, Deserialize)]
struct DownloadRangeQuery {
    url: String,
    start: u64,
    end: Option<u64>,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt().with_env_filter("info").init();

    let meting_base =
        std::env::var("METING_API_BASE").unwrap_or_else(|_| "http://127.0.0.1:3001/api".to_string());
    let meting_admin_base =
        std::env::var("METING_ADMIN_BASE").unwrap_or_else(|_| "http://127.0.0.1:3001/admin".to_string());
    let default_server = std::env::var("METING_SERVER").unwrap_or_else(|_| "netease".to_string());

    let state = Arc::new(AppState {
        client: Client::new(),
        meting_base,
        meting_admin_base,
        default_server: Arc::new(RwLock::new(default_server)),
    });

    let app = Router::new()
        .route("/api/playlists/:playlist_id", get(get_playlist))
        .route("/api/search", get(search_tracks))
        .route("/api/lrc", get(get_lrc_content))
        .route("/api/download-batch", post(download_batch))
        .route("/api/download-one", post(download_one))
        .route("/api/download-range", get(download_range))
        .route("/api/meting/settings", get(get_meting_settings))
        .route("/api/meting/default-server", post(set_default_server))
        .route("/api/meting/cookie", post(set_meting_cookie))
        .route("/api/meting/qr/create", post(create_netease_qr))
        .route("/api/meting/qr/check", get(check_netease_qr))
        .route("/api/meting/qq-login/start", post(start_qq_login))
        .route("/api/meting/qq-login/status", get(check_qq_login))
        .route("/api/meting/qq-login/stop", post(stop_qq_login))
        .nest_service("/", ServeDir::new("static"))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:8080").await.unwrap();
    info!("server started on http://127.0.0.1:8080");
    axum::serve(listener, app).await.unwrap();
}

async fn get_playlist(
    Path(playlist_id): Path<String>,
    Query(query): Query<PlatformQuery>,
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
) -> Response {
    let fallback_server = state.default_server.read().await.clone();
    let server = query.server.unwrap_or(fallback_server);
    let br = query.br.unwrap_or(320);
    match fetch_playlist(&state, &server, &playlist_id, br).await {
        Ok(tracks) => Json(tracks).into_response(),
        Err(err) => {
            error!("fetch playlist failed: {err:#}");
            (StatusCode::BAD_GATEWAY, format!("failed to fetch playlist: {err}")).into_response()
        }
    }
}

async fn search_tracks(
    Query(query): Query<SearchQuery>,
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
) -> Response {
    let fallback_server = state.default_server.read().await.clone();
    let server = query.server.unwrap_or(fallback_server);
    let limit = query.limit.unwrap_or(20).clamp(1, 50);

    let br = query.br.unwrap_or(320);
    match fetch_search(&state, &server, &query.keyword, limit, br).await {
        Ok(tracks) => Json(tracks).into_response(),
        Err(err) => {
            error!("search failed: {err:#}");
            (StatusCode::BAD_GATEWAY, format!("search failed: {err}")).into_response()
        }
    }
}

async fn get_lrc_content(
    Query(query): Query<LrcQuery>,
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
) -> Response {
    match fetch_lrc(&state.client, &query.lrc).await {
        Ok(content) => Json(serde_json::json!({ "content": content })).into_response(),
        Err(err) => {
            error!("fetch lrc failed: {err:#}");
            (StatusCode::BAD_GATEWAY, format!("fetch lrc failed: {err}")).into_response()
        }
    }
}

async fn get_meting_settings(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
) -> Response {
    let default_server = state.default_server.read().await.clone();
    let cookies_url = format!("{}/cookies", state.meting_admin_base);
    let cookies = match state.client.get(&cookies_url).send().await {
        Ok(resp) => match resp.json::<serde_json::Value>().await {
            Ok(v) => v,
            Err(_) => serde_json::json!({}),
        },
        Err(_) => serde_json::json!({}),
    };

    Json(MetingSettings {
        meting_base: state.meting_base.clone(),
        default_server,
        cookies,
    })
    .into_response()
}

async fn set_default_server(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
    Json(payload): Json<UpdateDefaultServerRequest>,
) -> Response {
    if payload.server.trim().is_empty() {
        return (StatusCode::BAD_REQUEST, "server cannot be empty").into_response();
    }
    *state.default_server.write().await = payload.server.clone();
    Json(serde_json::json!({ "ok": true, "default_server": payload.server })).into_response()
}

async fn set_meting_cookie(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
    Json(payload): Json<UpdateCookieRequest>,
) -> Response {
    let url = format!("{}/cookie", state.meting_admin_base);
    let result = state
        .client
        .post(&url)
        .json(&serde_json::json!({
            "server": payload.server,
            "cookie": payload.cookie
        }))
        .send()
        .await;

    match result {
        Ok(resp) if resp.status().is_success() => Json(serde_json::json!({ "ok": true })).into_response(),
        Ok(resp) => (
            StatusCode::BAD_GATEWAY,
            format!("meting cookie update failed: {}", resp.status()),
        )
            .into_response(),
        Err(err) => (
            StatusCode::BAD_GATEWAY,
            format!("meting cookie update request failed: {err}"),
        )
            .into_response(),
    }
}

async fn create_netease_qr(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
) -> Response {
    let url = format!("{}/netease/qr/create", state.meting_admin_base);
    match state.client.post(&url).send().await {
        Ok(resp) => match resp.json::<QrCreateResponse>().await {
            Ok(data) => Json(data).into_response(),
            Err(err) => (
                StatusCode::BAD_GATEWAY,
                format!("parse qr create response failed: {err}"),
            )
                .into_response(),
        },
        Err(err) => (
            StatusCode::BAD_GATEWAY,
            format!("request qr create failed: {err}"),
        )
            .into_response(),
    }
}

async fn check_netease_qr(
    Query(query): Query<QrCheckQuery>,
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
) -> Response {
    let url = format!(
        "{}/netease/qr/check?key={}",
        state.meting_admin_base,
        urlencoding::encode(&query.key)
    );
    match state.client.get(&url).send().await {
        Ok(resp) => match resp.json::<QrCheckResponse>().await {
            Ok(data) => Json(data).into_response(),
            Err(err) => (
                StatusCode::BAD_GATEWAY,
                format!("parse qr check response failed: {err}"),
            )
                .into_response(),
        },
        Err(err) => (
            StatusCode::BAD_GATEWAY,
            format!("request qr check failed: {err}"),
        )
            .into_response(),
    }
}

async fn start_qq_login(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
) -> Response {
    let url = format!("{}/tencent/browser-login/start", state.meting_admin_base);
    match state.client.post(&url).send().await {
        Ok(resp) => match resp.json::<serde_json::Value>().await {
            Ok(data) => Json(data).into_response(),
            Err(err) => (
                StatusCode::BAD_GATEWAY,
                format!("parse qq login start response failed: {err}"),
            )
                .into_response(),
        },
        Err(err) => (
            StatusCode::BAD_GATEWAY,
            format!("request qq login start failed: {err}"),
        )
            .into_response(),
    }
}

async fn check_qq_login(
    Query(query): Query<QqLoginStatusQuery>,
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
) -> Response {
    let url = format!(
        "{}/tencent/browser-login/status?session_id={}",
        state.meting_admin_base,
        urlencoding::encode(&query.session_id)
    );
    match state.client.get(&url).send().await {
        Ok(resp) => match resp.json::<serde_json::Value>().await {
            Ok(data) => Json(data).into_response(),
            Err(err) => (
                StatusCode::BAD_GATEWAY,
                format!("parse qq login status response failed: {err}"),
            )
                .into_response(),
        },
        Err(err) => (
            StatusCode::BAD_GATEWAY,
            format!("request qq login status failed: {err}"),
        )
            .into_response(),
    }
}

async fn stop_qq_login(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
) -> Response {
    let url = format!("{}/tencent/browser-login/stop", state.meting_admin_base);
    match state.client.post(&url).send().await {
        Ok(resp) if resp.status().is_success() => Json(serde_json::json!({ "ok": true })).into_response(),
        Ok(resp) => (
            StatusCode::BAD_GATEWAY,
            format!("request qq login stop failed: {}", resp.status()),
        )
            .into_response(),
        Err(err) => (
            StatusCode::BAD_GATEWAY,
            format!("request qq login stop failed: {err}"),
        )
            .into_response(),
    }
}

async fn download_batch(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
    Json(payload): Json<BatchDownloadRequest>,
) -> Response {
    if payload.tracks.is_empty() {
        return (StatusCode::BAD_REQUEST, "tracks is empty").into_response();
    }

    match build_zip_with_retry(&state.client, payload.tracks).await {
        Ok((zip_data, _)) => {
            let mut resp = Response::new(Body::from(zip_data));
            let headers = resp.headers_mut();
            headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/zip"));
            headers.insert(
                header::CONTENT_DISPOSITION,
                HeaderValue::from_static("attachment; filename=playlist_tracks.zip"),
            );
            resp
        }
        Err(err) => {
            error!("batch download failed: {err:#}");
            (StatusCode::BAD_GATEWAY, format!("batch download failed: {err}")).into_response()
        }
    }
}

async fn download_one(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
    Json(track): Json<Track>,
) -> Response {
    if track.url.trim().is_empty() {
        return (StatusCode::BAD_REQUEST, "track url is empty").into_response();
    }

    match fetch_track_bytes_with_retry(&state.client, &track.url, &track.name).await {
        Ok((bytes, ext)) => {
            let out_bytes = match enrich_audio_metadata(&state.client, &track, bytes.clone(), &ext).await {
                Ok(v) if !v.is_empty() => v,
                _ => bytes,
            };
            let mut resp = Response::new(Body::from(out_bytes));
            let headers = resp.headers_mut();
            headers.insert(
                header::CONTENT_TYPE,
                HeaderValue::from_str(content_type_by_ext(&ext)).unwrap_or(HeaderValue::from_static("application/octet-stream")),
            );
            if let Ok(v) = HeaderValue::from_str(&ext) {
                headers.insert("x-audio-ext", v);
            }
            resp
        }
        Err(err) => {
            error!("single download failed: {err:#}");
            (StatusCode::BAD_GATEWAY, format!("single download failed: {err}")).into_response()
        }
    }
}

async fn download_range(
    Query(query): Query<DownloadRangeQuery>,
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
) -> Response {
    let mut req = state.client.get(&query.url);
    let range_value = match query.end {
        Some(end) if end >= query.start => format!("bytes={}-{}", query.start, end),
        _ => format!("bytes={}-", query.start),
    };
    req = req.header(header::RANGE, range_value);

    match req.send().await {
        Ok(resp) => {
            let status = resp.status();
            if !status.is_success() && status.as_u16() != 206 {
                return (
                    StatusCode::BAD_GATEWAY,
                    format!("range upstream status {}", status.as_u16()),
                )
                    .into_response();
            }
            let content_type = resp
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("application/octet-stream")
                .to_string();
            let content_range = resp
                .headers()
                .get(header::CONTENT_RANGE)
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string());
            let content_length = resp
                .headers()
                .get(header::CONTENT_LENGTH)
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string());
            match resp.bytes().await {
                Ok(bytes) => {
                    let mut out = Response::new(Body::from(bytes));
                    *out.status_mut() = if status.as_u16() == 206 {
                        StatusCode::PARTIAL_CONTENT
                    } else {
                        StatusCode::OK
                    };
                    let headers = out.headers_mut();
                    headers.insert(
                        header::CONTENT_TYPE,
                        HeaderValue::from_str(&content_type).unwrap_or(HeaderValue::from_static("application/octet-stream")),
                    );
                    headers.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
                    if let Some(v) = content_range {
                        if let Ok(hv) = HeaderValue::from_str(&v) {
                            headers.insert(header::CONTENT_RANGE, hv);
                        }
                    }
                    if let Some(v) = content_length {
                        if let Ok(hv) = HeaderValue::from_str(&v) {
                            headers.insert(header::CONTENT_LENGTH, hv);
                        }
                    }
                    out
                }
                Err(err) => (
                    StatusCode::BAD_GATEWAY,
                    format!("read range bytes failed: {err}"),
                )
                    .into_response(),
            }
        }
        Err(err) => (
            StatusCode::BAD_GATEWAY,
            format!("range request failed: {err}"),
        )
            .into_response(),
    }
}

async fn fetch_playlist(state: &AppState, server: &str, playlist_id: &str, br: u32) -> Result<Vec<Track>> {
    let url = format!(
        "{}?server={}&type=playlist&id={}&br={}",
        state.meting_base, server, playlist_id, br
    );
    fetch_tracks_from_url(&state.client, &url).await
}

async fn fetch_search(state: &AppState, server: &str, keyword: &str, limit: usize, br: u32) -> Result<Vec<Track>> {
    let url_primary = format!(
        "{}?server={}&type=search&keyword={}&limit={}&br={}",
        state.meting_base,
        server,
        urlencoding::encode(keyword),
        limit,
        br
    );
    match fetch_tracks_from_url(&state.client, &url_primary).await {
        Ok(list) if !list.is_empty() => Ok(list),
        _ => {
            let url_fallback = format!(
                "{}?server={}&type=search&id={}&limit={}&br={}",
                state.meting_base,
                server,
                urlencoding::encode(keyword),
                limit,
                br
            );
            fetch_tracks_from_url(&state.client, &url_fallback).await
        }
    }
}

async fn fetch_tracks_from_url(client: &Client, url: &str) -> Result<Vec<Track>> {
    let resp = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("request meting url failed: {url}"))?
        .error_for_status()
        .context("meting returned non-success status")?;

    let v: serde_json::Value = resp.json().await.context("invalid meting json")?;
    let arr = pick_track_array(&v).context("meting response has no track array")?;

    let mut tracks = Vec::with_capacity(arr.len());
    for item in arr {
        let id = value_to_string(item.get("id"));
        let name = first_non_empty(
            value_to_string(item.get("name")),
            value_to_string(item.get("title")),
        );
        let url = value_to_string(item.get("url"));

        if name.is_empty() || url.is_empty() {
            continue;
        }

        let artist = match item.get("artist") {
            Some(serde_json::Value::Array(a)) => a
                .iter()
                .map(|x| x.as_str().unwrap_or_default())
                .collect::<Vec<_>>()
                .join(" / "),
            Some(v) => value_to_string(Some(v)),
            None => value_to_string(item.get("author")),
        };

        tracks.push(Track {
            id,
            name,
            artist,
            album: item.get("album").and_then(|x| x.as_str().map(|s| s.to_string())),
            url,
            pic: item.get("pic").and_then(|x| x.as_str().map(|s| s.to_string())),
            lrc: item.get("lrc").and_then(|x| x.as_str().map(|s| s.to_string())),
        });
    }

    Ok(tracks)
}

fn pick_track_array<'a>(v: &'a serde_json::Value) -> Option<&'a Vec<serde_json::Value>> {
    if let Some(arr) = v.as_array() {
        return Some(arr);
    }
    v.get("data")
        .and_then(|x| x.as_array())
        .or_else(|| v.get("songs").and_then(|x| x.as_array()))
        .or_else(|| {
            v.get("result")
                .and_then(|r| r.get("songs"))
                .and_then(|x| x.as_array())
        })
}

async fn fetch_lrc(client: &Client, lrc_url: &str) -> Result<String> {
    let resp = client
        .get(lrc_url)
        .send()
        .await
        .with_context(|| format!("request lrc failed: {lrc_url}"))?
        .error_for_status()
        .context("lrc returned non-success status")?;
    let txt = resp.text().await.context("read lrc text failed")?;
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&txt) {
        if let Some(lyric) = v.get("lyric").and_then(|x| x.as_str()) {
            return Ok(lyric.to_string());
        }
        if let Some(content) = v.get("content").and_then(|x| x.as_str()) {
            return Ok(content.to_string());
        }
    }
    Ok(txt)
}

async fn build_zip_with_retry(client: &Client, tracks: Vec<Track>) -> Result<(Vec<u8>, Vec<String>)> {
    let mut cursor = std::io::Cursor::new(Vec::new());
    let mut zip = zip::ZipWriter::new(&mut cursor);
    let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let mut failed = Vec::new();

    for (idx, track) in tracks.iter().enumerate() {
        if track.url.trim().is_empty() {
            failed.push(format!("{}: empty url", track.name));
            continue;
        }

        match fetch_track_bytes_with_retry(client, &track.url, &track.name).await {
            Ok((bytes, ext)) => {
                let out_bytes = match enrich_audio_metadata(client, track, bytes.clone(), &ext).await {
                    Ok(v) if !v.is_empty() => v,
                    _ => bytes,
                };
                let file_name = sanitize_filename(&format!("{:02}_{}_{}.{}", idx + 1, track.artist, track.name, ext));
                zip.start_file(file_name, options)?;
                zip.write_all(&out_bytes)?;
            }
            Err(err) => {
                failed.push(format!("{}: {}", track.name, err));
            }
        }
    }

    zip.finish()?;
    Ok((cursor.into_inner(), failed))
}

async fn fetch_track_bytes_with_retry(client: &Client, url: &str, name: &str) -> Result<(Bytes, String)> {
    let mut last_err = None;
    let strict_lossless = is_lossless_requested(url);
    for attempt in 1..=3 {
        match client.get(url).send().await {
            Ok(resp) => match resp.error_for_status() {
                Ok(ok_resp) => {
                    let content_type = ok_resp
                        .headers()
                        .get(header::CONTENT_TYPE)
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or("")
                        .to_ascii_lowercase();
                    let body = ok_resp.bytes().await.with_context(|| format!("read bytes failed: {name}"))?;

                    if content_type.contains("application/json") || body.starts_with(b"{") {
                        if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&body) {
                            if let Some(real_url) = v.get("url").and_then(|x| x.as_str()) {
                                let media_resp = client
                                    .get(real_url)
                                    .send()
                                    .await
                                    .with_context(|| format!("download real media failed: {name}"))?
                                    .error_for_status()
                                    .with_context(|| format!("real media status error: {name}"))?;
                                let media_ct = media_resp
                                    .headers()
                                    .get(header::CONTENT_TYPE)
                                    .and_then(|v| v.to_str().ok())
                                    .unwrap_or("")
                                    .to_ascii_lowercase();
                                let media = media_resp
                                    .bytes()
                                    .await
                                    .with_context(|| format!("read real media bytes failed: {name}"))?;
                                let ext = infer_audio_ext(&media_ct, &media);
                                if strict_lossless && ext != "flac" {
                                    last_err = Some(anyhow!(
                                        "lossless requested but upstream returned {} for {}",
                                        ext,
                                        name
                                    ));
                                    if attempt < 3 {
                                        warn!("retrying lossless track {} attempt {}", name, attempt + 1);
                                        tokio::time::sleep(Duration::from_millis(700 * attempt as u64)).await;
                                        continue;
                                    }
                                    return Err(anyhow!(
                                        "lossless requested but upstream returned {}",
                                        ext
                                    ));
                                }
                                return Ok((media, ext));
                            }
                        }
                    }
                    let ext = infer_audio_ext(&content_type, &body);
                    if strict_lossless && ext != "flac" {
                        last_err = Some(anyhow!(
                            "lossless requested but upstream returned {} for {}",
                            ext,
                            name
                        ));
                        if attempt < 3 {
                            warn!("retrying lossless track {} attempt {}", name, attempt + 1);
                            tokio::time::sleep(Duration::from_millis(700 * attempt as u64)).await;
                            continue;
                        }
                        return Err(anyhow!("lossless requested but upstream returned {}", ext));
                    }
                    return Ok((body, ext));
                }
                Err(err) => {
                    last_err = Some(anyhow!(err).context(format!("status error attempt {attempt}")));
                }
            },
            Err(err) => {
                last_err = Some(anyhow!(err).context(format!("request error attempt {attempt}")));
            }
        }

        if attempt < 3 {
            warn!("retrying track download {} attempt {}", name, attempt + 1);
            tokio::time::sleep(Duration::from_millis(700 * attempt as u64)).await;
        }
    }

    Err(last_err.unwrap_or_else(|| anyhow!("unknown download error")))
}

fn is_lossless_requested(url: &str) -> bool {
    if let Ok(parsed) = reqwest::Url::parse(url) {
        if let Some(br) = parsed
            .query_pairs()
            .find(|(k, _)| k == "br")
            .and_then(|(_, v)| v.parse::<u32>().ok())
        {
            return br >= 999;
        }
    }
    false
}

fn infer_audio_ext(content_type: &str, bytes: &Bytes) -> String {
    if content_type.contains("flac") || starts_with_flac(bytes) {
        return "flac".to_string();
    }
    if content_type.contains("mp4") || content_type.contains("m4a") {
        return "m4a".to_string();
    }
    if content_type.contains("aac") {
        return "aac".to_string();
    }
    if content_type.contains("mpeg") || content_type.contains("mp3") {
        return "mp3".to_string();
    }
    if content_type.contains("ogg") {
        return "ogg".to_string();
    }
    if content_type.contains("wav") {
        return "wav".to_string();
    }
    "mp3".to_string()
}

fn starts_with_flac(bytes: &Bytes) -> bool {
    bytes.len() >= 4 && &bytes[..4] == b"fLaC"
}

fn value_to_string(v: Option<&serde_json::Value>) -> String {
    match v {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(serde_json::Value::Number(n)) => n.to_string(),
        Some(serde_json::Value::Bool(b)) => b.to_string(),
        _ => String::new(),
    }
}

fn first_non_empty(a: String, b: String) -> String {
    if !a.trim().is_empty() {
        a
    } else {
        b
    }
}

fn sanitize_filename(s: &str) -> String {
    let invalid = ['<', '>', ':', '"', '/', '\\', '|', '?', '*'];
    s.chars().map(|c| if invalid.contains(&c) { '_' } else { c }).collect()
}

fn content_type_by_ext(ext: &str) -> &'static str {
    match ext {
        "flac" => "audio/flac",
        "mp3" => "audio/mpeg",
        "m4a" => "audio/mp4",
        "ogg" => "audio/ogg",
        "wav" => "audio/wav",
        "aac" => "audio/aac",
        _ => "application/octet-stream",
    }
}

async fn enrich_audio_metadata(client: &Client, track: &Track, bytes: Bytes, ext: &str) -> Result<Bytes> {
    let lyric = if let Some(lrc) = &track.lrc {
        fetch_lrc(client, lrc).await.ok()
    } else {
        None
    };
    let cover = if let Some(pic_url) = &track.pic {
        fetch_cover_bytes(client, pic_url).await.ok()
    } else {
        None
    };

    let original = bytes.clone();
    let tagged = match ext {
        "mp3" => tag_mp3(bytes, track, lyric, cover),
        "flac" => tag_flac(bytes, track, lyric, cover),
        "wav" => tag_wav(bytes, track, lyric, cover),
        // For some quality profiles upstream returns aac/ogg/m4a.
        // Use ID3-compatible fallback to maximize metadata availability.
        "aac" | "ogg" | "m4a" => tag_mp3(bytes, track, lyric, cover),
        _ => Ok(original.clone()),
    }?;
    if tagged.len() < original.len() / 2 {
        return Ok(original);
    }
    Ok(tagged)
}

fn tag_mp3(
    bytes: Bytes,
    track: &Track,
    lyric: Option<String>,
    cover: Option<(Vec<u8>, String)>,
) -> Result<Bytes> {
    let mut tag = Id3Tag::new();
    tag.set_title(track.name.clone());
    tag.set_artist(track.artist.clone());
    if let Some(album) = &track.album {
        if !album.trim().is_empty() {
            tag.set_album(album.clone());
        }
    }
    if let Some(text) = lyric {
        tag.add_frame(Lyrics {
            lang: "chi".to_string(),
            description: "LRC".to_string(),
            text,
        });
    }
    if let Some((data, mime)) = cover {
        tag.add_frame(Picture {
            mime_type: mime,
            picture_type: PictureType::CoverFront,
            description: "Cover".to_string(),
            data,
        });
    }

    let path = write_temp_audio(&bytes, "mp3")?;
    tag.write_to_path(&path, Version::Id3v24)?;
    read_temp_audio(path)
}

fn tag_flac(
    bytes: Bytes,
    track: &Track,
    lyric: Option<String>,
    cover: Option<(Vec<u8>, String)>,
) -> Result<Bytes> {
    let path = write_temp_audio(&bytes, "flac")?;
    let mut tag = match FlacTag::read_from_path(&path) {
        Ok(v) => v,
        Err(_) => return Ok(bytes),
    };
    {
        let comments = tag.vorbis_comments_mut();
        comments.set_title(vec![track.name.clone()]);
        comments.set_artist(vec![track.artist.clone()]);
        if let Some(album) = &track.album {
            if !album.trim().is_empty() {
                comments.set_album(vec![album.clone()]);
            }
        }
        if let Some(text) = lyric {
            comments.set("LYRICS".to_string(), vec![text]);
        }
    }
    if let Some((data, mime)) = cover {
        tag.remove_blocks(metaflac::BlockType::Picture);
        tag.add_picture(mime, FlacPictureType::CoverFront, data);
    }
    tag.save()?;
    read_temp_audio(path)
}


fn tag_wav(
    bytes: Bytes,
    track: &Track,
    lyric: Option<String>,
    cover: Option<(Vec<u8>, String)>,
) -> Result<Bytes> {
    let mut tag = Id3Tag::new();
    tag.set_title(track.name.clone());
    tag.set_artist(track.artist.clone());
    if let Some(album) = &track.album {
        if !album.trim().is_empty() {
            tag.set_album(album.clone());
        }
    }
    if let Some(text) = lyric {
        tag.add_frame(Lyrics {
            lang: "chi".to_string(),
            description: "LRC".to_string(),
            text,
        });
    }
    if let Some((data, mime)) = cover {
        tag.add_frame(Picture {
            mime_type: mime,
            picture_type: PictureType::CoverFront,
            description: "Cover".to_string(),
            data,
        });
    }
    let path = write_temp_audio(&bytes, "wav")?;
    tag.write_to_path(&path, Version::Id3v24)?;
    read_temp_audio(path)
}

fn write_temp_audio(bytes: &Bytes, ext: &str) -> Result<PathBuf> {
    let file = tempfile::Builder::new()
        .prefix("musicdownload-tag-")
        .suffix(&format!(".{ext}"))
        .tempfile()?;
    let (_f, path) = file.keep()?;
    fs::write(&path, bytes)?;
    Ok(path)
}

fn read_temp_audio(path: PathBuf) -> Result<Bytes> {
    let data = fs::read(&path)?;
    let _ = fs::remove_file(&path);
    Ok(Bytes::from(data))
}
async fn fetch_cover_bytes(client: &Client, url: &str) -> Result<(Vec<u8>, String)> {
    let resp = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("request cover failed: {url}"))?
        .error_for_status()
        .context("cover returned non-success status")?;
    let mime = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|x| x.to_str().ok())
        .unwrap_or("image/jpeg")
        .to_string();
    let data = resp.bytes().await.context("read cover bytes failed")?.to_vec();
    Ok((data, mime))
}








