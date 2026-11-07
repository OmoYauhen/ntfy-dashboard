//! Read-only JSON API for assistants and scripts (`/api/v1/*`, spec at `/api`). Everything comes
//! from the local archive, so past days NTFY no longer returns are included; it is refreshed hourly
//! (see `last_sync` in `/api/v1/status`, and `POST /api/sync` to refresh now).

use std::collections::{BTreeMap, HashMap};

use axum::{
    Json,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode, header},
    response::IntoResponse,
};
use chrono::{Datelike, NaiveDate};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::db::ArchivedMeal;
use crate::{AppError, Shared, archive};

pub const OPENAPI: &str = include_str!("openapi.json");
/// Longest range one request may ask for.
const MAX_RANGE_DAYS: i64 = 400;
const DEFAULT_PAST_DAYS: u64 = 7;
const DEFAULT_FUTURE_DAYS: u64 = 14;
const TOP_DISHES: usize = 10;

pub async fn spec() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "application/json")], OPENAPI)
}

#[derive(Deserialize)]
pub struct RangeQuery {
    from: Option<String>,
    to: Option<String>,
    #[serde(default)]
    alternatives: bool,
}

fn bad_request(message: String) -> AppError {
    AppError::new(StatusCode::BAD_REQUEST, &message)
}

fn parse_date(value: &str, name: &str) -> Result<NaiveDate, AppError> {
    NaiveDate::parse_from_str(value, "%Y-%m-%d").map_err(|_| bad_request(format!("{name} must be YYYY-MM-DD, got {value:?}")))
}

fn today() -> NaiveDate {
    chrono::Local::now().date_naive()
}

/// `from`/`to` with defaults, validated.
fn range(q: &RangeQuery, default_from: NaiveDate, default_to: NaiveDate) -> Result<(NaiveDate, NaiveDate), AppError> {
    let from = q.from.as_deref().map(|d| parse_date(d, "from")).transpose()?.unwrap_or(default_from);
    let to = q.to.as_deref().map(|d| parse_date(d, "to")).transpose()?.unwrap_or(default_to);
    if from > to {
        return Err(bad_request(format!("from ({from}) is after to ({to})")));
    }
    if (to - from).num_days() > MAX_RANGE_DAYS {
        return Err(bad_request(format!("range is limited to {MAX_RANGE_DAYS} days")));
    }
    Ok((from, to))
}

/// Absolute base URL as the client sees it (behind nginx: Host + X-Forwarded-Proto).
fn base_url(headers: &HeaderMap) -> String {
    let get = |name: header::HeaderName| headers.get(name).and_then(|v| v.to_str().ok()).map(str::to_string);
    let host = get(header::HOST).unwrap_or_else(|| "localhost".to_string());
    let scheme = headers.get("x-forwarded-proto").and_then(|v| v.to_str().ok()).unwrap_or("http");
    format!("{scheme}://{host}")
}

fn tag_values(list: &Value) -> Vec<String> {
    list.as_array()
        .into_iter()
        .flatten()
        .filter_map(|t| t["tag_value"]["value"].as_str().map(str::to_string))
        .collect()
}

fn main_image(p: &Value) -> Option<&str> {
    let images = p["images"].as_array()?;
    images.iter().find(|i| i["type"] == "MAIN").or(images.first())?["id"].as_str()
}

/// The compact dish shape used in day listings.
fn dish_summary(p: &Value, base: &str) -> Value {
    json!({
        "id": p["id"],
        "name": p["name"],
        "kcal": p["calorific"],
        "protein_g": p["protein"],
        "carb_g": p["carb"],
        "fat_g": p["fat"],
        "fiber_g": p["fiber"],
        "sugar_g": p["sugar"],
        "salt_g": p["salt"],
        "weight_g": p["weight"],
        "allergens": tag_values(&p["allergens"]),
        "ntfy_rating": p["rating"],
        "image_url": main_image(p).map(|id| format!("{base}/img/{id}?r=MEDIUM-RETINA")),
    })
}

