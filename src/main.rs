mod db;
mod images;
mod model;
mod orion;

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode, header},
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
const SESSION_COOKIE: &str = "ntfy_session";
/// How long a browser stays logged in (unless the server restarts or someone logs out).
const SESSION_DAYS: u64 = 30;
/// Failed logins allowed per client (and in total) per window, so the dashboard can't be used to
/// guess NTFY passwords, nor to get the owner's NTFY account locked.
const LOGIN_WINDOW: std::time::Duration = std::time::Duration::from_secs(15 * 60);
const LOGIN_FAILURES_PER_CLIENT: usize = 5;
const LOGIN_FAILURES_TOTAL: usize = 20;

struct AppState {
    client: Client,
    demo: bool,
    /// Live mode without writes: selecting a dish is refused instead of sent to NTFY.
    read_only: bool,
    /// Demo mode keeps the menu and the selections in memory.
    demo_days: Mutex<Option<Vec<Day>>>,
    /// Directory for the per-account archives (`ntfy-<user id>.db`).
    data_dir: PathBuf,
    /// Archive of the logged-in account.
    db: std::sync::Mutex<Option<Arc<db::Db>>>,
    /// Browser sessions (cookie values) of the logged-in account. Memory only.
    sessions: std::sync::Mutex<HashSet<String>>,
    account: std::sync::Mutex<Option<orion::Account>>,
    /// NTFY user ids allowed to log in; empty allows everyone.
    allowed_users: HashSet<u64>,
    /// Recent failed logins: (when, client).
    failed_logins: std::sync::Mutex<Vec<(std::time::Instant, String)>>,
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
            StatusCode::UNAUTHORIZED
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

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        if self.status != StatusCode::UNAUTHORIZED {
            eprintln!("error: {:#}", self.error);
        }
        let body = json!({ "error": format!("{:#}", self.error), "login_required": self.status == StatusCode::UNAUTHORIZED });
        (self.status, Json(body)).into_response()
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
    let allowed_users = std::env::var("NTFY_ALLOWED_USERS")
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.parse().with_context(|| format!("NTFY_ALLOWED_USERS: {s} is not a user id")))
        .collect::<Result<HashSet<u64>>>()?;
    if !demo && allowed_users.is_empty() {
        eprintln!("warning: NTFY_ALLOWED_USERS is not set, so any NTFY account can log in");
    }
    let state = Arc::new(AppState {
        client: Client::new()?,
        demo,
        read_only,
        demo_days: Mutex::new(None),
        data_dir,
        db: std::sync::Mutex::new(None),
        sessions: std::sync::Mutex::new(HashSet::new()),
        account: std::sync::Mutex::new(None),
        allowed_users,
        failed_logins: std::sync::Mutex::new(Vec::new()),
        images,
    });

    if !demo {
        // Archive everything hourly while someone is logged in.
        let state = state.clone();
        tokio::spawn(async move {
            loop {
                if state.client.is_logged_in() {
                    sync_and_prefetch(&state).await;
                }
                tokio::time::sleep(SYNC_EVERY).await;
            }
        });
    }

    let app = Router::new()
        .route("/", get(|| async { Html(include_str!("../web/index.html")) }))
        .route("/login", get(|| async { Html(include_str!("../web/login.html")) }))
        .route("/history", get(|| async { Html(include_str!("../web/history.html")) }))
        .route("/common.js", get(|| async { ([(header::CONTENT_TYPE, "text/javascript")], include_str!("../web/common.js")) }))
        .route("/app.js", get(|| async { ([(header::CONTENT_TYPE, "text/javascript")], include_str!("../web/app.js")) }))
        .route("/login.js", get(|| async { ([(header::CONTENT_TYPE, "text/javascript")], include_str!("../web/login.js")) }))
        .route("/history.js", get(|| async { ([(header::CONTENT_TYPE, "text/javascript")], include_str!("../web/history.js")) }))
        .route("/style.css", get(|| async { ([(header::CONTENT_TYPE, "text/css")], include_str!("../web/style.css")) }))
        .route("/api/session", get(session))
        .route("/api/login", post(login))
        .route("/api/logout", post(logout))
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

