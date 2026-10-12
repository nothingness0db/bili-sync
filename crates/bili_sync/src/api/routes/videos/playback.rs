use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::Body;
use axum::extract::{Extension, Path, Query, Request};
use axum::http::{StatusCode, header};
use axum::response::Response;
use axum::routing::get;
use bili_sync_entity::{page, video};
use parking_lot::Mutex;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder};
use serde::{Deserialize, Serialize};
use tower_http::services::ServeFile;
use uuid::Uuid;

use crate::api::error::InnerApiError;
use crate::api::wrapper::{ApiError, ApiResponse};
use crate::config::VersionedConfig;

const SESSION_TTL: Duration = Duration::from_secs(6 * 60 * 60);
const MAX_SESSIONS: usize = 256;
static SESSIONS: LazyLock<Mutex<PlaybackSessions>> = LazyLock::new(|| Mutex::new(PlaybackSessions::default()));

struct PlaybackSession {
    video_id: i32,
    paths: HashMap<i32, PathBuf>,
    auth_token: String,
    expires_at: Instant,
}

#[derive(Default)]
struct PlaybackSessions(HashMap<Uuid, PlaybackSession>);

impl PlaybackSessions {
    fn create(&mut self, video_id: i32, paths: HashMap<i32, PathBuf>, auth_token: String) -> Uuid {
        let now = Instant::now();
        self.0.retain(|_, session| session.expires_at > now);
        if self.0.len() >= MAX_SESSIONS
            && let Some(oldest) = self
                .0
                .iter()
                .min_by_key(|(_, session)| session.expires_at)
                .map(|(id, _)| *id)
        {
            self.0.remove(&oldest);
        }
        let ticket = Uuid::new_v4();
        self.0.insert(
            ticket,
            PlaybackSession {
                video_id,
                paths,
                auth_token,
                expires_at: now + SESSION_TTL,
            },
        );
        ticket
    }

    fn path(&self, ticket: &str, video_id: i32, page_id: i32, auth_token: &str) -> Option<PathBuf> {
        let session = self.0.get(&Uuid::parse_str(ticket).ok()?)?;
        if session.expires_at <= Instant::now() || session.video_id != video_id || session.auth_token != auth_token {
            return None;
        }
        session.paths.get(&page_id).cloned()
    }
}

#[derive(Serialize)]
pub(super) struct PlaybackPage {
    id: i32,
    pid: i32,
    name: String,
    url: String,
}

#[derive(Serialize)]
pub(super) struct PlaybackResponse {
    pages: Vec<PlaybackPage>,
}

/// 只提供数据库中属于该视频、且本地仍存在的文件，不依赖 B 站有效状态或下载任务状态。
pub(super) async fn create_playback(
    Extension(db): Extension<DatabaseConnection>,
    Path(video_id): Path<i32>,
) -> Result<ApiResponse<PlaybackResponse>, ApiError> {
    if video::Entity::find_by_id(video_id).one(&db).await?.is_none() {
        return Err(InnerApiError::NotFound(video_id).into());
    }
    let pages = page::Entity::find()
        .filter(page::Column::VideoId.eq(video_id))
        .order_by_asc(page::Column::Pid)
        .all(&db)
        .await?;
    let mut available = Vec::new();
    let mut paths = HashMap::new();
    for page in pages {
        let Some(path) = page.path.as_deref().filter(|path| !path.is_empty()) else {
            continue;
        };
        match tokio::fs::metadata(path).await {
            Ok(metadata) if metadata.is_file() && metadata.len() > 0 => {
                paths.insert(page.id, PathBuf::from(path));
                available.push(page);
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                tracing::warn!(page_id = page.id, %error, "读取本地视频文件失败");
            }
        }
    }
    if available.is_empty() {
        return Ok(ApiResponse::bad_request(
            "没有可播放的本地文件：视频尚未下载，或文件已被移除",
        ));
    }
    // 永久登录 token 不放进媒体 URL；凭据限定视频、分 P、有效期，并在登录 token 修改后失效。
    let auth_token = VersionedConfig::get().read().auth_token.clone();
    let ticket = SESSIONS.lock().create(video_id, paths, auth_token);
    Ok(ApiResponse::ok(PlaybackResponse {
        pages: available
            .into_iter()
            .map(|page| PlaybackPage {
                id: page.id,
                pid: page.pid,
                name: page.name,
                url: format!("/api/videos/{video_id}/pages/{}/stream?ticket={ticket}", page.id),
            })
            .collect(),
    }))
}

