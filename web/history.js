"use strict";

const NUTRIENTS = [
  ["kcal", "kcal", 0],
  ["protein", "protein g", 1],
  ["carb", "carbs g", 1],
  ["fat", "fat g", 1],
  ["fiber", "fiber g", 1],
  ["sugar", "sugar g", 1],
  ["salt", "salt g", 2],
];

/// Raw archived NTFY product → the dish shape used by `dishDetails`.
function toDish(p) {
  if (!p) return { name: "(unknown dish)", allergens: [], tags: [] };
  const values = (list) => (list ?? []).map((t) => t.tag_value?.value).filter(Boolean);
  const image = (p.images ?? []).find((i) => i.type === "MAIN") ?? p.images?.[0];
  return {
    name: p.name, image: image?.id, kcal: p.calorific, kj: p.kj, weight: p.weight,
    protein: p.protein, carb: p.carb, fat: p.fat, saturated_fat: p.saturated_fat,
    sugar: p.sugar, fiber: p.fiber, salt: p.salt,
    rating: p.rating, ratings_count: p.number_of_ratings,
    composition: p.composition, allergens: values(p.allergens), may_contain: p.cross_allergen_label,
    tags: values(p.work_name_tags), configurable_id: p.default_configurable_product_id,
  };
}

const sum = (list, f) => list.reduce((s, x) => s + (Number(f(x)) || 0), 0);
const money = (v) => `${Number(v).toFixed(2)} zł`;

function card(label, value, sub) {
  return el("div", { class: "card" }, el("span", { class: "label" }, label), el("strong", {}, value), sub ? el("span", { class: "sub" }, sub) : null);
}

function renderSummary(days, meals, orders) {
  const n = days.length || 1;
  const avg = (key) => sum(meals, (m) => m.dish[key]) / n;
  const spentMeals = sum(meals, (m) => m.price);
  const spentOrders = sum(orders, (o) => o.total_price_after_discount);
  const rated = meals.filter((m) => m.rating != null);
  $("summary").replaceChildren(
    card("Days archived", String(days.length), `${meals.length} meals`),
    card("Avg per day", `${Math.round(avg("kcal"))} kcal`,
      `P ${round(avg("protein"))} g · C ${round(avg("carb"))} g · F ${round(avg("fat"))} g`),
    card("Fiber · sugar · salt", `${round(avg("fiber"))} · ${round(avg("sugar"))} · ${round(avg("salt"), 1)} g`, "average per day"),
    card("Chosen by you", `${meals.filter((m) => m.user_selected).length} / ${meals.length}`, "meals you picked, not defaults"),
    card("Your ratings", rated.length ? `★ ${round(sum(rated, (m) => m.rating) / rated.length, 2)}` : "–", `${rated.length} rated`),
    card("Archived meals cost", money(spentMeals), `${orders.length} orders, ${money(spentOrders)} total`),
  );
}

function renderFavorites(meals) {
  const byDish = new Map();
  for (const m of meals) {
    const key = m.dish.configurable_id ?? m.dish.name;
    const entry = byDish.get(key) ?? { dish: m.dish, count: 0, ratings: [] };
    entry.count += 1;
    if (m.rating != null) entry.ratings.push(m.rating);
    byDish.set(key, entry);
  }
  const top = [...byDish.values()].sort((a, b) => b.count - a.count).slice(0, 10);
  if (!top.length || top[0].count < 2) {
    $("favorites").replaceChildren();
    return;
  }
  $("favorites").replaceChildren(
    el("h2", {}, "Most eaten"),
    el("ol", { class: "favorites" }, top.map((e) => el("li", {},
      `${e.dish.name} `,
      el("span", { class: "muted" }, `${e.count}×${e.ratings.length ? ` · you ★${round(sum(e.ratings, (r) => r) / e.ratings.length, 1)}` : ""}`)))),
  );
}

