mod db;
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
const SYNC_EVERY: std::time::Duration = std::time::Duration::from_secs(3600);

struct AppState {
    client: Client,
    demo: bool,
    /// Live mode without writes: selecting a dish is refused instead of sent to NTFY.
    read_only: bool,
    /// Demo mode keeps the menu and the selections in memory.
    demo_days: Mutex<Option<Vec<Day>>>,
    /// Local archive of everything seen in the account (live mode only).
    db: Option<db::Db>,
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
    // A missing .env is fine: the variables may come from the real environment.
    let _ = dotenvy::dotenv();
    let demo = std::env::args().any(|a| a == "--demo") || std::env::var("NTFY_DEMO").is_ok_and(|v| v == "1");
    let read_only = std::env::var("NTFY_READONLY").is_ok_and(|v| v == "1");
    let creds = if demo { None } else { Some(load_credentials()?) };
    let db = if demo {
        None
    } else {
        let path = std::env::var("NTFY_DB").unwrap_or_else(|_| "ntfy.db".to_string());
        Some(db::Db::open(&path).with_context(|| format!("cannot open archive {path}"))?)
    };
    let state = Arc::new(AppState { client: Client::new(creds)?, demo, read_only, demo_days: Mutex::new(None), db });

    if !demo {
        match state.client.check_login().await {
            Ok(()) => println!("logged in to NTFY"),
            Err(e) => eprintln!("warning: {e:#} (the dashboard will retry on each request)"),
        }
        let state = state.clone();
        tokio::spawn(async move {
            loop {
                match sync_archive(&state).await {
                    Ok(stats) => println!("archive synced: {stats:?}"),
                    Err(e) => eprintln!("archive sync failed: {e:#}"),
                }
                tokio::time::sleep(SYNC_EVERY).await;
            }
        });
    }

    let app = Router::new()
        .route("/", get(|| async { Html(include_str!("../web/index.html")) }))
        .route("/common.js", get(|| async { ([(header::CONTENT_TYPE, "text/javascript")], include_str!("../web/common.js")) }))
        .route("/app.js", get(|| async { ([(header::CONTENT_TYPE, "text/javascript")], include_str!("../web/app.js")) }))
        .route("/style.css", get(|| async { ([(header::CONTENT_TYPE, "text/css")], include_str!("../web/style.css")) }))
        .route("/history", get(|| async { Html(include_str!("../web/history.html")) }))
        .route("/history.js", get(|| async { ([(header::CONTENT_TYPE, "text/javascript")], include_str!("../web/history.js")) }))
        .route("/api/days", get(days))
        .route("/api/history", get(history))
        .route("/api/sync", post(sync_now))
        .route("/api/export.json", get(export_json))
        .route("/api/export.csv", get(export_csv))
        .route("/api/select", post(select))
        .route("/img/{id}", get(image))
        .with_state(state);

    let bind = std::env::var("NTFY_BIND").unwrap_or_else(|_| "127.0.0.1:8080".to_string());
    let listener = tokio::net::TcpListener::bind(&bind).await?;
    let mode = if demo { " (demo)" } else if read_only { " (read-only)" } else { "" };
    println!("ntfy-dashboard{mode} on http://{bind}");
    axum::serve(listener, app).await?;
    Ok(())
}

/// NTFY_EMAIL and NTFY_PASSWORD, from the environment or `.env`.
fn load_credentials() -> Result<Credentials> {
    let var = |name: &str| {
        std::env::var(name).with_context(|| format!("{name} is not set (see .env.example, or run with --demo)"))
    };
    Ok(Credentials { email: var("NTFY_EMAIL")?, password: var("NTFY_PASSWORD")? })
}

