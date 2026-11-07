//! Local SQLite archive. The NTFY API forgets deliveries a few days after they happen, so every
//! sync copies everything it sees (raw JSON included) and keeps it after the API drops it.

use std::collections::HashMap;
use std::sync::Mutex;

use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};

pub struct Db(Mutex<Connection>);

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS products (
    id INTEGER PRIMARY KEY,
    configurable_id INTEGER,
    name TEXT NOT NULL,
    json TEXT NOT NULL,
    first_seen TEXT NOT NULL,
    last_seen TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS meals (
    item_id INTEGER PRIMARY KEY,
    delivery_id INTEGER NOT NULL,
    date TEXT NOT NULL,
    delivery_diet_id INTEGER,
    meal_key TEXT,
    meal_name TEXT,
    position INTEGER,
    kcal_variant REAL,
    product_id INTEGER,
    user_selected INTEGER NOT NULL DEFAULT 0,
    status TEXT,
    price REAL,
    rating INTEGER,
    rating_comment TEXT,
    alternative_ids TEXT,
    item_json TEXT NOT NULL,
    delivery_json TEXT NOT NULL,
    first_seen TEXT NOT NULL,
    last_seen TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS meals_date ON meals(date);
CREATE TABLE IF NOT EXISTS diets (
    id INTEGER PRIMARY KEY,
    name TEXT,
    kcal REAL,
    json TEXT NOT NULL,
    last_seen TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS orders (
    id INTEGER PRIMARY KEY,
    json TEXT NOT NULL,
    last_seen TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS selections (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    at TEXT NOT NULL,
    date TEXT,
    item_id INTEGER NOT NULL,
    meal_name TEXT,
    from_product INTEGER,
    to_product INTEGER NOT NULL
);
";

#[derive(Default, Debug)]
pub struct SyncStats {
    pub meals: usize,
    pub products: usize,
    pub orders: usize,
    pub diets: usize,
}

/// One archived meal (delivery item), as stored by the last sync that saw it.
#[derive(Clone, Debug)]
pub struct ArchivedMeal {
    pub date: String,
    pub meal_key: Option<String>,
    pub meal_name: Option<String>,
    pub position: Option<i64>,
    pub product_id: Option<u64>,
    pub user_selected: bool,
    pub status: Option<String>,
    pub price: Option<f64>,
    pub rating: Option<i64>,
    pub rating_comment: Option<String>,
    pub alternative_ids: Vec<u64>,
    pub delivery_diet_id: Option<u64>,
    /// The raw delivery (date, deadlines, address) this meal belongs to.
    pub delivery: Value,
}

/// What we know about the user's past with one dish (matched across calorie sizes).
#[derive(Clone, Debug, serde::Serialize)]
pub struct DishHistory {
    pub eaten: u32,
    pub last_eaten: Option<String>,
    pub your_rating: Option<f64>,
}

fn now() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

fn by_id(includes: &Value, name: &str) -> HashMap<u64, Value> {
    includes[name]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|v| Some((v["id"].as_u64()?, v.clone())))
        .collect()
}

impl Db {
    pub fn open(path: &str) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self(Mutex::new(conn)))
    }

    #[cfg(test)]
    pub fn memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self(Mutex::new(conn)))
    }

    /// NTFY user id the archive belongs to (from the stored diets), if known.
    pub fn owner(&self) -> Result<Option<u64>> {
        let conn = self.0.lock().unwrap();
        let json: Option<String> = conn.query_row("SELECT json FROM diets LIMIT 1", [], |r| r.get(0)).optional()?;
        Ok(json.and_then(|j| serde_json::from_str::<Value>(&j).ok()).and_then(|d| d["user_id"].as_u64()))
    }

    /// Archives a merged `GET users/{id}/deliveries` response (`{results, includes}`).
    pub fn store_deliveries(&self, data: &Value) -> Result<SyncStats> {
        let includes = &data["includes"];
        let variant_meals = by_id(includes, "diet_variant_meals");
        let meal_types = by_id(includes, "diet_variant_meal_types");
        let deliveries = by_id(data, "results");
        let alternatives: HashMap<u64, Value> = includes["alternative_meals"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|a| Some((a["delivery_item_id"].as_u64()?, a["simple_product_ids"].clone())))
            .collect();
        let ts = now();
        let mut stats = SyncStats::default();
        let mut conn = self.0.lock().unwrap();
        let tx = conn.transaction()?;
        for product in includes["simple_products"].as_array().into_iter().flatten() {
            let Some(id) = product["id"].as_u64() else { continue };
            tx.execute(
                "INSERT INTO products (id, configurable_id, name, json, first_seen, last_seen)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?5)
                 ON CONFLICT(id) DO UPDATE SET configurable_id = ?2, name = ?3, json = ?4, last_seen = ?5",
                params![id, product["default_configurable_product_id"].as_u64(), product["name"].as_str().unwrap_or(""), product.to_string(), ts],
            )?;
            stats.products += 1;
        }
        for item in includes["delivery_items"].as_array().into_iter().flatten() {
            let (Some(item_id), Some(delivery_id)) = (item["id"].as_u64(), item["delivery_id"].as_u64()) else { continue };
            let Some(delivery) = deliveries.get(&delivery_id) else { continue };
            let variant = item["diet_variant_meal_id"].as_u64().and_then(|id| variant_meals.get(&id));
            let meal_type = variant.and_then(|v| meal_types.get(&v["diet_variant_meal_type_id"].as_u64()?));
            tx.execute(
                "INSERT INTO meals (item_id, delivery_id, date, delivery_diet_id, meal_key, meal_name, position,
                    kcal_variant, product_id, user_selected, status, price, rating, rating_comment, alternative_ids,
                    item_json, delivery_json, first_seen, last_seen)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?18)
                 ON CONFLICT(item_id) DO UPDATE SET delivery_id = ?2, date = ?3, delivery_diet_id = ?4,
                    meal_key = ?5, meal_name = ?6, position = ?7, kcal_variant = ?8, product_id = ?9,
                    user_selected = ?10, status = ?11, price = ?12, rating = ?13, rating_comment = ?14,
                    alternative_ids = COALESCE(?15, alternative_ids), item_json = ?16, delivery_json = ?17,
                    last_seen = ?18",
                params![
                    item_id,
                    delivery_id,
                    delivery["date"].as_str().unwrap_or("").get(..10).unwrap_or(""),
                    item["delivery_diet_id"].as_u64(),
                    meal_type.and_then(|t| t["meal_name"]["key"].as_str()),
                    meal_type.and_then(|t| t["meal_name"]["value"].as_str()),
                    meal_type.and_then(|t| t["position"].as_i64()),
                    variant.and_then(|v| v["kcal"].as_f64()),
                    item["simple_product_id"].as_u64(),
                    item["is_simple_product_selected_by_user"].as_bool().unwrap_or(false),
                    item["status"].as_str(),
                    item["price_after_discount"].as_f64(),
                    item["rating_value"].as_i64(),
                    item["rating_comment"].as_str(),
                    alternatives.get(&item_id).map(Value::to_string),
                    item.to_string(),
                    delivery.to_string(),
                    ts,
                ],
            )?;
            stats.meals += 1;
        }
        tx.commit()?;
        Ok(stats)
    }

    pub fn store_diets(&self, diets: &[Value]) -> Result<usize> {
        let conn = self.0.lock().unwrap();
        for d in diets {
            conn.execute(
                "INSERT INTO diets (id, name, kcal, json, last_seen) VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(id) DO UPDATE SET name = ?2, kcal = ?3, json = ?4, last_seen = ?5",
                params![d["id"].as_u64(), d["user_diet_name"].as_str(), d["kcal"].as_f64(), d.to_string(), now()],
            )?;
        }
        Ok(diets.len())
    }

    pub fn store_orders(&self, orders: &[Value]) -> Result<usize> {
        let conn = self.0.lock().unwrap();
        for o in orders {
            conn.execute(
                "INSERT INTO orders (id, json, last_seen) VALUES (?1, ?2, ?3)
                 ON CONFLICT(id) DO UPDATE SET json = ?2, last_seen = ?3",
                params![o["id"].as_u64(), o.to_string(), now()],
            )?;
        }
        Ok(orders.len())
    }

    /// Records a choice made in the dashboard and updates the archived meal right away.
    pub fn record_selection(&self, item_id: u64, from: Option<u64>, to: u64) -> Result<()> {
        let conn = self.0.lock().unwrap();
        let meal: Option<(String, Option<String>)> = conn
            .query_row("SELECT date, meal_name FROM meals WHERE item_id = ?1", [item_id], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()?;
        conn.execute(
            "INSERT INTO selections (at, date, item_id, meal_name, from_product, to_product) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![now(), meal.as_ref().map(|m| &m.0), item_id, meal.as_ref().and_then(|m| m.1.as_ref()), from, to],
        )?;
        conn.execute("UPDATE meals SET product_id = ?1, user_selected = 1 WHERE item_id = ?2", params![to, item_id])?;
        Ok(())
    }

    /// Past meals per configurable product (the same dish in any calorie size).
    pub fn dish_history(&self, until: &str) -> Result<HashMap<u64, DishHistory>> {
        let conn = self.0.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT p.configurable_id, COUNT(*), MAX(m.date), AVG(m.rating)
             FROM meals m JOIN products p ON p.id = m.product_id
             WHERE m.date <= ?1 AND m.status != 'CANCELED' AND p.configurable_id IS NOT NULL
             GROUP BY p.configurable_id",
        )?;
        let rows = stmt.query_map([until], |r| {
            Ok((r.get::<_, u64>(0)?, DishHistory { eaten: r.get(1)?, last_eaten: r.get(2)?, your_rating: r.get(3)? }))
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Everything archived for meals up to `until`, newest day first, plus diets and orders.
    pub fn history(&self, until: &str) -> Result<Value> {
        let conn = self.0.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT m.date, m.meal_key, m.meal_name, m.position, m.kcal_variant, m.user_selected, m.status,
                    m.price, m.rating, m.rating_comment, m.delivery_diet_id, m.item_id, p.json, m.alternative_ids
             FROM meals m LEFT JOIN products p ON p.id = m.product_id
             WHERE m.date <= ?1 AND m.status != 'CANCELED'
             ORDER BY m.date DESC, m.position",
        )?;
        let meals = stmt
            .query_map([until], |r| {
                let product: Option<String> = r.get(12)?;
                let alternatives: Option<String> = r.get(13)?;
                Ok(json!({
                    "date": r.get::<_, String>(0)?,
                    "meal_key": r.get::<_, Option<String>>(1)?,
                    "meal_name": r.get::<_, Option<String>>(2)?,
                    "position": r.get::<_, Option<i64>>(3)?,
                    "kcal_variant": r.get::<_, Option<f64>>(4)?,
                    "user_selected": r.get::<_, bool>(5)?,
                    "status": r.get::<_, Option<String>>(6)?,
                    "price": r.get::<_, Option<f64>>(7)?,
                    "rating": r.get::<_, Option<i64>>(8)?,
                    "rating_comment": r.get::<_, Option<String>>(9)?,
                    "delivery_diet_id": r.get::<_, Option<u64>>(10)?,
                    "item_id": r.get::<_, u64>(11)?,
                    "product": product.and_then(|p| serde_json::from_str::<Value>(&p).ok()),
                    "alternative_count": alternatives
                        .and_then(|a| serde_json::from_str::<Value>(&a).ok())
                        .and_then(|a| a.as_array().map(Vec::len)),
                }))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let diets = Self::json_rows(&conn, "SELECT json FROM diets ORDER BY id")?;
        let orders = Self::json_rows(&conn, "SELECT json FROM orders ORDER BY id DESC")?;
        let selections = conn
            .prepare("SELECT at, date, meal_name, from_product, to_product FROM selections ORDER BY id DESC")?
            .query_map([], |r| {
                Ok(json!({ "at": r.get::<_, String>(0)?, "date": r.get::<_, Option<String>>(1)?,
                           "meal_name": r.get::<_, Option<String>>(2)?, "from_product": r.get::<_, Option<u64>>(3)?,
                           "to_product": r.get::<_, u64>(4)? }))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let first_archived: Option<String> = conn.query_row("SELECT MIN(first_seen) FROM meals", [], |r| r.get(0))?;
        Ok(json!({ "meals": meals, "diets": diets, "orders": orders, "selections": selections, "archived_since": first_archived }))
    }

    /// Archived meals (cancelled ones included) with `from <= date <= to`, by date and meal order.
    pub fn meals_between(&self, from: &str, to: &str) -> Result<Vec<ArchivedMeal>> {
        let conn = self.0.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT date, meal_key, meal_name, position, product_id, user_selected, status, price, rating,
                    rating_comment, alternative_ids, delivery_diet_id, delivery_json
             FROM meals WHERE date BETWEEN ?1 AND ?2 ORDER BY date, position, item_id",
        )?;
        let rows = stmt.query_map([from, to], |r| {
            let alternatives: Option<String> = r.get(10)?;
            let delivery: String = r.get(12)?;
            Ok(ArchivedMeal {
                date: r.get(0)?,
                meal_key: r.get(1)?,
                meal_name: r.get(2)?,
                position: r.get(3)?,
                product_id: r.get(4)?,
                user_selected: r.get(5)?,
                status: r.get(6)?,
                price: r.get(7)?,
                rating: r.get(8)?,
                rating_comment: r.get(9)?,
                alternative_ids: alternatives
                    .and_then(|a| serde_json::from_str::<Vec<u64>>(&a).ok())
                    .unwrap_or_default(),
                delivery_diet_id: r.get(11)?,
                delivery: serde_json::from_str(&delivery).unwrap_or(Value::Null),
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Archived dishes (raw NTFY products) by id; unknown ids are left out.
    pub fn products(&self, ids: &[u64]) -> Result<HashMap<u64, Value>> {
        let conn = self.0.lock().unwrap();
        let mut stmt = conn.prepare("SELECT json FROM products WHERE id = ?1")?;
        let mut found = HashMap::new();
        for &id in ids {
            let json: Option<String> = stmt.query_row([id], |r| r.get(0)).optional()?;
            if let Some(product) = json.and_then(|j| serde_json::from_str::<Value>(&j).ok()) {
                found.insert(id, product);
            }
        }
        Ok(found)
    }

    /// Range and freshness of the archive.
    pub fn status(&self) -> Result<Value> {
        let conn = self.0.lock().unwrap();
        let (first, last, meals, synced): (Option<String>, Option<String>, u64, Option<String>) = conn.query_row(
            "SELECT MIN(date), MAX(date), COUNT(*), MAX(last_seen) FROM meals",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )?;
        let dishes: u64 = conn.query_row("SELECT COUNT(*) FROM products", [], |r| r.get(0))?;
        Ok(json!({ "first_day": first, "last_day": last, "meals": meals, "dishes": dishes, "last_sync": synced }))
    }

    pub fn orders(&self) -> Result<Vec<Value>> {
        Self::json_rows(&self.0.lock().unwrap(), "SELECT json FROM orders ORDER BY id DESC")
    }

    pub fn diets(&self) -> Result<Vec<Value>> {
        Self::json_rows(&self.0.lock().unwrap(), "SELECT json FROM diets ORDER BY id")
    }

    /// Complete dump of the archive, raw API JSON included.
    pub fn export(&self) -> Result<Value> {
        let conn = self.0.lock().unwrap();
        let meals = conn
            .prepare("SELECT item_json, delivery_json, meal_name, first_seen, last_seen FROM meals ORDER BY date, position")?
            .query_map([], |r| {
                let item: String = r.get(0)?;
                let delivery: String = r.get(1)?;
                Ok(json!({
                    "item": serde_json::from_str::<Value>(&item).unwrap_or(Value::Null),
                    "delivery": serde_json::from_str::<Value>(&delivery).unwrap_or(Value::Null),
                    "meal_name": r.get::<_, Option<String>>(2)?,
                    "first_seen": r.get::<_, String>(3)?,
                    "last_seen": r.get::<_, String>(4)?,
                }))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(json!({
            "meals": meals,
            "products": Self::json_rows(&conn, "SELECT json FROM products ORDER BY id")?,
            "diets": Self::json_rows(&conn, "SELECT json FROM diets ORDER BY id")?,
            "orders": Self::json_rows(&conn, "SELECT json FROM orders ORDER BY id")?,
        }))
    }

    fn json_rows(conn: &Connection, sql: &str) -> Result<Vec<Value>> {
        let mut stmt = conn.prepare(sql)?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        Ok(rows
            .filter_map(|r| r.ok())
            .filter_map(|s| serde_json::from_str(&s).ok())
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn deliveries() -> Value {
        json!({
            "results": [{"id": 1, "date": "2026-11-02", "status": "REALIZED"}],
            "includes": {
                "delivery_items": [
                    {"id": 10, "delivery_id": 1, "simple_product_id": 100, "diet_variant_meal_id": 500,
                     "is_simple_product_selected_by_user": true, "status": "REALIZED", "price_after_discount": 18.32, "rating_value": 5},
                ],
                "alternative_meals": [{"delivery_item_id": 10, "simple_product_ids": [100, 101]}],
                "simple_products": [{"id": 100, "name": "Owsianka", "default_configurable_product_id": 7, "calorific": 350, "composition": "owies"}],
                "diet_variant_meals": [{"id": 500, "diet_variant_meal_type_id": 1, "kcal": 350}],
                "diet_variant_meal_types": [{"id": 1, "position": 1, "meal_name": {"key": "BREAKFAST", "value": "Śniadanie"}}]
            }
        })
    }

    #[test]
    fn archives_meals_and_keeps_them_across_syncs() {
        let db = Db::memory().unwrap();
        db.store_deliveries(&deliveries()).unwrap();
        // A later sync where the API no longer returns the day must not lose it.
        db.store_deliveries(&json!({"results": [], "includes": {}})).unwrap();
        let history = db.history("2026-11-05").unwrap();
        let meal = &history["meals"][0];
        assert_eq!(meal["meal_name"], "Śniadanie");
        assert_eq!(meal["product"]["composition"], "owies");
        assert_eq!(meal["price"], 18.32);
        assert_eq!(meal["rating"], 5);
        assert_eq!(meal["alternative_count"], 2);
        assert_eq!(db.history("2026-11-01").unwrap()["meals"].as_array().unwrap().len(), 0);

        let dishes = db.dish_history("2026-11-05").unwrap();
        assert_eq!(dishes[&7].eaten, 1);
        assert_eq!(dishes[&7].your_rating, Some(5.0));
    }

    #[test]
    fn owner_comes_from_stored_diets() {
        let db = Db::memory().unwrap();
        assert_eq!(db.owner().unwrap(), None);
        db.store_diets(&[json!({"id": 1, "user_id": 4242, "user_diet_name": "X", "kcal": 1750})]).unwrap();
        assert_eq!(db.owner().unwrap(), Some(4242));
    }

    #[test]
    fn reads_meals_and_products_back() {
        let db = Db::memory().unwrap();
        db.store_deliveries(&deliveries()).unwrap();
        let meals = db.meals_between("2026-11-01", "2026-11-03").unwrap();
        assert_eq!(meals.len(), 1);
        assert_eq!(meals[0].meal_key.as_deref(), Some("BREAKFAST"));
        assert_eq!(meals[0].alternative_ids, [100, 101]);
        assert_eq!(meals[0].delivery["status"], "REALIZED");
        assert!(db.meals_between("2026-11-03", "2026-11-09").unwrap().is_empty());
        let products = db.products(&[100, 999]).unwrap();
        assert_eq!(products.len(), 1);
        assert_eq!(products[&100]["name"], "Owsianka");
        assert_eq!(db.status().unwrap()["meals"], 1);
    }

    #[test]
    fn selection_updates_archived_meal() {
        let db = Db::memory().unwrap();
        db.store_deliveries(&deliveries()).unwrap();
        db.record_selection(10, Some(100), 101).unwrap();
        let history = db.history("2026-11-05").unwrap();
        assert_eq!(history["selections"][0]["to_product"], 101);
        assert_eq!(history["selections"][0]["meal_name"], "Śniadanie");
    }
}
