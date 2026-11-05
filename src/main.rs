mod model;
mod orion;

use std::sync::Arc;

use anyhow::{Context, Result};
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{StatusCode, header},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::json;
use tokio::sync::Mutex;

use model::Day;
use orion::{Client, Credentials};

const DEMO_DAYS: u64 = 7;

struct AppState {
    client: Client,
    demo: bool,
    /// Live mode without writes: selecting a dish is refused instead of sent to NTFY.
    read_only: bool,
    /// Demo mode keeps the menu and the selections in memory.
    demo_days: Mutex<Option<Vec<Day>>>,
}

type Shared = Arc<AppState>;

struct AppError(anyhow::Error);

impl<E: Into<anyhow::Error>> From<E> for AppError {
    fn from(e: E) -> Self {
        Self(e.into())
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        eprintln!("error: {:#}", self.0);
        (StatusCode::BAD_GATEWAY, Json(json!({ "error": format!("{:#}", self.0) }))).into_response()
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let demo = std::env::args().any(|a| a == "--demo") || std::env::var("NFTY_DEMO").is_ok_and(|v| v == "1");
    let read_only = std::env::var("NFTY_READONLY").is_ok_and(|v| v == "1");
    let creds = if demo { None } else { Some(load_credentials()?) };
    let state = Arc::new(AppState { client: Client::new(creds)?, demo, read_only, demo_days: Mutex::new(None) });

    if !demo {
        match state.client.check_login().await {
            Ok(()) => println!("logged in to NTFY"),
            Err(e) => eprintln!("warning: {e:#} (the dashboard will retry on each request)"),
        }
    }

    let app = Router::new()
        .route("/", get(|| async { Html(include_str!("../web/index.html")) }))
        .route("/app.js", get(|| async { ([(header::CONTENT_TYPE, "text/javascript")], include_str!("../web/app.js")) }))
        .route("/style.css", get(|| async { ([(header::CONTENT_TYPE, "text/css")], include_str!("../web/style.css")) }))
        .route("/api/days", get(days))
        .route("/api/select", post(select))
        .route("/img/{id}", get(image))
        .with_state(state);

    let bind = std::env::var("NFTY_BIND").unwrap_or_else(|_| "127.0.0.1:8080".to_string());
    let listener = tokio::net::TcpListener::bind(&bind).await?;
    let mode = if demo { " (demo)" } else if read_only { " (read-only)" } else { "" };
    println!("nfty-dashboard{mode} on http://{bind}");
    axum::serve(listener, app).await?;
    Ok(())
}

/// NFTY_EMAIL/NFTY_PASSWORD, or an `email:password` file at NFTY_CREDS (default ./nfty_creds.txt).
fn load_credentials() -> Result<Credentials> {
    if let (Ok(email), Ok(password)) = (std::env::var("NFTY_EMAIL"), std::env::var("NFTY_PASSWORD")) {
        return Ok(Credentials { email, password });
    }
    let path = std::env::var("NFTY_CREDS").unwrap_or_else(|_| "nfty_creds.txt".to_string());
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("no NFTY_EMAIL/NFTY_PASSWORD and cannot read {path} (or run with --demo)"))?;
    let (email, password) = text.trim().split_once(':').context("credentials file must contain email:password")?;
    Ok(Credentials { email: email.to_string(), password: password.to_string() })
}

async fn days(State(state): State<Shared>) -> Result<Json<serde_json::Value>, AppError> {
    let days = if state.demo {
        let mut cache = state.demo_days.lock().await;
        if cache.is_none() {
            *cache = Some(load_demo_days(&state.client).await?);
        }
        cache.clone().unwrap()
    } else {
        let today = chrono::Local::now().date_naive().to_string();
        model::days_from_deliveries(&state.client.deliveries(&today).await?, chrono::Utc::now().fixed_offset())
    };
    Ok(Json(json!({ "demo": state.demo, "read_only": state.read_only && !state.demo, "days": days })))
}

async fn load_demo_days(client: &Client) -> Result<Vec<Day>> {
    let start = chrono::Local::now().date_naive();
    let dates: Vec<String> = (1..=DEMO_DAYS).map(|i| (start + chrono::Days::new(i)).to_string()).collect();
    let menus = futures::future::join_all(dates.iter().map(|d| client.diet_menu(d))).await;
    let mut days = vec![];
    for (i, (date, menu)) in dates.iter().zip(menus).enumerate() {
        if let Some(day) = model::demo_day_from_menu(date, &menu?, i as u64 + 1) {
            days.push(day);
        }
    }
    Ok(days)
}

#[derive(Deserialize)]
struct SelectRequest {
    delivery_id: u64,
    item_id: u64,
    product_id: u64,
}

async fn select(State(state): State<Shared>, Json(req): Json<SelectRequest>) -> Result<Json<serde_json::Value>, AppError> {
    if state.demo {
        let mut cache = state.demo_days.lock().await;
        let meal = cache
            .iter_mut()
            .flatten()
            .flat_map(|d| d.meals.iter_mut())
            .find(|m| m.item_id == req.item_id)
            .context("unknown meal")?;
        meal.selected = Some(req.product_id);
        meal.user_selected = true;
        return Ok(Json(json!({ "ok": true })));
    }
    if state.read_only {
        return Err(anyhow::anyhow!("read-only mode: changes are not sent to NTFY").into());
    }
    state.client.change_item(req.delivery_id, req.item_id, req.product_id).await?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct ImageQuery {
    r: Option<String>,
}

async fn image(State(state): State<Shared>, Path(id): Path<String>, Query(q): Query<ImageQuery>) -> Result<Response, AppError> {
    let (content_type, bytes) = state.client.image(&id, q.r.as_deref().unwrap_or("MEDIUM")).await?;
    Ok(([(header::CONTENT_TYPE, content_type), (header::CACHE_CONTROL, "public, max-age=604800".to_string())], bytes).into_response())
}
