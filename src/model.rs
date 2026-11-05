//! Dashboard view model: days → meals (rows) → dishes (tiles).

use std::collections::{BTreeMap, HashMap, HashSet};

use serde::Serialize;
use serde_json::Value;

/// Meals per day shown in the demo (the regular 5-meal plan, without the extra snack).
const DEMO_MEALS: i64 = 5;
const DEMO_OPTIONS: usize = 7;

#[derive(Serialize, Clone, Debug)]
pub struct Day {
    pub date: String,
    /// Last moment the menu for this day can still be changed (ISO, from the API).
    pub change_deadline: Option<String>,
    pub editable: bool,
    pub meals: Vec<Meal>,
}

#[derive(Serialize, Clone, Debug)]
pub struct Meal {
    pub delivery_id: u64,
    pub item_id: u64,
    pub key: String,
    pub name: String,
    pub position: i64,
    pub selected: Option<u64>,
    /// True once the user picked this meal (here or in the app); false for the default dish.
    pub user_selected: bool,
    pub options: Vec<Dish>,
}

#[derive(Serialize, Clone, Debug)]
pub struct Dish {
    pub id: u64,
    pub name: String,
    pub image: Option<String>,
    pub kcal: Option<f64>,
    pub protein: Option<f64>,
    pub carb: Option<f64>,
    pub fat: Option<f64>,
    pub allergens: Vec<String>,
}

fn by_id(includes: &Value, name: &str) -> HashMap<u64, Value> {
    includes[name]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|v| Some((v["id"].as_u64()?, v.clone())))
        .collect()
}

fn dish(product: &Value) -> Dish {
    let images = product["images"].as_array();
    let image = images
        .and_then(|imgs| imgs.iter().find(|i| i["type"] == "MAIN").or(imgs.first()))
        .and_then(|i| i["id"].as_str())
        .map(str::to_string);
    let allergens = product["allergens"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|a| a["tag_value"]["value"].as_str().map(str::to_string))
        .collect();
    Dish {
        id: product["id"].as_u64().unwrap_or_default(),
        name: product["name"].as_str().unwrap_or_default().to_string(),
        image,
        kcal: product["calorific"].as_f64(),
        protein: product["protein"].as_f64(),
        carb: product["carb"].as_f64(),
        fat: product["fat"].as_f64(),
        allergens,
    }
}

struct MealType {
    key: String,
    name: String,
    position: i64,
}

fn meal_type(t: &Value) -> MealType {
    MealType {
        key: t["meal_name"]["key"].as_str().unwrap_or_default().to_string(),
        name: t["meal_name"]["value"].as_str().unwrap_or_default().to_string(),
        position: t["position"].as_i64().or(t["meal_name"]["position"].as_i64()).unwrap_or_default(),
    }
}

/// Builds days from `GET users/{id}/deliveries` (`data` object with `results` and `includes`).
pub fn days_from_deliveries(data: &Value, now: chrono::DateTime<chrono::FixedOffset>) -> Vec<Day> {
    let includes = &data["includes"];
    let products = by_id(includes, "simple_products");
    let variant_meals = by_id(includes, "diet_variant_meals");
    let meal_types = by_id(includes, "diet_variant_meal_types");
    let alternatives: HashMap<u64, Vec<u64>> = includes["alternative_meals"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|a| {
            let ids = a["simple_product_ids"].as_array()?.iter().filter_map(Value::as_u64).collect();
            Some((a["delivery_item_id"].as_u64()?, ids))
        })
        .collect();
    let items = includes["delivery_items"].as_array().cloned().unwrap_or_default();

    let mut days: BTreeMap<String, Day> = BTreeMap::new();
    for delivery in data["results"].as_array().into_iter().flatten() {
        let Some(delivery_id) = delivery["id"].as_u64() else { continue };
        let date = delivery["date"].as_str().unwrap_or_default().get(..10).unwrap_or_default().to_string();
        let deadline = delivery["products_change_to"].as_str().map(str::to_string);
        let editable = deadline
            .as_deref()
            .and_then(parse_api_time)
            .is_some_and(|d| d > now);
        let day = days.entry(date.clone()).or_insert_with(|| Day {
            date,
            change_deadline: deadline.clone(),
            editable,
            meals: vec![],
        });
        day.editable |= editable;

        for item in items.iter().filter(|i| i["delivery_id"].as_u64() == Some(delivery_id)) {
            if item["status"] == "CANCELED" {
                continue;
            }
            let Some(item_id) = item["id"].as_u64() else { continue };
            let selected = item["simple_product_id"].as_u64();
            let mt = item["diet_variant_meal_id"]
                .as_u64()
                .and_then(|id| variant_meals.get(&id))
                .and_then(|vm| meal_types.get(&vm["diet_variant_meal_type_id"].as_u64()?))
                .map(meal_type)
                .unwrap_or(MealType { key: String::new(), name: String::new(), position: 0 });

            let mut ids: Vec<u64> = selected.into_iter().collect();
            ids.extend(alternatives.get(&item_id).into_iter().flatten().copied());
            let mut seen = HashSet::new();
            ids.retain(|id| seen.insert(*id));
            let options = ids.iter().filter_map(|id| products.get(id)).map(dish).collect();

            day.meals.push(Meal {
                delivery_id,
                item_id,
                key: mt.key,
                name: mt.name,
                position: mt.position,
                selected,
                user_selected: item["is_simple_product_selected_by_user"].as_bool().unwrap_or(false),
                options,
            });
        }
    }
    let mut days: Vec<Day> = days.into_values().filter(|d| !d.meals.is_empty()).collect();
    for day in &mut days {
        day.meals.sort_by_key(|m| (m.position, m.item_id));
    }
    days
}