fn session_cookie(headers: &HeaderMap) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .find_map(|c| c.trim().strip_prefix(SESSION_COOKIE)?.strip_prefix('=').map(str::to_string))
}

/// Every account-related endpoint requires a browser session created by `/api/login`.
fn authorize(state: &AppState, headers: &HeaderMap) -> Result<(), AppError> {
    let valid = state.demo
        || session_cookie(headers).is_some_and(|token| state.sessions.lock().unwrap().contains(&token));
    if valid { Ok(()) } else { Err(AppError::new(StatusCode::UNAUTHORIZED, "please log in")) }
}

async fn session(State(state): State<Shared>, headers: HeaderMap) -> Json<serde_json::Value> {
    let logged_in = authorize(&state, &headers).is_ok();
    let name = state.account.lock().unwrap().as_ref().map(|a| a.first_name.clone());
    Json(json!({ "demo": state.demo, "logged_in": logged_in, "name": if logged_in { name } else { None } }))
}

#[derive(Deserialize)]
struct LoginRequest {
    email: String,
    password: String,
}

/// The client address as seen by the reverse proxy in front (its own entry is the last one), or
/// "direct" when there is no proxy.
fn client_id(headers: &HeaderMap) -> String {
    headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.rsplit(',').next())
        .map(|ip| ip.trim().to_string())
        .unwrap_or_else(|| "direct".to_string())
}

/// Too many recent failed logins from this client, or overall.
fn login_throttled(state: &AppState, client: &str) -> bool {
    let mut failures = state.failed_logins.lock().unwrap();
    failures.retain(|(at, _)| at.elapsed() < LOGIN_WINDOW);
    failures.len() >= LOGIN_FAILURES_TOTAL || failures.iter().filter(|(_, c)| c == client).count() >= LOGIN_FAILURES_PER_CLIENT
}

async fn login(State(state): State<Shared>, headers: HeaderMap, Json(req): Json<LoginRequest>) -> Result<Response, AppError> {
    if state.demo {
        return Err(AppError::new(StatusCode::BAD_REQUEST, "the demo needs no login"));
    }
    let client = client_id(&headers);
    if login_throttled(&state, &client) {
        return Err(AppError::new(StatusCode::TOO_MANY_REQUESTS, "Too many failed logins. Try again in 15 minutes."));
    }
    let creds = Credentials { email: req.email.trim().to_string(), password: req.password.clone() };
    let allowed = |account: &orion::Account| state.allowed_users.is_empty() || state.allowed_users.contains(&account.user_id);
    let account = match state.client.login(creds, allowed).await {
        Ok(account) => account,
        Err(e) => {
            state.failed_logins.lock().unwrap().push((std::time::Instant::now(), client.clone()));
            eprintln!("failed login from {client}: {e:#}");
            // Slow down guessing; NTFY may lock the account after many failures.
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            let rejected = e.downcast_ref::<orion::ApiError>().is_some_and(|e| e.status == 401);
            return Err(if rejected {
                AppError::new(StatusCode::UNAUTHORIZED, "Wrong email or password.")
            } else if e.downcast_ref::<orion::NotAllowed>().is_some() {
                AppError::new(StatusCode::FORBIDDEN, "This NTFY account is not allowed on this dashboard.")
            } else {
                e.into()
            });
        }
    };

    let switched = state.account.lock().unwrap().as_ref().is_some_and(|a| a.user_id != account.user_id);
    if switched || state.db.lock().unwrap().is_none() {
        *state.db.lock().unwrap() = Some(Arc::new(open_archive(&state.data_dir, account.user_id)?));
    }
    let token = uuid::Uuid::new_v4().simple().to_string();
    {
        let mut sessions = state.sessions.lock().unwrap();
        if switched {
            sessions.clear(); // other browsers were logged in to the previous account
        }
        sessions.insert(token.clone());
    }
    println!("logged in to NTFY as user {}", account.user_id);
    *state.account.lock().unwrap() = Some(account.clone());

    let background = state.clone();
    tokio::spawn(async move { sync_and_prefetch(&background).await });

    let cookie = format!(
        "{SESSION_COOKIE}={token}; Path=/; HttpOnly; SameSite=Strict; Max-Age={}{}",
        SESSION_DAYS * 86400,
        secure_suffix(&headers)
    );
    Ok(([(header::SET_COOKIE, cookie)], Json(json!({ "name": account.first_name }))).into_response())
}