fn day_status(meals: &[&ArchivedMeal], date: NaiveDate) -> &'static str {
    let statuses: Vec<&str> = meals.iter().filter_map(|m| m.status.as_deref()).collect();
    if !statuses.is_empty() && statuses.iter().all(|s| *s == "CANCELED") {
        "cancelled"
    } else if statuses.contains(&"REALIZED") {
        "delivered"
    } else if date < today() {
        // Past, but the archive never saw NTFY confirm it (it had already left the API's window).
        "past"
    } else {
        "planned"
    }
}

fn round(v: f64, digits: i32) -> f64 {
    let f = 10f64.powi(digits);
    (v * f).round() / f
}

/// Sums of nutrients and prices over the meals that have a dish (cancelled meals excluded).
fn totals(meals: &[&ArchivedMeal], products: &HashMap<u64, Value>) -> Value {
    let active: Vec<&&ArchivedMeal> = meals.iter().filter(|m| m.status.as_deref() != Some("CANCELED")).collect();
    let sum = |field: &str| {
        active
            .iter()
            .filter_map(|m| products.get(&m.product_id?)?[field].as_f64())
            .sum::<f64>()
    };
    json!({
        "kcal": round(sum("calorific"), 0),
        "protein_g": round(sum("protein"), 1),
        "carb_g": round(sum("carb"), 1),
        "fat_g": round(sum("fat"), 1),
        "fiber_g": round(sum("fiber"), 1),
        "sugar_g": round(sum("sugar"), 1),
        "salt_g": round(sum("salt"), 2),
        "price_pln": round(active.iter().filter_map(|m| m.price).sum::<f64>(), 2),
    })
}

fn day_json(date: NaiveDate, meals: &[&ArchivedMeal], products: &HashMap<u64, Value>, alternatives: bool, base: &str) -> Value {
    let delivery = &meals[0].delivery;
    let deadline = delivery["products_change_to"].as_str().and_then(crate::model::parse_api_time);
    let menu_from = delivery["show_menu_from"].as_str().and_then(crate::model::parse_api_time);
    let now = chrono::Utc::now().fixed_offset();
    let meal_list: Vec<Value> = meals
        .iter()
        .map(|m| {
            let mut meal = json!({
                "meal": m.meal_name,
                "meal_key": m.meal_key,
                "position": m.position,
                "status": m.status,
                "chosen_by_user": m.user_selected,
                "diet_id": m.delivery_diet_id,
                "dish": m.product_id.and_then(|id| products.get(&id)).map(|p| dish_summary(p, base)),
                "price_pln": m.price,
                "your_rating": m.rating,
                "your_comment": m.rating_comment,
            });
            if alternatives {
                meal["alternatives"] = m
                    .alternative_ids
                    .iter()
                    .filter_map(|id| products.get(id))
                    .map(|p| dish_summary(p, base))
                    .collect();
            }
            meal
        })
        .collect();
    json!({
        "date": date.to_string(),
        "weekday": date.weekday().to_string(),
        "status": day_status(meals, date),
        "menu_published": meals.iter().any(|m| m.product_id.is_some()),
        "menu_published_at": menu_from.map(|d| d.to_rfc3339()),
        "can_change": deadline.is_some_and(|d| d > now),
        "can_change_until": deadline.map(|d| d.to_rfc3339()),
        "totals": totals(meals, products),
        "meals": meal_list,
    })
}

