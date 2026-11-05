//! Client for the Nice To Fit You "Orion" backend (https://orion-api.ntfy.pl).
//! See API.md for how the endpoints were discovered.

use anyhow::{Context, Result, anyhow, bail};
use base64::Engine;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::Mutex;

pub const BASE_URL: &str = "https://orion-api.ntfy.pl/api/v2.0/";

/// Expansions the official client requests to get meal choices with each delivery.
const DELIVERY_EXPANSIONS: &[&str] = &[
    "delivery_items",
    "delivery_items.simple_products",
    "delivery_items.diet_variant_meals",
    "delivery_items.diet_variant_meals.diet_variant_meal_types",
    "delivery_items.alternative_meals",
];

const MENU_EXPANSIONS: &[&str] = &[
    "diet_variant_meal_types",
    "configurable_products",
    "simple_products",
    "diet_variant_meals",
    "diets",
];

/// Login data, kept only in memory and wiped when dropped.
#[derive(Clone, zeroize::Zeroize, zeroize::ZeroizeOnDrop)]
pub struct Credentials {
    pub email: String,
    pub password: String,
}

#[derive(Clone)]
struct Session {
    token: String,
    refresh_token: String,
    user_id: u64,
    first_name: String,
}

/// The NTFY account the client is logged in as.
#[derive(Clone, Debug)]
pub struct Account {
    pub user_id: u64,
    pub first_name: String,
}

pub struct Client {
    http: reqwest::Client,
    /// Set by `login`, cleared by `logout`; used to log in again when the NTFY session expires.
    creds: std::sync::Mutex<Option<Credentials>>,
    session: Mutex<Option<Session>>,
}

/// No one is logged in (or they logged out).
#[derive(Debug)]
pub struct NotLoggedIn;

impl std::fmt::Display for NotLoggedIn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("not logged in")
    }
}

impl std::error::Error for NotLoggedIn {}

/// The credentials work, but the account is not on the dashboard's allowlist.
#[derive(Debug)]
pub struct NotAllowed;

impl std::fmt::Display for NotAllowed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("account not allowed")
    }
}

impl std::error::Error for NotAllowed {}

#[derive(Deserialize)]
struct Envelope<T> {
    data: T,
}

#[derive(Deserialize)]
struct SessionData {
    token: String,
    refresh_token: String,
}