function mealRow(m) {
  const d = m.dish;
  return el("details", { class: "hist-meal" },
    el("summary", {},
      el("img", { src: imageUrl(d.image), alt: "", loading: "lazy" }),
      el("span", { class: "meal-name" }, m.meal_name ?? m.meal_key ?? ""),
      el("span", { class: "dish-name" }, d.name,
        m.user_selected ? null : el("span", { class: "muted" }, " (default)")),
      el("span", { class: "num" }, d.kcal != null ? `${round(d.kcal)} kcal` : ""),
      el("span", { class: "num muted" }, `P ${round(d.protein) ?? "–"} · C ${round(d.carb) ?? "–"} · F ${round(d.fat) ?? "–"}`),
      el("span", { class: "num" }, m.rating != null ? `★ ${m.rating}` : ""),
      el("span", { class: "num muted" }, m.price != null ? money(m.price) : ""),
    ),
    m.rating_comment ? el("p", { class: "hint" }, `Your comment: ${m.rating_comment}`) : null,
    dishDetails(d),
  );
}

function renderDays(days, dietNames) {
  $("history-days").replaceChildren(
    el("h2", {}, "Days"),
    ...days.map(({ date, meals }) => {
      const totals = NUTRIENTS.map(([key, label, digits]) => `${round(sum(meals, (m) => m.dish[key]), digits)} ${label}`);
      const diets = [...new Set(meals.map((m) => dietNames.get(m.delivery_diet_id)).filter(Boolean))];
      return el("article", { class: "hist-day" },
        el("header", {},
          el("h3", {}, dateLabel(date, { weekday: "long", day: "numeric", month: "long", year: "numeric" })),
          diets.length ? el("span", { class: "muted" }, diets.join(", ")) : null,
          el("span", { class: "totals" }, totals.join(" · ")),
        ),
        meals.map(mealRow),
      );
    }),
  );
}

function renderOrders(orders) {
  if (!orders.length) return;
  $("orders").replaceChildren(
    el("h2", {}, "Orders"),
    el("table", { class: "orders" },
      el("thead", {}, el("tr", {}, ["Order", "Placed", "Deliveries", "Days", "Meals", "Delivery", "Total", "Status"].map((h) => el("th", {}, h)))),
      el("tbody", {}, orders.map((o) => el("tr", {},
        el("td", {}, String(o.id)),
        el("td", {}, (o.created_at ?? "").slice(0, 10)),
        el("td", {}, o.first_delivery_date && o.last_delivery_date ? `${o.first_delivery_date} → ${o.last_delivery_date}` : ""),
        el("td", { class: "num" }, String(o.delivery_days_count ?? o.deliveries?.length ?? "")),
        el("td", { class: "num" }, o.products_price != null ? money(o.products_price) : ""),
        el("td", { class: "num" }, o.delivery_price != null ? money(o.delivery_price) : ""),
        el("td", { class: "num" }, o.total_price_after_discount != null ? money(o.total_price_after_discount) : ""),
        el("td", {}, o.status ?? ""),
      ))),
    ),
  );
}

async function load() {
  const msg = $("message");
  try {
    const resp = await api("/api/history");
    const data = await resp.json();
    if (!resp.ok) throw new Error(data.error || resp.statusText);
    const meals = data.meals.map((m) => ({ ...m, dish: toDish(m.product) }));
    const byDate = new Map();
    for (const m of meals) byDate.set(m.date, [...(byDate.get(m.date) ?? []), m]);
    const days = [...byDate].map(([date, list]) => ({ date, meals: list }));
    const dietNames = new Map(data.diets.map((d) => [d.id, `${d.user_diet_name} (${d.kcal} kcal)`]));

    $("note").textContent = `NTFY only keeps about 3 past days, so this dashboard archives every meal it sees (synced hourly)` +
      (data.archived_since ? `, since ${data.archived_since}.` : ".") +
      " Your orders go back further, but NTFY has already deleted what was in them.";
    renderSummary(days, meals, data.orders);
    renderFavorites(meals);
    renderDays(days, dietNames);
    renderOrders(data.orders);
    msg.hidden = !!days.length;
    msg.textContent = "Nothing archived yet.";
  } catch (err) {
    msg.hidden = false;
    msg.textContent = `Could not load history: ${err.message}`;
  }
}

$("sync").addEventListener("click", async (e) => {
  const button = e.currentTarget;
  button.disabled = true;
  try {
    const resp = await api("/api/sync", { method: "POST" });
    const data = await resp.json();
    if (!resp.ok) throw new Error(data.error || resp.statusText);
    toast(`Synced ${data.meals} meals, ${data.orders} orders`);
    await load();
  } catch (err) {
    toast(`Sync failed: ${err.message}`, true);
  } finally {
    button.disabled = false;
  }
});

load();