fn load_days(state: &Shared, from: NaiveDate, to: NaiveDate, alternatives: bool, base: &str) -> Result<Vec<Value>, AppError> {
    let db = archive(state)?;
    let meals = db.meals_between(&from.to_string(), &to.to_string())?;
    let mut ids: Vec<u64> = meals.iter().filter_map(|m| m.product_id).collect();
    if alternatives {
        ids.extend(meals.iter().flat_map(|m| m.alternative_ids.iter().copied()));
    }
    ids.sort_unstable();
    ids.dedup();
    let products = db.products(&ids)?;
    let mut by_date: BTreeMap<String, Vec<&ArchivedMeal>> = BTreeMap::new();
    for m in &meals {
        by_date.entry(m.date.clone()).or_default().push(m);
    }
    by_date
        .iter()
        .map(|(date, meals)| Ok(day_json(parse_date(date, "date")?, meals, &products, alternatives, base)))
        .collect()
}

pub async fn status(State(state): State<Shared>) -> Result<Json<Value>, AppError> {
    let account = state.account.lock().unwrap().clone();
    let login_error = state.login_error.lock().unwrap().clone();
    let archive = archive(&state).ok().map(|db| db.status()).transpose()?;
    Ok(Json(json!({
        "connected_to_ntfy": account.is_some(),
        "account": account.map(|a| json!({ "first_name": a.first_name, "user_id": a.user_id })),
        "login_error": login_error,
        "today": today().to_string(),
        "archive": archive,
        "notes": "Data comes from the local archive, synced with NTFY hourly (POST /api/sync forces a sync). \
                  NTFY itself only keeps about 3 past days; older days exist only here.",
    })))
}

pub async fn days(State(state): State<Shared>, headers: HeaderMap, Query(q): Query<RangeQuery>) -> Result<Json<Value>, AppError> {
    let t = today();
    let (from, to) = range(&q, t - chrono::Days::new(DEFAULT_PAST_DAYS), t + chrono::Days::new(DEFAULT_FUTURE_DAYS))?;
    let days = load_days(&state, from, to, q.alternatives, &base_url(&headers))?;
    Ok(Json(json!({ "from": from.to_string(), "to": to.to_string(), "days": days })))
}

pub async fn day(State(state): State<Shared>, headers: HeaderMap, Path(date): Path<String>) -> Result<Json<Value>, AppError> {
    let date = parse_date(&date, "date")?;
    load_days(&state, date, date, true, &base_url(&headers))?
        .pop()
        .map(Json)
        .ok_or_else(|| AppError::new(StatusCode::NOT_FOUND, &format!("no delivery on {date}")))
}

pub async fn dish(State(state): State<Shared>, headers: HeaderMap, Path(id): Path<u64>) -> Result<Json<Value>, AppError> {
    let db = archive(&state)?;
    let p = db
        .products(&[id])?
        .remove(&id)
        .ok_or_else(|| AppError::new(StatusCode::NOT_FOUND, &format!("no dish {id} in the archive")))?;
    let history = p["default_configurable_product_id"]
        .as_u64()
        .and_then(|cid| db.dish_history(&today().to_string()).ok()?.remove(&cid));
    let mut dish = dish_summary(&p, &base_url(&headers));
    let extra = json!({
        "kj": p["kj"],
        "saturated_fat_g": p["saturated_fat"],
        "ntfy_ratings_count": p["number_of_ratings"],
        "composition": p["composition"],
        "may_contain": p["cross_allergen_label"],
        "tags": tag_values(&p["work_name_tags"]),
        "categories": p["categories"].as_array().into_iter().flatten()
            .filter_map(|c| c["value"].as_str()).collect::<Vec<_>>(),
        "same_dish_id": p["default_configurable_product_id"],
        "your_history": history.map(|h| json!({
            "times_eaten": h.eaten, "last_eaten": h.last_eaten, "your_avg_rating": h.your_rating,
        })),
    });
    dish.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
    Ok(Json(dish))
}