impl Client {
    pub fn new() -> Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent("ntfy-dashboard")
            .timeout(std::time::Duration::from_secs(45))
            .build()?;
        Ok(Self { http, creds: std::sync::Mutex::new(None), session: Mutex::new(None) })
    }

    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        // The API rejects requests without a Trace-Id and an explicit language.
        self.http
            .request(method, format!("{BASE_URL}{}", path.trim_start_matches('/')))
            .header("Trace-Id", uuid::Uuid::new_v4().to_string())
            .header("Api-Language", "pl")
    }

    async fn send_json(req: reqwest::RequestBuilder) -> Result<Value> {
        let resp = req.send().await?;
        let status = resp.status();
        let body: Value = resp.json().await.unwrap_or(Value::Null);
        if !status.is_success() {
            let msg = body.get("message").and_then(Value::as_str).unwrap_or("");
            bail!(ApiError { status: status.as_u16(), message: msg.to_string() });
        }
        Ok(body)
    }

    async fn sign_in(&self, creds: &Credentials) -> Result<Session> {
        let body = json!({ "email": creds.email, "password": creds.password });
        let value = Self::send_json(self.request(reqwest::Method::POST, "sessions").json(&body))
            .await
            .context("login failed")?;
        session_from(value)
    }

    /// Checks the credentials with NTFY and, if they work for an `allowed` account, keeps them in memory.
    pub async fn login(&self, creds: Credentials, allowed: impl Fn(&Account) -> bool) -> Result<Account> {
        let session = self.sign_in(&creds).await?;
        let account = Account { user_id: session.user_id, first_name: session.first_name.clone() };
        if !allowed(&account) {
            bail!(NotAllowed);
        }
        *self.creds.lock().unwrap() = Some(creds);
        *self.session.lock().await = Some(session);
        Ok(account)
    }

    /// Forgets the credentials and the NTFY session.
    pub async fn logout(&self) {
        self.creds.lock().unwrap().take();
        self.session.lock().await.take();
    }

    pub fn is_logged_in(&self) -> bool {
        self.creds.lock().unwrap().is_some()
    }

    /// Logs in again with the stored credentials (the NTFY session expired).
    async fn relogin(&self) -> Result<Session> {
        let creds = self.creds.lock().unwrap().clone().ok_or(NotLoggedIn)?;
        self.sign_in(&creds).await
    }

    async fn refresh(&self, refresh_token: &str) -> Result<Session> {
        let body = json!({ "refresh_token": refresh_token });
        session_from(Self::send_json(self.request(reqwest::Method::PATCH, "sessions").json(&body)).await?)
    }

    async fn session(&self) -> Result<Session> {
        let mut guard = self.session.lock().await;
        if guard.is_none() {
            *guard = Some(self.relogin().await?);
        }
        Ok(guard.clone().unwrap())
    }

    /// Sends an authorized request, renewing the session once on 401.
    async fn authed(
        &self,
        method: reqwest::Method,
        path: impl Fn(u64) -> String,
        build: impl Fn(reqwest::RequestBuilder) -> reqwest::RequestBuilder,
    ) -> Result<Value> {
        let session = self.session().await?;
        let req = self.request(method.clone(), &path(session.user_id)).bearer_auth(&session.token);
        match Self::send_json(build(req)).await {
            Err(e) if is_unauthorized(&e) => {
                let renewed = match self.refresh(&session.refresh_token).await {
                    Ok(s) => s,
                    Err(_) => self.relogin().await?,
                };
                *self.session.lock().await = Some(renewed.clone());
                let req = self.request(method, &path(renewed.user_id)).bearer_auth(&renewed.token);
                Self::send_json(build(req)).await
            }
            other => other,
        }
    }

    /// Deliveries from `date_from` on (`status` filter optional), with their items and alternative
    /// meals, merged over all pages into one `{results, includes}` object.
    /// Without a status filter the API still only returns roughly the last three days of the past.
    pub async fn deliveries(&self, date_from: Option<&str>, status: Option<&str>) -> Result<Value> {
        const PAGE: usize = 100;
        let mut merged = json!({ "results": [], "includes": {} });
        for page in 0.. {
            let mut query = vec![
                ("expansions__in", DELIVERY_EXPANSIONS.join(",")),
                ("limit", PAGE.to_string()),
                ("offset", (page * PAGE).to_string()),
            ];
            query.extend(date_from.map(|d| ("date__gte", d.to_string())));
            query.extend(status.map(|s| ("status__in", s.to_string())));
            let value = self
                .authed(reqwest::Method::GET, |uid| format!("users/{uid}/deliveries"), |r| r.query(&query))
                .await?;
            let data = &value["data"];
            let results = data["results"].as_array().cloned().unwrap_or_default();
            let count = results.len();
            merged["results"].as_array_mut().unwrap().extend(results);
            for (name, list) in data["includes"].as_object().into_iter().flatten() {
                let target = merged["includes"].as_object_mut().unwrap().entry(name).or_insert_with(|| json!([]));
                if let (Some(target), Some(list)) = (target.as_array_mut(), list.as_array()) {
                    target.extend(list.iter().cloned());
                }
            }
            if count < PAGE {
                break;
            }
        }
        Ok(merged)
    }

    /// The user's diet subscriptions (name, kcal, delivery days).
    pub async fn delivery_diets(&self) -> Result<Vec<Value>> {
        self.list(|uid| format!("users/{uid}/delivery-diets")).await
    }

    /// All orders with prices and delivery dates (no dishes; those are not kept by the API).
    pub async fn orders(&self) -> Result<Vec<Value>> {
        self.list(|uid| format!("users/{uid}/orders")).await
    }

    async fn list(&self, path: impl Fn(u64) -> String) -> Result<Vec<Value>> {
        let value = self.authed(reqwest::Method::GET, path, |r| r.query(&[("limit", "100")])).await?;
        Ok(value["data"]["results"].as_array().cloned().unwrap_or_default())
    }

    /// Replaces the product of one delivery item with one of its alternatives.
    /// Returns the updated item; fails if the API did not take the new product.
    pub async fn change_item(&self, delivery_id: u64, item_id: u64, simple_product_id: u64) -> Result<Value> {
        let body = json!({ "simple_product_id": simple_product_id });
        let value = self
            .authed(
                reqwest::Method::PATCH,
                |uid| format!("users/{uid}/deliveries/{delivery_id}/items/{item_id}"),
                |r| r.json(&body),
            )
            .await?;
        let item = value["data"]["result"].clone();
        if item["simple_product_id"].as_u64() != Some(simple_product_id) {
            bail!("NTFY did not apply the change (item now has product {})", item["simple_product_id"]);
        }
        Ok(item)
    }

    /// Public menu of all diets for a day (`data` of the response).
    pub async fn diet_menu(&self, date: &str) -> Result<Value> {
        let query = [("expansions__in", MENU_EXPANSIONS.join(","))];
        let value = Self::send_json(self.request(reqwest::Method::GET, &format!("diet-menu/{date}")).query(&query)).await?;
        Ok(value["data"].clone())
    }

    pub async fn image(&self, id: &str, resolution: &str) -> Result<(String, bytes::Bytes)> {
        let resp = self
            .request(reqwest::Method::GET, &format!("images/{id}"))
            .query(&[("resolution", resolution)])
            .send()
            .await?
            .error_for_status()?;
        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("image/jpeg")
            .to_string();
        Ok((content_type, resp.bytes().await?))
    }
}

#[derive(Debug)]
pub struct ApiError {
    pub status: u16,
    pub message: String,
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "NTFY API error {}: {}", self.status, self.message)
    }
}

impl std::error::Error for ApiError {}

fn is_unauthorized(e: &anyhow::Error) -> bool {
    e.downcast_ref::<ApiError>().is_some_and(|e| e.status == 401)
}

fn session_from(value: Value) -> Result<Session> {
    let data: SessionData = serde_json::from_value::<Envelope<SessionData>>(value)?.data;
    let claims = jwt_claims(&data.token)?;
    let user_id = match &claims["id"] {
        Value::Number(n) => n.as_u64(),
        Value::String(s) => s.parse().ok(),
        _ => None,
    }
    .ok_or_else(|| anyhow!("token has no user id"))?;
    let first_name = claims["first_name"].as_str().unwrap_or_default().to_string();
    Ok(Session { token: data.token, refresh_token: data.refresh_token, user_id, first_name })
}

/// The user id and name are only available from the claims of the access token.
fn jwt_claims(token: &str) -> Result<Value> {
    let payload = token.split('.').nth(1).ok_or_else(|| anyhow!("malformed token"))?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(payload.trim_end_matches('='))?;
    Ok(serde_json::from_slice(&bytes)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_account_from_jwt() {
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(r#"{"id":4242,"first_name":"A"}"#);
        let token = format!("h.{payload}.s");
        let session = session_from(json!({"data": {"token": token, "refresh_token": "r"}})).unwrap();
        assert_eq!((session.user_id, session.first_name.as_str()), (4242, "A"));
    }
}