/// Behind an HTTPS proxy the cookie must only ever travel over HTTPS.
fn secure_suffix(headers: &HeaderMap) -> &'static str {
    let https = headers.get("x-forwarded-proto").and_then(|v| v.to_str().ok()) == Some("https");
    if https { "; Secure" } else { "" }
}

/// Forgets the credentials, the NTFY session and every browser session.
async fn logout(State(state): State<Shared>, headers: HeaderMap) -> Result<Response, AppError> {
    authorize(&state, &headers)?;
    state.client.logout().await;
    state.sessions.lock().unwrap().clear();
    state.account.lock().unwrap().take();
    state.db.lock().unwrap().take();
    println!("logged out");
    let cookie = format!("{SESSION_COOKIE}=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0{}", secure_suffix(&headers));
    Ok(([(header::SET_COOKIE, cookie)], Json(json!({ "ok": true }))).into_response())
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
    state.db.lock().unwrap().clone().ok_or_else(|| AppError::new(StatusCode::UNAUTHORIZED, "please log in"))
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

async fn days(State(state): State<Shared>, headers: HeaderMap) -> Result<Json<serde_json::Value>, AppError> {
    authorize(&state, &headers)?;
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
        let today = today();
        let data = state.client.deliveries(Some(&today), Some("TO-BE-REALIZED")).await?;
        let mut days = model::days_from_deliveries(&data, chrono::Utc::now().fixed_offset());
        let db = archive(&state)?;
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

async fn select(
    State(state): State<Shared>,
    headers: HeaderMap,
    Json(req): Json<SelectRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    authorize(&state, &headers)?;
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

async fn history(State(state): State<Shared>, headers: HeaderMap) -> Result<Json<serde_json::Value>, AppError> {
    authorize(&state, &headers)?;
    Ok(Json(archive(&state)?.history(&today())?))
}

async fn sync_now(State(state): State<Shared>, headers: HeaderMap) -> Result<Json<serde_json::Value>, AppError> {
    authorize(&state, &headers)?;
    let (stats, image_ids) = sync_archive(&state).await?;
    let background = state.clone();
    tokio::spawn(async move { background.images.prefetch(&background.client, image_ids).await });
    Ok(Json(json!({ "meals": stats.meals, "products": stats.products, "orders": stats.orders, "diets": stats.diets })))
}

async fn export_json(State(state): State<Shared>, headers: HeaderMap) -> Result<Response, AppError> {
    authorize(&state, &headers)?;
    let body = serde_json::to_string_pretty(&archive(&state)?.export()?)?;
    Ok(download("application/json", "ntfy-archive.json", body))
}

/// One row per eaten meal with every nutrient, for spreadsheets.
async fn export_csv(State(state): State<Shared>, headers: HeaderMap) -> Result<Response, AppError> {
    authorize(&state, &headers)?;
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

/// Dish photos are public NTFY content, so they need no session.
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
    fn finds_session_cookie() {
        let mut headers = HeaderMap::new();
        headers.insert(header::COOKIE, "a=1; ntfy_session=abc123; ntfy_session_x=2".parse().unwrap());
        assert_eq!(session_cookie(&headers).as_deref(), Some("abc123"));
        headers.insert(header::COOKIE, "ntfy_session_x=2".parse().unwrap());
        assert_eq!(session_cookie(&headers), None);
    }

    #[test]
    fn client_is_the_address_the_proxy_saw() {
        let mut headers = HeaderMap::new();
        assert_eq!(client_id(&headers), "direct");
        // A client can prepend fake entries; the proxy appends the real one last.
        headers.insert("x-forwarded-for", "1.2.3.4, 203.0.113.9".parse().unwrap());
        assert_eq!(client_id(&headers), "203.0.113.9");
    }
}