pub async fn stats(State(state): State<Shared>, Query(q): Query<RangeQuery>) -> Result<Json<Value>, AppError> {
    let db = archive(&state)?;
    let first = db.status()?["first_day"].as_str().and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok());
    let t = today();
    let (from, to) = range(&q, first.unwrap_or(t).min(t), t)?;
    let meals: Vec<ArchivedMeal> = db
        .meals_between(&from.to_string(), &to.to_string())?
        .into_iter()
        .filter(|m| m.status.as_deref() != Some("CANCELED") && m.product_id.is_some())
        .collect();
    let ids: Vec<u64> = meals.iter().filter_map(|m| m.product_id).collect();
    let products = db.products(&ids)?;

    let mut by_date: BTreeMap<&str, Vec<&ArchivedMeal>> = BTreeMap::new();
    let mut counts: HashMap<String, (u32, Vec<i64>)> = HashMap::new();
    for m in &meals {
        by_date.entry(&m.date).or_default().push(m);
        if let Some(p) = m.product_id.and_then(|id| products.get(&id)) {
            let entry = counts.entry(p["name"].as_str().unwrap_or_default().to_string()).or_default();
            entry.0 += 1;
            entry.1.extend(m.rating);
        }
    }
    let day_totals: Vec<Value> = by_date.values().map(|ms| totals(ms, &products)).collect();
    let n = day_totals.len().max(1) as f64;
    let avg = |field: &str, digits| round(day_totals.iter().filter_map(|t| t[field].as_f64()).sum::<f64>() / n, digits);
    let mut top: Vec<(String, (u32, Vec<i64>))> = counts.into_iter().collect();
    top.sort_by(|a, b| b.1.0.cmp(&a.1.0).then(a.0.cmp(&b.0)));
    let ratings: Vec<i64> = meals.iter().filter_map(|m| m.rating).collect();

    Ok(Json(json!({
        "from": from.to_string(),
        "to": to.to_string(),
        "days": by_date.len(),
        "meals": meals.len(),
        "meals_chosen_by_user": meals.iter().filter(|m| m.user_selected).count(),
        "average_per_day": {
            "kcal": avg("kcal", 0), "protein_g": avg("protein_g", 1), "carb_g": avg("carb_g", 1),
            "fat_g": avg("fat_g", 1), "fiber_g": avg("fiber_g", 1), "sugar_g": avg("sugar_g", 1),
            "salt_g": avg("salt_g", 2), "price_pln": avg("price_pln", 2),
        },
        "total_price_pln": round(day_totals.iter().filter_map(|t| t["price_pln"].as_f64()).sum(), 2),
        "your_average_rating": (!ratings.is_empty())
            .then(|| round(ratings.iter().sum::<i64>() as f64 / ratings.len() as f64, 2)),
        "most_eaten": top.iter().take(TOP_DISHES).map(|(name, (count, rs))| json!({
            "name": name,
            "times": count,
            "your_avg_rating": (!rs.is_empty()).then(|| round(rs.iter().sum::<i64>() as f64 / rs.len() as f64, 1)),
        })).collect::<Vec<_>>(),
    })))
}

