use crate::config::ServiceInstance;
use crate::supervisor::ServiceManager;
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Path, Query, State,
    },
    http::StatusCode,
    response::{IntoResponse, Json},
    routing::{get, post},
    Router,
};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tower_http::cors::CorsLayer;
use tower_http::services::ServeDir;
use tracing::info;

#[derive(Clone)]
pub struct AppState {
    pub manager: Arc<ServiceManager>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct ServiceWithState {
    #[serde(flatten)]
    pub instance: ServiceInstance,
    pub runtime: crate::config::ServiceRuntimeState,
}

#[derive(Deserialize)]
pub struct LogQuery {
    pub limit: Option<usize>,
}

pub fn create_router(state: AppState, static_dir: Option<PathBuf>) -> Router {
    let mut router = Router::new()
        .route("/api/health", get(health_check))
        .route("/api/services", get(list_services).post(add_service))
        .route(
            "/api/services/{id}",
            get(get_service).put(update_service).delete(delete_service),
        )
        .route("/api/services/{id}/start", post(start_service))
        .route("/api/services/{id}/stop", post(stop_service))
        .route("/api/services/{id}/restart", post(restart_service))
        .route("/api/services/{id}/probe", post(probe_service))
        .route("/api/services/{id}/logs", get(get_logs))
        .route("/api/services/{id}/logs/ws", get(ws_logs_handler))
        .route("/api/events/ws", get(ws_events_handler))
        .layer(CorsLayer::permissive())
        .with_state(state);

    if let Some(dir) = static_dir {
        if dir.exists() {
            info!("Serving static UI assets from {:?}", dir);
            router = router.fallback_service(ServeDir::new(dir));
        }
    }

    router
}

pub async fn start_server(
    manager: Arc<ServiceManager>,
    addr: SocketAddr,
    static_dir: Option<PathBuf>,
) -> anyhow::Result<()> {
    let state = AppState { manager };
    let app = create_router(state, static_dir);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!("ProxyMan HTTP/IPC server listening on http://{}", addr);
    axum::serve(listener, app).await?;
    Ok(())
}

async fn health_check() -> impl IntoResponse {
    Json(serde_json::json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION"),
    }))
}

async fn list_services(State(state): State<AppState>) -> impl IntoResponse {
    let instances = state.manager.store().list().await;
    let mut res = Vec::new();
    for inst in instances {
        let runtime = state.manager.get_state(&inst.id).await;
        res.push(ServiceWithState {
            instance: inst,
            runtime,
        });
    }
    Json(res)
}

async fn get_service(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, StatusCode> {
    if let Some(inst) = state.manager.store().get(&id).await {
        let runtime = state.manager.get_state(&id).await;
        Ok(Json(ServiceWithState {
            instance: inst,
            runtime,
        }))
    } else {
        Err(StatusCode::NOT_FOUND)
    }
}

async fn add_service(
    State(state): State<AppState>,
    Json(payload): Json<ServiceInstance>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let id = payload.id.clone();
    state
        .manager
        .store()
        .add(payload)
        .await
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;

    Ok(Json(serde_json::json!({ "success": true, "id": id })))
}

async fn update_service(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(mut payload): Json<ServiceInstance>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    payload.id = id.clone();
    state
        .manager
        .store()
        .update(payload)
        .await
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;

    Ok(Json(serde_json::json!({ "success": true, "id": id })))
}

async fn delete_service(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let _ = state.manager.stop_service(&id).await;
    match state.manager.store().remove(&id).await {
        Ok(Some(_)) => Ok(Json(serde_json::json!({ "success": true }))),
        Ok(None) => Err((StatusCode::NOT_FOUND, "Service not found".to_string())),
        Err(e) => Err((StatusCode::INTERNAL_SERVER_ERROR, e.to_string())),
    }
}

async fn start_service(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    state
        .manager
        .start_service(&id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(serde_json::json!({ "success": true })))
}

async fn stop_service(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    state
        .manager
        .stop_service(&id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(serde_json::json!({ "success": true })))
}

async fn restart_service(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    state
        .manager
        .restart_service(&id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(serde_json::json!({ "success": true })))
}

async fn probe_service(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let probe_res = state
        .manager
        .run_immediate_probe(&id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(serde_json::json!({
        "success": probe_res.success,
        "latency_ms": probe_res.latency_ms,
        "error": probe_res.error,
    })))
}

async fn get_logs(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<LogQuery>,
) -> impl IntoResponse {
    let limit = query.limit.unwrap_or(200);
    let logs = state.manager.get_logs(&id, limit).await;
    Json(logs)
}

async fn ws_logs_handler(
    ws: WebSocketUpgrade,
    Path(id): Path<String>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_ws_logs(socket, id, state))
}

async fn handle_ws_logs(mut socket: WebSocket, id: String, state: AppState) {
    if let Some(mut rx) = state.manager.subscribe_logs(&id).await {
        while let Ok(line) = rx.recv().await {
            if socket.send(Message::Text(line.into())).await.is_err() {
                break;
            }
        }
    }
}

async fn ws_events_handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_ws_events(socket, state))
}

async fn handle_ws_events(mut socket: WebSocket, state: AppState) {
    let mut rx = state.manager.subscribe_events();
    while let Ok(event) = rx.recv().await {
        if let Ok(json) = serde_json::to_string(&event) {
            if socket.send(Message::Text(json.into())).await.is_err() {
                break;
            }
        }
    }
}
