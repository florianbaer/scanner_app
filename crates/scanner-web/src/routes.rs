//! HTTP routes. Handlers only translate between HTTP and `ScannerService`.
//! Blocking service calls (discovery, capability queries) run on the blocking pool.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use scanner_core::output;
use scanner_core::service::{ScanRequest, ScannerService};

use crate::dto;
use crate::error::{ApiError, ApiResult};

type AppState = Arc<ScannerService>;

/// How long a discovery request listens for network scanners.
const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(6);

pub fn router(service: AppState) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/app.js", get(app_js))
        .route("/style.css", get(style_css))
        .route("/api/devices", get(devices))
        .route("/api/capabilities", get(capabilities))
        .route("/api/destinations", get(destinations))
        .route("/api/jobs", get(list_jobs).post(create_job))
        .route("/api/jobs/{id}", get(get_job))
        .route("/api/jobs/{id}/cancel", post(cancel_job))
        .route("/api/uploads/pending", get(pending_uploads))
        .route("/api/uploads/retry", post(retry_uploads))
        .route("/api/files", get(list_files))
        .route("/files/{name}", get(download))
        .with_state(service)
}

// --- static UI (embedded in the binary) -------------------------------------

async fn index() -> impl IntoResponse {
    static_file(
        "text/html; charset=utf-8",
        include_str!("static/index.html"),
    )
}

async fn app_js() -> impl IntoResponse {
    static_file(
        "text/javascript; charset=utf-8",
        include_str!("static/app.js"),
    )
}

async fn style_css() -> impl IntoResponse {
    static_file("text/css; charset=utf-8", include_str!("static/style.css"))
}

fn static_file(content_type: &'static str, body: &'static str) -> impl IntoResponse {
    ([(header::CONTENT_TYPE, content_type)], body)
}

// --- devices -----------------------------------------------------------------

async fn devices(State(svc): State<AppState>) -> ApiResult<Json<dto::Devices>> {
    let list = tokio::task::spawn_blocking(move || svc.discover(DISCOVERY_TIMEOUT)).await??;
    Ok(Json(list.into()))
}

async fn capabilities(
    State(svc): State<AppState>,
    Query(q): Query<dto::DeviceQuery>,
) -> ApiResult<Json<dto::CapabilitiesDto>> {
    let caps = tokio::task::spawn_blocking(move || svc.capabilities(&q.device)).await??;
    Ok(Json(caps.into()))
}

async fn destinations(State(svc): State<AppState>) -> Json<Vec<dto::DestinationDto>> {
    Json(svc.destinations().into_iter().map(Into::into).collect())
}

// --- jobs --------------------------------------------------------------------

async fn list_jobs(State(svc): State<AppState>) -> Json<Vec<dto::JobDto>> {
    Json(svc.jobs().into_iter().map(Into::into).collect())
}

async fn create_job(
    State(svc): State<AppState>,
    Json(body): Json<dto::NewScan>,
) -> ApiResult<(StatusCode, Json<dto::Created>)> {
    let request = ScanRequest::try_from(body)?;
    let id = svc.submit(request)?;
    Ok((StatusCode::ACCEPTED, Json(dto::Created { id })))
}

async fn get_job(State(svc): State<AppState>, Path(id): Path<u64>) -> ApiResult<Json<dto::JobDto>> {
    svc.job(id)
        .map(|j| Json(j.into()))
        .ok_or_else(|| ApiError::not_found("job"))
}

async fn cancel_job(State(svc): State<AppState>, Path(id): Path<u64>) -> ApiResult<StatusCode> {
    if svc.cancel(id) {
        Ok(StatusCode::ACCEPTED)
    } else {
        Err(ApiError::bad_request("job is not running or queued"))
    }
}

// --- pending uploads (retries) ---------------------------------------------

async fn pending_uploads(State(svc): State<AppState>) -> Json<dto::PendingUploads> {
    Json(dto::PendingUploads {
        items: svc.pending_uploads().into_iter().map(Into::into).collect(),
        error: svc.pending_uploads_error(),
    })
}

async fn retry_uploads(State(svc): State<AppState>) -> ApiResult<StatusCode> {
    svc.retry_uploads_now()?;
    Ok(StatusCode::ACCEPTED)
}

// --- files -------------------------------------------------------------------

async fn list_files(State(svc): State<AppState>) -> ApiResult<Json<Vec<dto::FileDto>>> {
    Ok(Json(svc.files()?.into_iter().map(Into::into).collect()))
}

async fn download(
    State(svc): State<AppState>,
    Path(name): Path<String>,
    Query(q): Query<dto::DownloadQuery>,
) -> ApiResult<Response> {
    let path = svc
        .file_path(&name)
        .ok_or_else(|| ApiError::not_found("file"))?;
    let bytes = tokio::fs::read(&path).await?;
    let content_type = output::content_type(&path);
    // `name` is validated by core to [A-Za-z0-9._-], so it is safe in the header.
    let disposition = format!(
        "{}; filename=\"{name}\"",
        if q.inline { "inline" } else { "attachment" }
    );
    Ok((
        [
            (header::CONTENT_TYPE, content_type.to_string()),
            (header::CONTENT_DISPOSITION, disposition),
        ],
        bytes,
    )
        .into_response())
}