pub async fn orders(State(state): State<Shared>) -> Result<Json<Value>, AppError> {
    let db = archive(&state)?;
    let diets: Vec<Value> = db
        .diets()?
        .iter()
        .map(|d| json!({
            "id": d["id"], "name": d["user_diet_name"], "kcal": d["kcal"], "status": d["status"],
            "first_day": d["first_delivery_day"], "last_day": d["last_delivery_day"],
            "days": d["diet_days_count"], "delivered_days": d["realized_count"],
        }))
        .collect();
    let orders: Vec<Value> = db
        .orders()?
        .iter()
        .map(|o| json!({
            "id": o["id"], "placed_at": o["created_at"], "status": o["status"],
            "first_delivery": o["first_delivery_date"], "last_delivery": o["last_delivery_date"],
            "delivery_days": o["delivery_days_count"],
            "meals_price_pln": o["products_price"], "delivery_price_pln": o["delivery_price"],
            "discount_pln": o["discount_amount"], "total_pln": o["total_price_after_discount"],
        }))
        .collect();
    Ok(Json(json!({ "diets": diets, "orders": orders })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn spec_is_valid_json_and_documents_every_route() {
        let spec: Value = serde_json::from_str(OPENAPI).unwrap();
        assert_eq!(spec["openapi"], "3.1.0");
        let paths: Vec<&str> = spec["paths"].as_object().unwrap().keys().map(String::as_str).collect();
        let main = include_str!("main.rs");
        for path in &paths {
            assert!(main.contains(&format!("\"{path}\"")), "{path} is documented but not routed");
        }
        for routed in main.lines().filter_map(|l| l.trim().strip_prefix(".route(\"/api/v1")) {
            let path = format!("/api/v1{}", routed.split('"').next().unwrap());
            assert!(paths.contains(&path.as_str()), "{path} is routed but not documented");
        }
    }

    fn state_with_archive() -> Shared {
        let db = crate::db::Db::memory().unwrap();
        let product = |id: u64, name: &str, kcal: f64| json!({
            "id": id, "name": name, "calorific": kcal, "protein": 10.0, "carb": 20.0, "fat": 5.0,
            "allergens": [{"tag_value": {"value": "Mleko"}}], "images": [{"id": format!("{id}.jpg"), "type": "MAIN"}],
            "default_configurable_product_id": id * 10,
        });
        db.store_deliveries(&json!({
            "results": [
                {"id": 1, "date": "2020-01-02", "status": "REALIZED", "products_change_to": "2019-12-31 16:00:00"},
                {"id": 2, "date": "2999-01-05", "status": "TO-BE-REALIZED", "products_change_to": "2999-01-03 16:00:00"}
            ],
            "includes": {
                "delivery_items": [
                    {"id": 10, "delivery_id": 1, "simple_product_id": 100, "diet_variant_meal_id": 500, "status": "REALIZED",
                     "is_simple_product_selected_by_user": true, "price_after_discount": 18.5, "rating_value": 5},
                    {"id": 11, "delivery_id": 1, "simple_product_id": 101, "diet_variant_meal_id": 501, "status": "CANCELED"},
                    {"id": 20, "delivery_id": 2, "simple_product_id": 101, "diet_variant_meal_id": 500, "status": "TO-BE-REALIZED"}
                ],
                "alternative_meals": [{"delivery_item_id": 20, "simple_product_ids": [101, 100]}],
                "simple_products": [product(100, "Owsianka", 350.0), product(101, "Omlet", 400.0)],
                "diet_variant_meals": [{"id": 500, "diet_variant_meal_type_id": 1}, {"id": 501, "diet_variant_meal_type_id": 3}],
                "diet_variant_meal_types": [
                    {"id": 1, "position": 1, "meal_name": {"key": "BREAKFAST", "value": "Śniadanie"}},
                    {"id": 3, "position": 3, "meal_name": {"key": "LUNCH", "value": "Obiad"}}
                ]
            }
        })).unwrap();
        Arc::new(crate::AppState {
            client: crate::orion::Client::offline(),
            demo: false,
            read_only: false,
            demo_days: tokio::sync::Mutex::new(None),
            db: std::sync::Mutex::new(Some(Arc::new(db))),
            account: std::sync::Mutex::new(None),
            login_error: std::sync::Mutex::new(None),
            images: crate::images::ImageCache::new(std::env::temp_dir().join("ntfy-dashboard-test-images")).unwrap(),
        })
    }

    #[test]
    fn days_have_status_totals_and_alternatives() {
        let state = state_with_archive();
        let from = NaiveDate::from_ymd_opt(2020, 1, 1).unwrap();
        let to = NaiveDate::from_ymd_opt(2999, 12, 31).unwrap();
        let days = load_days(&state, from, to, true, "http://ntfy.test").unwrap();
        assert_eq!(days.len(), 2);

        let past = &days[0];
        assert_eq!(past["date"], "2020-01-02");
        assert_eq!(past["status"], "delivered");
        assert_eq!(past["can_change"], false);
        // The cancelled lunch doesn't count.
        assert_eq!(past["totals"]["kcal"], 350.0);
        assert_eq!(past["totals"]["price_pln"], 18.5);
        let breakfast = &past["meals"][0];
        assert_eq!(breakfast["meal"], "Śniadanie");
        assert_eq!(breakfast["dish"]["name"], "Owsianka");
        assert_eq!(breakfast["dish"]["allergens"][0], "Mleko");
        assert_eq!(breakfast["dish"]["image_url"], "http://ntfy.test/img/100.jpg?r=MEDIUM-RETINA");
        assert_eq!(breakfast["your_rating"], 5);

        let future = &days[1];
        assert_eq!(future["status"], "planned");
        assert_eq!(future["can_change"], true);
        assert_eq!(future["can_change_until"], "2999-01-03T16:00:00+01:00");
        let names: Vec<&str> = future["meals"][0]["alternatives"].as_array().unwrap()
            .iter().map(|d| d["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["Omlet", "Owsianka"]);

        let without = load_days(&state, from, to, false, "http://ntfy.test").unwrap();
        assert!(without[1]["meals"][0].get("alternatives").is_none());
    }

    #[test]
    fn ranges_are_validated() {
        let q = |from: &str, to: &str| RangeQuery { from: Some(from.into()), to: Some(to.into()), alternatives: false };
        let d = NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();
        assert!(range(&q("2026-01-02", "2026-01-01"), d, d).is_err());
        assert!(range(&q("2026-01-01", "2028-01-01"), d, d).is_err());
        assert!(range(&q("2026-13-01", "2026-01-01"), d, d).is_err());
        assert!(range(&q("2026-01-01", "2026-03-01"), d, d).is_ok());
    }
}

#[cfg(test)]
mod real_archive {
    //! `NTFY_TEST_DB=/path/to/ntfy.db cargo test real_archive -- --ignored`: runs the endpoints
    //! over a real archive and writes the responses to $NTFY_TEST_OUT for schema checks.
    use super::*;
    use std::sync::Arc;

    #[tokio::test]
    #[ignore]
    async fn dump_responses() {
        let path = std::env::var("NTFY_TEST_DB").unwrap();
        let out = std::path::PathBuf::from(std::env::var("NTFY_TEST_OUT").unwrap());
        std::fs::create_dir_all(&out).unwrap();
        let state: Shared = Arc::new(crate::AppState {
            client: crate::orion::Client::offline(),
            demo: false,
            read_only: false,
            demo_days: tokio::sync::Mutex::new(None),
            db: std::sync::Mutex::new(Some(Arc::new(crate::db::Db::open(&path).unwrap()))),
            account: std::sync::Mutex::new(None),
            login_error: std::sync::Mutex::new(None),
            images: crate::images::ImageCache::new(std::env::temp_dir().join("ntfy-dashboard-test-images")).unwrap(),
        });
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, "ntfy.yauhen.io".parse().unwrap());
        let all = || RangeQuery { from: Some("2026-01-01".into()), to: Some("2026-12-31".into()), alternatives: true };
        let write = |name: &str, v: &Value| std::fs::write(out.join(name), serde_json::to_string_pretty(v).unwrap()).unwrap();

        write("status.json", &status(State(state.clone())).await.unwrap().0);
        let days_all = days(State(state.clone()), headers.clone(), Query(all())).await.unwrap().0;
        write("days.json", &days_all);
        let date = days_all["days"][0]["date"].as_str().unwrap().to_string();
        write("day.json", &day(State(state.clone()), headers.clone(), Path(date)).await.unwrap().0);
        let dish_id = days_all["days"][0]["meals"][0]["dish"]["id"].as_u64().unwrap();
        write("dish.json", &dish(State(state.clone()), headers.clone(), Path(dish_id)).await.unwrap().0);
        write("stats.json", &stats(State(state.clone()), Query(all())).await.unwrap().0);
        write("orders.json", &orders(State(state.clone())).await.unwrap().0);
    }
}