async fn days(State(state): State<Shared>) -> Result<Json<serde_json::Value>, AppError> {
    let days = if state.demo {
        let mut cache = state.demo_days.lock().await;
        if cache.is_none() {
            *cache = Some(load_demo_days(&state.client).await?);
        }
        cache.clone().unwrap()
    } else {
        let today = today();
        let data = state.client.deliveries(Some(&today), Some("TO-BE-REALIZED")).await?;
        let mut days = model::days_from_deliveries(&data, chrono::Utc::now().fixed_offset());
        if let Some(db) = &state.db {
            db.store_deliveries(&data)?;
            model::attach_history(&mut days, &db.dish_history(&yesterday())?);
        }
        days
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
    from_product_id: Option<u64>,
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
    if let Some(db) = &state.db {
        db.record_selection(req.item_id, req.from_product_id, req.product_id)?;
    }
    Ok(Json(json!({ "ok": true })))
}

fn today() -> String {
    chrono::Local::now().date_naive().to_string()
}

fn yesterday() -> String {
    (chrono::Local::now().date_naive() - chrono::Days::new(1)).to_string()
}

/// Copies every delivery the API still returns (about three past days and all future ones),
/// plus diets and orders, into the local archive.
async fn sync_archive(state: &AppState) -> Result<db::SyncStats> {
    let db = state.db.as_ref().context("no archive in demo mode")?;
    let mut stats = db.store_deliveries(&state.client.deliveries(None, None).await?)?;
    stats.diets = db.store_diets(&state.client.delivery_diets().await?)?;
    stats.orders = db.store_orders(&state.client.orders().await?)?;
    Ok(stats)
}

fn archive(state: &AppState) -> Result<&db::Db, AppError> {
    state.db.as_ref().ok_or_else(|| anyhow::anyhow!("history is not available in demo mode").into())
}

async fn history(State(state): State<Shared>) -> Result<Json<serde_json::Value>, AppError> {
    Ok(Json(archive(&state)?.history(&today())?))
}

async fn sync_now(State(state): State<Shared>) -> Result<Json<serde_json::Value>, AppError> {
    let stats = sync_archive(&state).await?;
    Ok(Json(json!({ "meals": stats.meals, "products": stats.products, "orders": stats.orders, "diets": stats.diets })))
}

async fn export_json(State(state): State<Shared>) -> Result<Response, AppError> {
    let body = serde_json::to_string_pretty(&archive(&state)?.export()?)?;
    Ok(download("application/json", "ntfy-archive.json", body))
}

/// One row per eaten meal with every nutrient, for spreadsheets.
async fn export_csv(State(state): State<Shared>) -> Result<Response, AppError> {
    const COLUMNS: &[&str] = &[
        "date", "meal", "dish", "kcal", "protein_g", "carb_g", "fat_g", "saturated_fat_g", "sugar_g", "fiber_g",
        "salt_g", "weight_g", "kj", "price_pln", "your_rating", "your_comment", "chosen_by_you", "ntfy_rating",
        "ntfy_ratings_count", "allergens", "may_contain", "composition",
    ];
    let history = archive(&state)?.history(&today())?;
    let mut out = COLUMNS.join(",") + "\n";
    for m in history["meals"].as_array().into_iter().flatten() {
        let p = &m["product"];
        let allergens: Vec<&str> = p["allergens"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|a| a["tag_value"]["value"].as_str())
            .collect();
        let cells = [
            m["date"].clone(), m["meal_name"].clone(), p["name"].clone(), p["calorific"].clone(), p["protein"].clone(),
            p["carb"].clone(), p["fat"].clone(), p["saturated_fat"].clone(), p["sugar"].clone(), p["fiber"].clone(),
            p["salt"].clone(), p["weight"].clone(), p["kj"].clone(), m["price"].clone(), m["rating"].clone(),
            m["rating_comment"].clone(), m["user_selected"].clone(), p["rating"].clone(), p["number_of_ratings"].clone(),
            json!(allergens.join("; ")), p["cross_allergen_label"].clone(), p["composition"].clone(),
        ];
        let row: Vec<String> = cells.iter().map(csv_cell).collect();
        out += &(row.join(",") + "\n");
    }
    Ok(download("text/csv; charset=utf-8", "ntfy-meals.csv", out))
}

fn csv_cell(v: &serde_json::Value) -> String {
    let s = match v {
        serde_json::Value::Null => return String::new(),
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    if s.contains([',', '"', '\n']) { format!("\"{}\"", s.replace('"', "\"\"")) } else { s }
}

fn download(content_type: &str, filename: &str, body: String) -> Response {
    (
        [
            (header::CONTENT_TYPE, content_type.to_string()),
            (header::CONTENT_DISPOSITION, format!("attachment; filename=\"{filename}\"")),
        ],
        body,
    )
        .into_response()
}

#[derive(Deserialize)]
struct ImageQuery {
    r: Option<String>,
}

async fn image(State(state): State<Shared>, Path(id): Path<String>, Query(q): Query<ImageQuery>) -> Result<Response, AppError> {
    let (content_type, bytes) = state.client.image(&id, q.r.as_deref().unwrap_or("MEDIUM")).await?;
    Ok(([(header::CONTENT_TYPE, content_type), (header::CACHE_CONTROL, "public, max-age=604800".to_string())], bytes).into_response())
}
