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

#[derive(Clone)]
pub struct Credentials {
    pub email: String,
    pub password: String,
}

#[derive(Clone)]
struct Session {
    token: String,
    refresh_token: String,
    user_id: u64,
}

pub struct Client {
    http: reqwest::Client,
    creds: Option<Credentials>,
    session: Mutex<Option<Session>>,
}

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
    pub fn new(creds: Option<Credentials>) -> Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent("nfty-dashboard")
            .timeout(std::time::Duration::from_secs(45))
            .build()?;
        Ok(Self { http, creds, session: Mutex::new(None) })
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

    async fn login(&self) -> Result<Session> {
        let creds = self.creds.as_ref().ok_or_else(|| anyhow!("no credentials configured"))?;
        let body = json!({ "email": creds.email, "password": creds.password });
        let value = Self::send_json(self.request(reqwest::Method::POST, "sessions").json(&body))
            .await
            .context("login failed")?;
        session_from(value)
    }

    async fn refresh(&self, refresh_token: &str) -> Result<Session> {
        let body = json!({ "refresh_token": refresh_token });
        session_from(Self::send_json(self.request(reqwest::Method::PATCH, "sessions").json(&body)).await?)
    }

    async fn session(&self) -> Result<Session> {
        let mut guard = self.session.lock().await;
        if guard.is_none() {
            *guard = Some(self.login().await?);
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
                    Err(_) => self.login().await?,
                };
                *self.session.lock().await = Some(renewed.clone());
                let req = self.request(method, &path(renewed.user_id)).bearer_auth(&renewed.token);
                Self::send_json(build(req)).await
            }
            other => other,
        }
    }

    pub async fn check_login(&self) -> Result<()> {
        self.session().await.map(|_| ())
    }

    /// Upcoming deliveries with their items and alternative meals (`data` of the response).
    pub async fn deliveries(&self, date_from: &str) -> Result<Value> {
        let query = [
            ("date__gte", date_from.to_string()),
            ("status__in", "TO-BE-REALIZED".to_string()),
            ("expansions__in", DELIVERY_EXPANSIONS.join(",")),
            ("limit", "100".to_string()),
        ];
        let value = self
            .authed(reqwest::Method::GET, |uid| format!("users/{uid}/deliveries"), |r| r.query(&query))
            .await?;
        Ok(value["data"].clone())
    }

    /// Replaces the product of one delivery item with one of its alternatives.
    pub async fn change_item(&self, delivery_id: u64, item_id: u64, simple_product_id: u64) -> Result<Value> {
        let body = json!({ "simple_product_id": simple_product_id });
        self.authed(
            reqwest::Method::PATCH,
            |uid| format!("users/{uid}/deliveries/{delivery_id}/items/{item_id}"),
            |r| r.json(&body),
        )
        .await
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
    let user_id = jwt_user_id(&data.token)?;
    Ok(Session { token: data.token, refresh_token: data.refresh_token, user_id })
}

/// The user id is only available from the `id` claim of the access token.
fn jwt_user_id(token: &str) -> Result<u64> {
    let payload = token.split('.').nth(1).ok_or_else(|| anyhow!("malformed token"))?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(payload.trim_end_matches('='))?;
    let claims: Value = serde_json::from_slice(&bytes)?;
    match &claims["id"] {
        Value::Number(n) => n.as_u64(),
        Value::String(s) => s.parse().ok(),
        _ => None,
    }
    .ok_or_else(|| anyhow!("token has no user id"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_user_id_from_jwt() {
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(r#"{"id":4242,"first_name":"A"}"#);
        assert_eq!(jwt_user_id(&format!("h.{payload}.s")).unwrap(), 4242);
    }
}
