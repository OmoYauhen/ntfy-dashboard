mod agent_api;
mod db;
mod images;
mod model;
mod orion;

use std::path::PathBuf;
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
use zeroize::Zeroize;

use model::Day;
use orion::{Client, Credentials};

const DEMO_DAYS: u64 = 7;
const SYNC_EVERY: std::time::Duration = std::time::Duration::from_secs(3600);
/// Retry delays for the startup login: soon after network errors, but rarely after NTFY rejects
/// the credentials, so a wrong password can't get the account locked.
const RETRY_AFTER_ERROR: std::time::Duration = std::time::Duration::from_secs(60);
const RETRY_AFTER_REJECTION: std::time::Duration = std::time::Duration::from_secs(30 * 60);
/// Name of the systemd credential (`LoadCredential[Encrypted]=`) holding `email:password`.
const SYSTEMD_CREDENTIAL: &str = "ntfy-credentials";

struct AppState {
    client: Client,
    demo: bool,
    /// Live mode without writes: selecting a dish is refused instead of sent to NTFY.
    read_only: bool,
    /// Demo mode keeps the menu and the selections in memory.
    demo_days: Mutex<Option<Vec<Day>>>,
    /// Archive of the account, opened once the startup login succeeded.
    db: std::sync::Mutex<Option<Arc<db::Db>>>,
    account: std::sync::Mutex<Option<orion::Account>>,
    /// Why the dashboard isn't connected to NTFY yet, shown to the user.
    login_error: std::sync::Mutex<Option<String>>,
    images: images::ImageCache,
}

type Shared = Arc<AppState>;

struct AppError {
    status: StatusCode,
    error: anyhow::Error,
}

impl<E: Into<anyhow::Error>> From<E> for AppError {
    fn from(e: E) -> Self {
        let error = e.into();
        let status = if error.downcast_ref::<orion::NotLoggedIn>().is_some() {
            StatusCode::SERVICE_UNAVAILABLE
        } else {
            StatusCode::BAD_GATEWAY
        };
        Self { status, error }
    }
}

impl AppError {
    fn new(status: StatusCode, message: &str) -> Self {
        Self { status, error: anyhow::anyhow!(message.to_string()) }
    }
}

impl std::fmt::Debug for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {:#}", self.status, self.error)
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        eprintln!("error: {:#}", self.error);
        (self.status, Json(json!({ "error": format!("{:#}", self.error) }))).into_response()
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    // A missing .env is fine: the variables may come from the real environment.
    let _ = dotenvy::dotenv();
    let demo = std::env::args().any(|a| a == "--demo") || std::env::var("NTFY_DEMO").is_ok_and(|v| v == "1");
    let read_only = std::env::var("NTFY_READONLY").is_ok_and(|v| v == "1");
    let data_dir = PathBuf::from(std::env::var("NTFY_DATA_DIR").unwrap_or_else(|_| ".".to_string()));
    std::fs::create_dir_all(&data_dir).with_context(|| format!("cannot create {}", data_dir.display()))?;
    let cache_dir = std::env::var("NTFY_IMAGE_CACHE").unwrap_or_else(|_| "cache/images".to_string());
    let images = images::ImageCache::new(&cache_dir).with_context(|| format!("cannot create image cache {cache_dir}"))?;
    let creds = if demo { None } else { Some(load_credentials()?) };
    let state = Arc::new(AppState {
        client: Client::new()?,
        demo,
        read_only,
        demo_days: Mutex::new(None),
        db: std::sync::Mutex::new(None),
        account: std::sync::Mutex::new(None),
        login_error: std::sync::Mutex::new(None),
        images,
    });

    if let Some(creds) = creds {
        // Log in, then archive everything hourly.
        let state = state.clone();
        tokio::spawn(async move {
            connect(&state, creds, &data_dir).await;
            loop {
                sync_and_prefetch(&state).await;
                tokio::time::sleep(SYNC_EVERY).await;
            }
        });
    }

