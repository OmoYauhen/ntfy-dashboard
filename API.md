# NTFY (Nice To Fit You) API notes

Reverse-engineered on 2026-11-05.

## Sources

- `Nice To Fit You.apk` (package `pl.ntfy.app`) is a Flutter app. The provided file is only the
  **base split**: the compiled Dart code (`lib/arm64-v8a/libapp.so`) ships in a separate
  `config.arm64_v8a.apk` split, which is not included. The base APK only gave the brand id (`ntfyPl`),
  the Firebase project, and the Polish UI strings (`assets/flutter_assets/assets/ntfyPl/flutter_i18n/pl.json`).
- The customer web panel on https://ntfy.pl (Vue bundle under `/hercules-dist/js/`) talks to the same
  backend. All endpoints below come from that bundle, except the item-change route (see below).

## Basics

- Base URL: `https://orion-api.ntfy.pl/api/v2.0/` (some calls use `/api/v3.0/`).
- Every request needs `Trace-Id: <uuid4>` (400 without it) and `Api-Language: pl|en` (406 without it).
- Auth: `Authorization: Bearer <token>`.
- Response envelope: `{ trace_id, start_timestamp, end_timestamp, data | message, code, details }`.
  Lists come as `data.results` plus side-loaded entities in `data.includes`, which you request with
  `expansions__in=a,b.c,...`.

## Endpoints used

| Method | Path | Notes |
|---|---|---|
| POST | `sessions` | `{email, password}` → `data.token`, `data.refresh_token`. The JWT payload has `id` (user id) and `first_name`. |
| PATCH | `sessions` | `{refresh_token}` → new session. |
| GET | `users/{uid}/deliveries` | Query: `date__gte`, `status__in=TO-BE-REALIZED`, `limit`, `offset`, `expansions__in=delivery_items,delivery_items.simple_products,delivery_items.diet_variant_meals,delivery_items.diet_variant_meals.diet_variant_meal_types,delivery_items.alternative_meals`. |
| PATCH | `users/{uid}/deliveries/{did}/items/{iid}` | **Swap the dish of one meal.** See below. |
| GET | `diet-menu/{YYYY-MM-DD}` | Public. Whole menu of all diets for a day. `expansions__in=diet_variant_meal_types,configurable_products,simple_products,diet_variant_meals,diets`. |
| GET | `images/{imageId}?resolution=MEDIUM` | Public, but needs the headers above. Resolutions seen: `MEDIUM`, `MEDIUM-V`, `V-MEDIUM-RETINA`. |

Other routes found in the bundle: `calendar-days/{from}/{to}`, `users/{uid}/delivery-diets`,
`users/{uid}/deliveries/{did}/items-delete` (`{delivery_item_ids}`), `PATCH users/{uid}/deliveries`
(change day or address, `{deliveries:[{delivery_id,date,delivery_diet_ids}]}`), `users/{uid}/orders`,
`cart/...`, `diet-offers`, `tiles`, `users/{uid}/moneybox`.

## Delivery model

- `results[]` is a delivery: `id`, `date`, `delivery_date`, `show_menu_from`, `products_change_to`
  (menu-change deadline, Warsaw local time), `address_id`, `status`.
- `includes.delivery_items[]` is one meal: `id`, `delivery_id`, `delivery_diet_id`, `simple_product_id`
  (current dish), `diet_variant_meal_id`, `is_simple_product_selected_by_user`, `rating_value`, `status`.
- `includes.alternative_meals[]`: `{delivery_item_id, simple_product_ids[]}`, the dishes this meal
  can be swapped to.
- `diet_variant_meals[].diet_variant_meal_type_id` → `diet_variant_meal_types[]`
  (`position`, `meal_name.{key,value}`, e.g. `BREAKFAST`/"Śniadanie").
- `simple_products[]`: `name`, `images[{id,type:MAIN}]`, `calorific`, `protein`, `carb`, `fat`,
  `allergens[].tag_value.value`, and `default_configurable_product_id` (the same dish in other kcal sizes).

## Changing a meal

The web panel cannot change dishes. Meal choice ("Wybór posiłków") exists only in the mobile app,
and its code is in the missing split. Probing without auth shows which routes exist:

```
PATCH users/1/deliveries/1/items/1    → 406 (route exists; fails later on locale/auth)
PATCH users/1/deliveries/1/items      → 404 No route found
PATCH users/1/delivery-items/1        → 404 No route found
```

`PATCH users/{uid}/deliveries/{did}/items/{iid}` is the only item-level write route, and the web client
never uses it. With body `{"simple_product_id": N}` it returns `200` and the updated item in
`data.result` (checked on 2026-11-05 by re-sending a meal's current dish). The dashboard checks that the
returned `simple_product_id` equals the requested one, so a body the API ignores shows up as an error.

## History limits

- `GET users/{uid}/deliveries` never returns days older than about three days, whatever the filters
  (`date__gte`, `date__lte`, `date`, `status__in=REALIZED`, `delivery_diet_id__in`, v3).
- `GET users/{uid}/orders` returns all orders (prices and delivery dates only). There's no GET for a single
  order, and expansions are rejected.
- `GET users/{uid}/delivery-diets` returns the diet subscriptions (`user_diet_name`, `kcal`, `delivery_days`).
- `GET diet-menu/{date}` still works for past dates, but it's the public menu, not what you got.