pub(in crate::api::routes) fn stream_router() -> Router {
    Router::new().route("/videos/{video_id}/pages/{page_id}/stream", get(stream_video))
}

#[derive(Deserialize)]
struct StreamQuery {
    ticket: Option<String>,
}

async fn stream_video(
    Path((video_id, page_id)): Path<(i32, i32)>,
    Query(query): Query<StreamQuery>,
    request: Request,
) -> Result<Response, StatusCode> {
    let ticket = query.ticket.ok_or(StatusCode::UNAUTHORIZED)?;
    let auth_token = VersionedConfig::get().read().auth_token.clone();
    let path = SESSIONS
        .lock()
        .path(&ticket, video_id, page_id, &auth_token)
        .ok_or(StatusCode::UNAUTHORIZED)?;
    serve_video(path, request).await
}

async fn serve_video(path: PathBuf, request: Request) -> Result<Response, StatusCode> {
    // ServeFile 支持 GET/HEAD 和 HTTP Range，按块读取，拖动进度无需下载整个文件。
    let mut response = ServeFile::new(path)
        .try_call(request)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .map(Body::new);
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, "private, no-store".parse().unwrap());
    response
        .headers_mut()
        .insert(header::REFERRER_POLICY, "no-referrer".parse().unwrap());
    Ok(response)
}

#[cfg(test)]
mod tests {
    use axum::body::to_bytes;
    use axum::http::Method;
    use bili_sync_migration::{Migrator, MigratorTrait};
    use sea_orm::ActiveValue::Set;
    use sea_orm::{ActiveModelTrait, ConnectOptions, Database, IntoActiveModel};
    use serde_json::Value;
    use tower::ServiceExt;

    use super::*;

    async fn fixture() -> (DatabaseConnection, async_tempfile::TempDir, PathBuf) {
        let mut options = ConnectOptions::new("sqlite::memory:");
        options.max_connections(1).sqlx_logging(false);
        let db = Database::connect(options).await.unwrap();
        Migrator::up(&db, None).await.unwrap();
        let mut video = video::Model {
            id: 1,
            valid: false,
            bvid: "BV1test".into(),
            ..Default::default()
        }
        .into_active_model();
        video.id = Set(1);
        video.insert(&db).await.unwrap();
        let dir = async_tempfile::TempDir::new().await.unwrap();
        let file = dir.dir_path().join("视频.mp4");
        tokio::fs::write(&file, b"0123456789").await.unwrap();
        // 分 P 主键顺序与 pid 顺序不同，另有丢失、空文件、目录及未下载的分页。
        let empty = dir.dir_path().join("empty.mp4");
        tokio::fs::write(&empty, b"").await.unwrap();
        for (id, pid, path) in [
            (10, 2, Some(file.clone())),
            (11, 1, Some(file.clone())),
            (12, 3, Some(dir.dir_path().join("missing.mp4"))),
            (13, 4, Some(empty)),
            (14, 5, Some(dir.dir_path().clone())),
            (15, 6, None),
        ] {
            let mut page = page::Model {
                id,
                video_id: 1,
                cid: id.into(),
                pid,
                name: format!("分页 {pid}"),
                path: path.map(|path| path.to_string_lossy().into_owned()),
                download_status: 0,
                ..Default::default()
            }
            .into_active_model();
            page.id = Set(id);
            page.insert(&db).await.unwrap();
        }
        (db, dir, file)
    }

    fn request(method: Method, url: &str, auth: bool) -> Request {
        let mut builder = Request::builder().method(method).uri(url);
        if auth {
            builder = builder.header(header::AUTHORIZATION, VersionedConfig::get().read().auth_token.clone());
        }
        builder.body(Body::empty()).unwrap()
    }