    let app = Router::new()
        .route("/", get(|| async { Html(include_str!("../web/index.html")) }))
        .route("/history", get(|| async { Html(include_str!("../web/history.html")) }))
        .route("/common.js", get(|| async { ([(header::CONTENT_TYPE, "text/javascript")], include_str!("../web/common.js")) }))
        .route("/app.js", get(|| async { ([(header::CONTENT_TYPE, "text/javascript")], include_str!("../web/app.js")) }))
        .route("/history.js", get(|| async { ([(header::CONTENT_TYPE, "text/javascript")], include_str!("../web/history.js")) }))
        .route("/style.css", get(|| async { ([(header::CONTENT_TYPE, "text/css")], include_str!("../web/style.css")) }))
        // Read-only API for assistants; its OpenAPI spec is served at /api.
        .route("/api", get(agent_api::spec))
        .route("/api/v1/status", get(agent_api::status))
        .route("/api/v1/days", get(agent_api::days))
        .route("/api/v1/days/{date}", get(agent_api::day))
        .route("/api/v1/dishes/{id}", get(agent_api::dish))
        .route("/api/v1/stats", get(agent_api::stats))
        .route("/api/v1/orders", get(agent_api::orders))
        .route("/api/session", get(session))
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

/// `email:password` from the systemd credential `ntfy-credentials`, the file at
/// NTFY_CREDENTIALS_FILE, or NTFY_EMAIL + NTFY_PASSWORD.
fn load_credentials() -> Result<Credentials> {
    let file = std::env::var_os("CREDENTIALS_DIRECTORY")
        .map(|dir| PathBuf::from(dir).join(SYSTEMD_CREDENTIAL))
        .filter(|path| path.exists())
        .or_else(|| std::env::var_os("NTFY_CREDENTIALS_FILE").map(PathBuf::from));
    if let Some(path) = file {
        let mut text = std::fs::read_to_string(&path).with_context(|| format!("cannot read {}", path.display()))?;
        let creds = parse_credentials(&text);
        text.zeroize();
        return creds.with_context(|| format!("{} must contain email:password", path.display()));
    }
    match (std::env::var("NTFY_EMAIL"), std::env::var("NTFY_PASSWORD")) {
        (Ok(email), Ok(password)) => Ok(Credentials { email, password }),
        _ => anyhow::bail!(
            "no NTFY credentials: set the systemd credential {SYSTEMD_CREDENTIAL}, NTFY_CREDENTIALS_FILE, \
             or NTFY_EMAIL and NTFY_PASSWORD (or run with --demo)"
        ),
    }
}

fn parse_credentials(text: &str) -> Option<Credentials> {
    let (email, password) = text.trim_end_matches(['\r', '\n']).split_once(':')?;
    let (email, password) = (email.trim(), password);
    (!email.is_empty() && !password.is_empty())
        .then(|| Credentials { email: email.to_string(), password: password.to_string() })
}

/// Logs in to NTFY, retrying until it works, and opens the account's archive.
async fn connect(state: &AppState, creds: Credentials, data_dir: &std::path::Path) {
    loop {
        match state.client.login(creds.clone()).await {
            Ok(account) => {
                match open_archive(data_dir, account.user_id) {
                    Ok(db) => *state.db.lock().unwrap() = Some(Arc::new(db)),
                    Err(e) => eprintln!("error: {e:#}"),
                }
                println!("logged in to NTFY as user {}", account.user_id);
                *state.account.lock().unwrap() = Some(account);
                state.login_error.lock().unwrap().take();
                return;
            }
            Err(e) => {
                let rejected = e.downcast_ref::<orion::ApiError>().is_some_and(|e| e.status == 401);
                let (message, wait) = if rejected {
                    ("NTFY rejected the configured email or password".to_string(), RETRY_AFTER_REJECTION)
                } else {
                    (format!("cannot reach NTFY: {e:#}"), RETRY_AFTER_ERROR)
                };
                eprintln!("login failed: {message}; retrying in {} min", wait.as_secs() / 60);
                *state.login_error.lock().unwrap() = Some(message);
                tokio::time::sleep(wait).await;
            }
        }
    }
}

async fn session(State(state): State<Shared>) -> Json<serde_json::Value> {
    let name = state.account.lock().unwrap().as_ref().map(|a| a.first_name.clone());
    let error = state.login_error.lock().unwrap().clone();
    Json(json!({ "demo": state.demo, "connected": name.is_some(), "name": name, "error": error }))
}

/// Opens `ntfy-<user id>.db`, adopting a pre-login `ntfy.db` if it belongs to this account.
fn open_archive(dir: &std::path::Path, user_id: u64) -> Result<db::Db> {
    let path = dir.join(format!("ntfy-{user_id}.db"));
    let legacy = dir.join("ntfy.db");
    if !path.exists() && legacy.exists() && db::Db::open(&legacy.to_string_lossy())?.owner()? == Some(user_id) {
        std::fs::rename(&legacy, &path)?;
        println!("moved {} to {}", legacy.display(), path.display());
    }
    db::Db::open(&path.to_string_lossy()).with_context(|| format!("cannot open archive {}", path.display()))
}

fn archive(state: &AppState) -> Result<Arc<db::Db>, AppError> {
    if state.demo {
        return Err(AppError::new(StatusCode::NOT_FOUND, "history is not available in demo mode"));
    }
    state.db.lock().unwrap().clone().ok_or_else(|| {
        let reason = state.login_error.lock().unwrap().clone().unwrap_or_else(|| "still logging in".to_string());
        AppError::new(StatusCode::SERVICE_UNAVAILABLE, &format!("Not connected to NTFY yet: {reason}"))
    })
}

async fn sync_and_prefetch(state: &AppState) {
    match sync_archive(state).await {
        Ok((stats, image_ids)) => {
            println!("archive synced: {stats:?}");
            let fetched = state.images.prefetch(&state.client, image_ids).await;
            println!("image cache: {fetched} new images");
        }
        Err(e) => eprintln!("archive sync failed: {e:#}"),
    }
}

async fn days(State(state): State<Shared>) -> Result<Json<serde_json::Value>, AppError> {
    let days = if state.demo {
        let mut cache = state.demo_days.lock().await;
        if cache.is_none() {
            let days = load_demo_days(&state.client).await?;
            let ids = days.iter().flat_map(|d| &d.meals).flat_map(|m| &m.options).filter_map(|o| o.image.clone()).collect();
            let background = state.clone();
            tokio::spawn(async move { background.images.prefetch(&background.client, ids).await });
            *cache = Some(days);
        }
        cache.clone().unwrap()
    } else {
        let db = archive(&state)?;
        let today = today();
        let data = state.client.deliveries(Some(&today), Some("TO-BE-REALIZED")).await?;
        let mut days = model::days_from_deliveries(&data, chrono::Utc::now().fixed_offset());
        db.store_deliveries(&data)?;
        model::attach_history(&mut days, &db.dish_history(&yesterday())?);
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
    archive(&state)?.record_selection(req.item_id, req.from_product_id, req.product_id)?;
    Ok(Json(json!({ "ok": true })))
}

fn today() -> String {
    chrono::Local::now().date_naive().to_string()
}

fn yesterday() -> String {
    (chrono::Local::now().date_naive() - chrono::Days::new(1)).to_string()
}

/// Copies every delivery the API still returns (about three past days and all future ones),
/// plus diets and orders, into the local archive. Also returns the photos of every dish seen.
async fn sync_archive(state: &AppState) -> Result<(db::SyncStats, Vec<String>)> {
    let db = state.db.lock().unwrap().clone().ok_or(orion::NotLoggedIn)?;
    let deliveries = state.client.deliveries(None, None).await?;
    let mut stats = db.store_deliveries(&deliveries)?;
    stats.diets = db.store_diets(&state.client.delivery_diets().await?)?;
    stats.orders = db.store_orders(&state.client.orders().await?)?;
    Ok((stats, images::product_image_ids(&deliveries)))
}

async fn history(State(state): State<Shared>) -> Result<Json<serde_json::Value>, AppError> {
    Ok(Json(archive(&state)?.history(&today())?))
}

async fn sync_now(State(state): State<Shared>) -> Result<Json<serde_json::Value>, AppError> {
    let (stats, image_ids) = sync_archive(&state).await?;
    let background = state.clone();
    tokio::spawn(async move { background.images.prefetch(&background.client, image_ids).await });
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
    let resolution = q.r.as_deref().unwrap_or(images::TILE_RESOLUTION);
    let (content_type, bytes) = state.images.get(&state.client, &id, resolution).await?;
    Ok(([(header::CONTENT_TYPE, content_type), (header::CACHE_CONTROL, "public, max-age=31536000, immutable".to_string())], bytes).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_credentials() {
        let c = parse_credentials("me@example.com:pa:ss word\n").unwrap();
        assert_eq!((c.email.as_str(), c.password.as_str()), ("me@example.com", "pa:ss word"));
        assert!(parse_credentials("no-colon").is_none());
        assert!(parse_credentials("me@example.com:\n").is_none());
    }
}
