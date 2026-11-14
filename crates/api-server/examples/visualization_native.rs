//! Loopback-only native GUI integration host. Model fixtures invoke real tools.
use axum::{Json, Router, extract::State, http::StatusCode, routing::post};
use serde_json::{Value, json};
async fn tool(
    State(path): State<String>,
    Json(input): Json<Value>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let registration = visualization_tools::registrations(path.into()).remove(0);
    let result = registration
        .implementation
        .execute(input)
        .await
        .map_err(|error| {
            (
                StatusCode::BAD_REQUEST,
                Json(json!({"error":error.to_string()})),
            )
        })?;
    Ok(Json(serde_json::to_value(result).unwrap()))
}
// Real catalogue/tool path for opt-in asset integration tests; loopback only.
async fn bioicons(
    State(path): State<String>,
    Json(input): Json<Value>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let registration = visualization_tools::bioicons_registrations(path.into()).remove(0);
    let result = registration
        .implementation
        .execute(input)
        .await
        .map_err(|error| {
            (
                StatusCode::BAD_REQUEST,
                Json(json!({"error":error.to_string()})),
            )
        })?;
    Ok(Json(serde_json::to_value(result).unwrap()))
}
async fn bioart(
    State(path): State<String>,
    Json(input): Json<Value>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let registration = visualization_tools::bioart_registrations(path.into()).remove(0);
    let result = registration
        .implementation
        .execute(input)
        .await
        .map_err(|error| {
            (
                StatusCode::BAD_REQUEST,
                Json(json!({"error":error.to_string()})),
            )
        })?;
    Ok(Json(serde_json::to_value(result).unwrap()))
}
async fn find_space(
    State(path): State<String>,
    Json(input): Json<Value>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let registration = visualization_tools::registrations(path.into())
        .into_iter()
        .find(|tool| tool.definition.name == "visualization_find_space")
        .expect("registered placement tool");
    let result = registration
        .implementation
        .execute(input)
        .await
        .map_err(|error| {
            (
                StatusCode::BAD_REQUEST,
                Json(json!({"error":error.to_string()})),
            )
        })?;
    Ok(Json(serde_json::to_value(result).unwrap()))
}
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let path = args.next().ok_or("database path is required")?;
    let addr: std::net::SocketAddr = args
        .next()
        .unwrap_or_else(|| "127.0.0.1:0".into())
        .parse()?;
    if !addr.ip().is_loopback() {
        return Err("integration host requires a loopback address".into());
    }
    let store = visualization::Store::open(&path)?;
    let app = Router::new()
        .nest(
            "/api/v1/visualization",
            api_server::visualization::router(store),
        )
        .merge(
            Router::new()
                .route("/test/visualization", post(tool))
                .route("/test/bioicons", post(bioicons))
                .route("/test/bioart", post(bioart))
                .route("/test/find-space", post(find_space))
                .with_state(path),
        );
    let listener = tokio::net::TcpListener::bind(addr).await?;
    println!("VISUALIZATION_NATIVE_ADDR={}", listener.local_addr()?);
    axum::serve(listener, app).await?;
    Ok(())
}