    #[tokio::test]
    async fn invalid_video_with_reset_status_still_plays_existing_pages_in_pid_order() {
        let (db, _dir, _) = fixture().await;
        let app = crate::api::routes::router().layer(Extension(db));
        let response = app
            .clone()
            .oneshot(request(Method::POST, "/api/videos/1/playback", true))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let data: Value = serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
        let pages = data["data"]["pages"].as_array().unwrap();
        assert_eq!(pages.len(), 2);
        assert_eq!(pages[0]["pid"], 1);
        assert_eq!(pages[1]["pid"], 2);
        let url = pages[0]["url"].as_str().unwrap();
        // 媒体请求没有 Authorization 头，只能访问凭据明确授权的分 P。
        let response = app.clone().oneshot(request(Method::GET, url, false)).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CONTENT_TYPE], "video/mp4");
        assert_eq!(
            to_bytes(response.into_body(), 4096).await.unwrap().as_ref(),
            b"0123456789"
        );
        let denied = app
            .oneshot(request(Method::GET, &url.replace("/pages/11/", "/pages/12/"), false))
            .await
            .unwrap();
        assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn playback_creation_requires_login_and_reports_missing_video_or_files() {
        let (db, _dir, file) = fixture().await;
        let app = crate::api::routes::router().layer(Extension(db));
        let response = app
            .clone()
            .oneshot(request(Method::POST, "/api/videos/1/playback", false))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let response = app
            .clone()
            .oneshot(request(Method::POST, "/api/videos/999/playback", true))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        tokio::fs::remove_file(file).await.unwrap();
        let response = app
            .oneshot(request(Method::POST, "/api/videos/1/playback", true))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let data: Value = serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
        assert!(data["message"].as_str().unwrap().contains("没有可播放的本地文件"));
    }

    #[tokio::test]
    async fn stream_rejects_missing_or_invalid_ticket_even_with_login_header() {
        let app = stream_router();
        for url in ["/videos/1/pages/10/stream", "/videos/1/pages/10/stream?ticket=invalid"] {
            let response = app.clone().oneshot(request(Method::GET, url, true)).await.unwrap();
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        }
    }

    #[test]
    fn tickets_are_scoped_expire_and_are_revoked_on_auth_token_change() {
        let mut sessions = PlaybackSessions::default();
        let ticket = sessions.create(1, HashMap::from([(10, PathBuf::from("video.mp4"))]), "secret".into());
        let key = ticket.to_string();
        assert!(sessions.path(&key, 1, 10, "secret").is_some());
        assert!(sessions.path(&key, 2, 10, "secret").is_none());
        assert!(sessions.path(&key, 1, 11, "secret").is_none());
        assert!(sessions.path(&key, 1, 10, "new secret").is_none());
        sessions.0.get_mut(&ticket).unwrap().expires_at = Instant::now() - Duration::from_secs(1);
        assert!(sessions.path(&key, 1, 10, "secret").is_none());
        sessions.create(1, HashMap::new(), "secret".into());
        assert!(!sessions.0.contains_key(&ticket));
    }

    #[test]
    fn session_memory_is_bounded() {
        let mut sessions = PlaybackSessions::default();
        let oldest = sessions.create(1, HashMap::new(), "secret".into());
        for _ in 0..MAX_SESSIONS {
            sessions.create(1, HashMap::new(), "secret".into());
        }
        assert_eq!(sessions.0.len(), MAX_SESSIONS);
        assert!(!sessions.0.contains_key(&oldest));
    }

    #[tokio::test]
    async fn media_supports_seeking_head_and_unsatisfiable_ranges() {
        let (_db, _dir, file) = fixture().await;
        for (range, expected) in [("bytes=2-5", b"2345".as_slice()), ("bytes=-3", b"789".as_slice())] {
            let req = Request::builder()
                .header(header::RANGE, range)
                .body(Body::empty())
                .unwrap();
            let response = serve_video(file.clone(), req).await.unwrap();
            assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
            assert_eq!(response.headers()[header::CACHE_CONTROL], "private, no-store");
            assert_eq!(to_bytes(response.into_body(), 4096).await.unwrap().as_ref(), expected);
        }
        let response = serve_video(file.clone(), request(Method::HEAD, "/", false))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CONTENT_LENGTH], "10");
        assert!(to_bytes(response.into_body(), 4096).await.unwrap().is_empty());
        let req = Request::builder()
            .header(header::RANGE, "bytes=99-100")
            .body(Body::empty())
            .unwrap();
        let response = serve_video(file.clone(), req).await.unwrap();
        assert_eq!(response.status(), StatusCode::RANGE_NOT_SATISFIABLE);
        tokio::fs::remove_file(&file).await.unwrap();
        let response = serve_video(file, request(Method::GET, "/", false)).await.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}