/// Builds a demo day from the public `GET diet-menu/{date}` response: for each meal, the
/// dishes that the selectable ("… P") diets serve that day.
pub fn demo_day_from_menu(date: &str, data: &Value, day_index: u64) -> Option<Day> {
    let includes = &data["includes"];
    let products = by_id(includes, "simple_products");
    let meal_types = by_id(includes, "diet_variant_meal_types");
    let diets = by_id(includes, "diets");
    let selectable: HashSet<u64> = diets
        .iter()
        .filter(|(_, d)| d["name"].as_str().is_some_and(|n| n.ends_with(" P")))
        .map(|(id, _)| *id)
        .collect();

    let mut results: Vec<&Value> = data["results"].as_array()?.iter().collect();
    results.sort_by_key(|r| {
        let diet = r["diet_id"].as_u64().and_then(|id| diets.get(&id));
        diet.and_then(|d| d["position"].as_i64()).unwrap_or(i64::MAX)
    });

    let mut meals: BTreeMap<i64, Meal> = BTreeMap::new();
    let mut seen: HashSet<(i64, u64)> = HashSet::new();
    for r in results {
        let diet_id = r["diet_id"].as_u64().unwrap_or_default();
        if !selectable.is_empty() && !selectable.contains(&diet_id) {
            continue;
        }
        let Some(mt) = r["diet_variant_meal_type_id"].as_u64().and_then(|id| meal_types.get(&id)).map(meal_type)
        else {
            continue;
        };
        if mt.position < 1 || mt.position > DEMO_MEALS {
            continue;
        }
        let Some(product) = r["simple_product_id"].as_u64().and_then(|id| products.get(&id)) else { continue };
        let configurable = r["configurable_product_id"].as_u64().unwrap_or_default();
        let meal = meals.entry(mt.position).or_insert_with(|| Meal {
            delivery_id: 0,
            item_id: day_index * 100 + mt.position as u64,
            key: mt.key,
            name: mt.name,
            position: mt.position,
            selected: None,
            user_selected: false,
            options: vec![],
        });
        // The same dish is served in several calorie sizes; show it once.
        if meal.options.len() < DEMO_OPTIONS && seen.insert((mt.position, configurable)) {
            meal.options.push(dish(product));
        }
    }
    if meals.is_empty() {
        return None;
    }
    let mut meals: Vec<Meal> = meals.into_values().collect();
    for meal in &mut meals {
        meal.selected = meal.options.first().map(|d| d.id);
    }
    Some(Day { date: date.to_string(), change_deadline: None, editable: true, meals })
}

/// The API uses "YYYY-MM-DD HH:MM:SS" (Polish local time) or RFC 3339.
fn parse_api_time(s: &str) -> Option<chrono::DateTime<chrono::FixedOffset>> {
    chrono::DateTime::parse_from_rfc3339(s).ok().or_else(|| {
        let naive = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S").ok()?;
        naive.and_local_timezone(warsaw_offset(naive)).single()
    })
}

/// Europe/Warsaw offset without a tz database: CEST between the last Sundays of March and October.
fn warsaw_offset(t: chrono::NaiveDateTime) -> chrono::FixedOffset {
    use chrono::Datelike;
    let last_sunday = |month: u32| {
        let last = chrono::NaiveDate::from_ymd_opt(t.year(), month, 31).unwrap();
        last - chrono::Days::new(last.weekday().num_days_from_sunday() as u64)
    };
    let summer = t.date() >= last_sunday(3) && t.date() < last_sunday(10);
    chrono::FixedOffset::east_opt(if summer { 7200 } else { 3600 }).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn now() -> chrono::DateTime<chrono::FixedOffset> {
        chrono::DateTime::parse_from_rfc3339("2026-11-05T10:00:00+01:00").unwrap()
    }

    #[test]
    fn demo_day_has_five_meals_of_distinct_dishes() {
        let data: Value = serde_json::from_str(include_str!("../tests/fixtures/diet_menu.json")).unwrap();
        let day = demo_day_from_menu("2026-11-08", &data, 1).unwrap();
        let names: Vec<_> = day.meals.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, ["Śniadanie", "II Śniadanie", "Obiad", "Podwieczorek", "Kolacja"]);
        for meal in &day.meals {
            // Fixture has 3 selectable diets, the "MINI" diet must be excluded.
            assert!((1..=3).contains(&meal.options.len()), "{}: {}", meal.name, meal.options.len());
            assert_eq!(meal.selected, Some(meal.options[0].id));
            assert!(meal.options.iter().all(|d| d.image.is_some() && !d.name.is_empty()));
        }
    }

    #[test]
    fn deliveries_become_days_with_current_dish_and_alternatives() {
        let product = |id: u64, name: &str| json!({"id": id, "name": name, "calorific": 400, "images": [{"id": format!("{id}.jpg"), "type": "MAIN"}]});
        let data = json!({
            "results": [
                {"id": 1, "date": "2026-11-07", "products_change_to": "2026-11-05 23:59:59"},
                {"id": 2, "date": "2026-11-06", "products_change_to": "2026-11-04 12:00:00"}
            ],
            "includes": {
                "delivery_items": [
                    {"id": 11, "delivery_id": 1, "simple_product_id": 100, "diet_variant_meal_id": 501, "is_simple_product_selected_by_user": false},
                    {"id": 10, "delivery_id": 1, "simple_product_id": 200, "diet_variant_meal_id": 500, "is_simple_product_selected_by_user": true},
                    {"id": 12, "delivery_id": 1, "simple_product_id": 300, "diet_variant_meal_id": 500, "status": "CANCELED"},
                    {"id": 20, "delivery_id": 2, "simple_product_id": 100, "diet_variant_meal_id": 500}
                ],
                "alternative_meals": [{"delivery_item_id": 11, "simple_product_ids": [101, 100, 102]}],
                "simple_products": [product(100, "A"), product(101, "B"), product(102, "C"), product(200, "D")],
                "diet_variant_meals": [{"id": 500, "diet_variant_meal_type_id": 1}, {"id": 501, "diet_variant_meal_type_id": 3}],
                "diet_variant_meal_types": [
                    {"id": 1, "position": 1, "meal_name": {"key": "BREAKFAST", "value": "Śniadanie"}},
                    {"id": 3, "position": 3, "meal_name": {"key": "LUNCH", "value": "Obiad"}}
                ]
            }
        });
        let days = days_from_deliveries(&data, now());
        assert_eq!(days.iter().map(|d| d.date.as_str()).collect::<Vec<_>>(), ["2026-11-06", "2026-11-07"]);
        assert!(!days[0].editable);
        let day = &days[1];
        assert!(day.editable);
        assert_eq!(day.meals.iter().map(|m| m.item_id).collect::<Vec<_>>(), [10, 11]);
        let lunch = &day.meals[1];
        assert_eq!(lunch.name, "Obiad");
        assert_eq!(lunch.options.iter().map(|d| d.id).collect::<Vec<_>>(), [100, 101, 102]);
        assert_eq!(lunch.selected, Some(100));
        assert!(day.meals[0].user_selected && !lunch.user_selected);
    }

    #[test]
    fn parses_api_times_in_warsaw_time() {
        assert_eq!(parse_api_time("2026-11-05 12:00:00").unwrap().to_rfc3339(), "2026-11-05T12:00:00+01:00");
        assert_eq!(parse_api_time("2026-07-05 12:00:00").unwrap().to_rfc3339(), "2026-07-05T12:00:00+02:00");
    }
}
