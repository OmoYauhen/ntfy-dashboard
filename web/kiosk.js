"use strict";

// Kitchen kiosk (/dashboard): today's meals, upcoming days and rating past meals, for a landscape
// touch screen that shows this page fullscreen all day.

const PAST_DAYS = 10;
/// Far enough ahead to see the last day of the paid plan.
const FUTURE_DAYS = 60;
const REFRESH_MS = 5 * 60 * 1000;
const IDLE_MS = 2 * 60 * 1000;
const WAKE_MS = 60 * 1000;
const NIGHT_FROM = 22;
const NIGHT_TO = 6;
const STALE_SYNC_MS = 2 * 3600 * 1000;
/// Usual start of each meal (hours); the card stays "up next" until 90 min after it starts.
const MEAL_HOURS = { BREAKFAST: 6.5, "SECOND-BREAKFAST": 10, LUNCH: 13, TEA: 16, DINNER: 18.5 };
const MEAL_WINDOW_H = 1.5;
/// Today's layout: these meals get the three big columns; the others (second breakfast or
/// afternoon snack on top, snack below) share a narrower fourth column.
const MAIN_MEALS = ["BREAKFAST", "LUNCH", "DINNER"];
/// One-tap comment phrases. Comments go to the caterer, so they're in Polish.
const QUICK_COMMENTS = [
  ["Pyszne!", "Delicious"], ["Idealna porcja", "Perfect portion"], ["Za mała porcja", "Too small"],
  ["Za słone", "Too salty"], ["Mdłe", "Bland"], ["Za ostre", "Too spicy"], ["Suche", "Dry"], ["Nie smakowało", "Didn't like it"],
];

const state = { days: [], status: null, view: "today", lastTouch: Date.now(), loading: null };

const iso = (d) => `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
const addDays = (date, n) => { const d = new Date(`${date}T12:00:00`); d.setDate(d.getDate() + n); return iso(d); };
const fmtDate = (date, opts) => new Date(`${date}T12:00:00`).toLocaleDateString("en-GB", opts);
const today = () => state.status?.today || iso(new Date());
const thumb = (dish) => dish?.image_url?.replace("r=MEDIUM-RETINA", "r=MEDIUM");
const num = (v, digits = 0) => (v == null ? "–" : Number(v).toFixed(digits).replace(/\.0+$/, ""));
const mealsWithDish = (day) => day.meals.filter((m) => m.dish && m.status !== "CANCELED");


async function getJson(url, options) {
  const resp = await fetch(url, options);
  const data = await resp.json().catch(() => ({}));
  if (!resp.ok) throw new Error(data.error || resp.statusText);
  return data;
}

// ---------- data ----------

async function load() {
  if (state.loading) return state.loading;
  state.loading = (async () => {
    try {
      const status = await getJson("/api/v1/status");
      state.status = status;
      const t = status.today;
      const data = await getJson(`/api/v1/days?from=${addDays(t, -PAST_DAYS)}&to=${addDays(t, FUTURE_DAYS)}`);
      state.days = data.days;
      render();
    } catch (err) {
      if (!state.days.length) {
        $("view").replaceChildren(el("div", { class: "empty" }, el("span", { class: "big" }, "Can't reach the kitchen data"), err.message));
      }
    } finally {
      state.loading = null;
    }
  })();
  return state.loading;
}

/// Today's delivery, or the next one when nothing comes today.
function featuredDay() {
  const t = today();
  return state.days.find((d) => d.date === t && d.menu_published)
    || state.days.find((d) => d.date > t && d.menu_published);
}

/// Index of the meal to highlight, and which ones are over (only for today's day).
function mealTiming(day) {
  const isToday = day.date === today();
  const now = new Date();
  const hour = now.getHours() + now.getMinutes() / 60;
  let next = -1;
  const done = day.meals.map((m, i) => {
    const start = MEAL_HOURS[m.meal_key];
    if (!isToday) return false;
    if (start == null) return false;
    const over = start + MEAL_WINDOW_H <= hour;
    if (!over && next === -1) next = i;
    return over;
  });
  if (!isToday) next = 0;
  return { next, done };
}

// ---------- chrome ----------

function renderClock() {
  const now = new Date();
  $("time").textContent = now.toLocaleTimeString("en-GB", { hour: "2-digit", minute: "2-digit" });
  $("date").textContent = now.toLocaleDateString("en-GB", { weekday: "long", day: "numeric", month: "long" });
  const h = now.getHours();
  const night = (h >= NIGHT_FROM || h < NIGHT_TO) && Date.now() - state.lastTouch > WAKE_MS;
  document.body.classList.toggle("night", night);
}

function renderSummary(day) {
  const box = $("summary");
  if (!day) return box.replaceChildren();
  const t = day.totals;
  const kcalFromMacros = (t.protein_g * 4 + t.carb_g * 4 + t.fat_g * 9) || 1;
  const share = (g, k) => `${(g * k * 100) / kcalFromMacros}%`;
  const label = day.date === today() ? "today" : fmtDate(day.date, { weekday: "short" });
  box.replaceChildren(
    el("span", { class: "kcal" }, num(t.kcal), el("small", {}, `kcal ${label}`)),
    el("div", {},
      el("div", { class: "macro-bar" },
        el("span", { style: `width:${share(t.protein_g, 4)};background:var(--protein)` }),
        el("span", { style: `width:${share(t.carb_g, 4)};background:var(--carb)` }),
        el("span", { style: `width:${share(t.fat_g, 9)};background:var(--fat)` })),
      el("div", { class: "macro-legend", style: "margin-top:.45rem" },
        el("span", {}, el("i", { class: "dot", style: "background:var(--protein)" }), "Protein ", el("b", {}, `${num(t.protein_g)} g`)),
        el("span", {}, el("i", { class: "dot", style: "background:var(--carb)" }), "Carbs ", el("b", {}, `${num(t.carb_g)} g`)),
        el("span", {}, el("i", { class: "dot", style: "background:var(--fat)" }), "Fat ", el("b", {}, `${num(t.fat_g)} g`)))),
  );
}

function renderSync() {
  const box = $("sync");
  const s = state.status;
  if (!s) return;
  if (!s.connected_to_ntfy) {
    box.textContent = `Not connected to NTFY${s.login_error ? `: ${s.login_error}` : ""}`;
    box.className = "sync stale";
    return;
  }
  const last = s.archive?.last_sync ? new Date(s.archive.last_sync.replace(" ", "T")) : null;
  const age = last ? Date.now() - last : Infinity;
  const mins = Math.round(age / 60000);
  box.textContent = !last ? "Not synced yet" : mins < 1 ? "Synced just now" : mins < 90 ? `Synced ${mins} min ago` : `Synced ${Math.round(mins / 60)} h ago`;
  box.className = `sync${age > STALE_SYNC_MS ? " stale" : ""}`;
}

function rateBadge() {
  const pending = state.days
    .filter((d) => d.can_rate)
    .flatMap(mealsWithDish)
    .filter((m) => m.your_rating == null).length;
  const badge = $("rate-badge");
  badge.hidden = !pending;
  badge.textContent = pending;
}

// ---------- views ----------

function title(text, ...extra) {
  return el("div", { class: "view-title" }, el("h1", {}, text), ...extra);
}

function renderToday() {
  const day = featuredDay();
  if (!day) {
    return el("div", { class: "empty" }, el("span", { class: "big" }, "No meals planned"), "Nothing is scheduled in the coming weeks.");
  }
  const { next, done } = mealTiming(day);
  const isToday = day.date === today();
  const heading = isToday ? "Today's menu" : `Next delivery · ${fmtDate(day.date, { weekday: "long", day: "numeric", month: "long" })}`;
  const sub = isToday && next === -1 ? "All meals done. Enjoy your evening" : null;
  return el("div", { class: "today" },
    title(heading, sub ? el("span", { class: "sub" }, sub) : null,
      day.can_rate ? el("span", { class: "pill gold" }, "★ Tap a meal to rate it") : null),
    mealGrid(day, next, done),
  );
}

/// Breakfast, lunch and dinner as three main columns; the smaller meals stacked in a fourth.
function mealGrid(day, next, done) {
  const cards = day.meals.map((m, i) => ({ meal: m, card: mealCard(day, m, i === next, done[i]) }));
  let main = cards.filter((c) => MAIN_MEALS.includes(c.meal.meal_key));
  let side = cards.filter((c) => !MAIN_MEALS.includes(c.meal.meal_key));
  if (!main.length) [main, side] = [cards.slice(0, 3), cards.slice(3)];
  return el("div", { class: `meal-grid${side.length ? "" : " no-side"}`, style: `--main:${main.length}` },
    main.map((c) => c.card),
    side.length ? el("div", { class: "side-meals" }, side.map((c) => c.card)) : null);
}

function mealCard(day, meal, isNext, isDone) {
  const d = meal.dish;
  return el("button", {
    class: `meal-card${isNext ? " next" : ""}${isDone ? " done" : ""}`,
    onclick: () => d && openDish(day, meal),
  },
    el("div", { class: "photo" },
      d ? el("img", { src: d.image_url, alt: "" }) : el("div", { class: "no-dish" }, "Menu not published yet"),
      isNext ? el("span", { class: "badge-next" }, day.date === today() ? "UP NEXT" : "FIRST") : null),
    el("div", { class: "info" },
      el("span", { class: "meal-name" }, meal.meal || meal.meal_key),
      d ? el("p", { class: "dish-name" }, d.name) : null,
      d ? el("div", { class: "chips" },
        el("span", { class: "chip kcal" }, `${num(d.kcal)} kcal`),
        el("span", { class: "chip" }, `P ${num(d.protein_g)}`),
        el("span", { class: "chip" }, `C ${num(d.carb_g)}`),
        el("span", { class: "chip" }, `F ${num(d.fat_g)}`),
        meal.your_rating ? el("span", { class: "chip mine" }, "★".repeat(meal.your_rating)) : null) : null),
  );
}

function renderUpcoming() {
  const t = today();
  const future = state.days.filter((d) => d.date > t && d.status !== "cancelled");
  if (!future.length) return el("div", { class: "empty" }, el("span", { class: "big" }, "Nothing planned yet"));
  // Days with a published menu, plus the last delivery day so it's clear how far the paid plan goes.
  const last = future.at(-1);
  const published = future.filter((d) => d.menu_published && d !== last).slice(0, 5);
  const gap = published.length && future.indexOf(last) - future.indexOf(published.at(-1)) > 1;
  return el("div", { style: "height:100%;display:flex;flex-direction:column" },
    title("Coming up", el("span", { class: "sub" }, "Tap a dish for details")),
    el("div", { class: "upcoming", style: "flex:1;min-height:0" },
      published.map((d) => dayColumn(d)), dayColumn(last, { last: true, gap })));
}

function dayColumn(day, { last = false, gap = false } = {}) {
  const lockSoon = day.can_change && day.can_change_until && new Date(day.can_change_until) - Date.now() < 48 * 3600e3;
  const header = el("header", {},
    el("span", { class: "weekday" }, fmtDate(day.date, { weekday: "long" })),
    el("span", { class: "dmy" }, fmtDate(day.date, { day: "numeric", month: "short" })),
    el("div", { class: "meta" },
      day.menu_published ? el("span", { class: "total" }, el("b", {}, num(day.totals.kcal)), " kcal") : el("span", { class: "total" }, " "),
      last ? el("span", { class: "pill gold" }, "Last paid day") : lockSoon ? el("span", { class: "pill gold" }, "Choose soon") : null));
  const cls = `day-col${day.menu_published ? "" : " unpublished"}${gap ? " gap" : ""}`;
  if (!day.menu_published) {
    // A publish date in the past means the menu is out and the next sync will bring it.
    const at = day.menu_published_at && new Date(day.menu_published_at);
    const text = at && at > Date.now()
      ? `Menu arrives ${fmtDate(day.menu_published_at.slice(0, 10), { weekday: "short", day: "numeric", month: "short" })}`
      : "Menu coming soon";
    return el("section", { class: cls }, header, el("ol", {}, text));
  }
  return el("section", { class: cls }, header,
    el("ol", {}, day.meals.map((m) => el("li", {},
      el("button", { class: "mini", onclick: () => m.dish && openDish(day, m) },
        el("img", { src: thumb(m.dish) || "", alt: "" }),
        el("div", { class: "t" },
          el("div", { class: "m" }, m.meal || ""),
          el("div", { class: "n" }, m.dish?.name || "—"),
          el("div", { class: "k" }, m.dish ? `${num(m.dish.kcal)} kcal` : "")))))));
}

function renderRate() {
  const t = today();
  const days = state.days
    .filter((d) => d.date <= t && d.menu_published && d.status !== "cancelled" && d.status !== "planned")
    .sort((a, b) => b.date.localeCompare(a.date));
  if (!days.length) return el("div", { class: "empty" }, el("span", { class: "big" }, "Nothing to rate yet"), "Delivered meals show up here.");
  return el("div", { class: "rate" },
    title("How was it?", el("span", { class: "sub" }, "Tap the stars. Ratings go straight to Nice To Fit You")),
    days.map((day) => el("section", { class: "rate-day" },
      el("h2", {}, fmtDate(day.date, { weekday: "long", day: "numeric", month: "long" }),
        day.can_rate
          ? el("span", { class: "pill gold" }, `open until ${fmtDate(day.can_rate_until.slice(0, 10), { weekday: "short" })} ${day.can_rate_until.slice(11, 16)}`)
          : el("span", { class: "pill" }, "rating closed")),
      el("div", { class: "rate-grid" }, mealsWithDish(day).map((m) => rateCard(day, m))))));
}

function rateCard(day, meal) {
  return el("div", { class: `rate-card${meal.your_rating ? " rated" : ""}` },
    el("img", { src: thumb(meal.dish), alt: "", onclick: () => openDish(day, meal) }),
    el("div", {},
      el("div", { class: "meal-name" }, meal.meal || ""),
      el("div", { class: "n", onclick: () => openDish(day, meal) }, meal.dish.name),
      el("div", { class: "row" },
        stars(meal.your_rating, day.can_rate ? (r) => saveRating(meal, r, meal.your_comment) : null),
        day.can_rate ? el("button", { class: "comment-btn", onclick: () => openDish(day, meal, true) }, meal.your_comment ? "✎ Edit" : "＋ Comment") : null),
      meal.your_comment ? el("div", { class: "your-comment" }, `“${meal.your_comment}”`) : null));
}

function stars(value, onRate, big = false) {
  return el("div", { class: `stars${onRate ? "" : " readonly"}${big ? " big" : ""}`, role: "radiogroup", "aria-label": "Rating" },
    [1, 2, 3, 4, 5].map((n) => el("button", {
      class: n <= (value || 0) ? "on" : "",
      "aria-label": `${n} star${n > 1 ? "s" : ""}`,
      onclick: (e) => { e.stopPropagation(); onRate?.(n); },
    }, "★")));
}

function render() {
  renderClock();
  renderSync();
  const day = featuredDay();
  renderSummary(day);
  rateBadge();
  const views = { today: renderToday, upcoming: renderUpcoming, rate: renderRate };
  const scroll = $("view").querySelector(".rate")?.scrollTop;
  $("view").replaceChildren(views[state.view]());
  if (scroll) $("view").querySelector(".rate")?.scrollTo(0, scroll);
  document.querySelectorAll(".tabs button").forEach((b) => b.classList.toggle("active", b.dataset.view === state.view));
}

function show(view) {
  state.view = view;
  render();
}

// ---------- rating ----------

async function saveRating(meal, rating, comment) {
  const before = { your_rating: meal.your_rating, your_comment: meal.your_comment };
  meal.your_rating = rating;
  meal.your_comment = comment || null;
  render();
  try {
    await getJson("/api/rate", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ item_id: meal.id, rating, comment: comment || null }),
    });
    toast(`Saved ${"★".repeat(rating)}`);
    return true;
  } catch (err) {
    Object.assign(meal, before);
    render();
    toast(`Couldn't save: ${err.message}`, true);
    return false;
  }
}

// ---------- sheet (details + feedback) ----------

function closeSheet() {
  const sheet = $("sheet");
  sheet.hidden = true;
  sheet.replaceChildren();
}

async function openDish(day, meal, focusFeedback = false) {
  const sheet = $("sheet");
  const d = meal.dish;
  const body = el("div", { class: "sheet-body" },
    el("span", { class: "meal-name" }, `${meal.meal || ""} · ${fmtDate(day.date, { weekday: "long", day: "numeric", month: "long" })}`),
    el("h2", {}, d.name),
    el("div", { class: "chips" },
      day.status === "planned" ? el("span", { class: "chip" }, meal.chosen_by_user ? "Your choice" : "Caterer's default") : null,
      d.ntfy_rating ? el("span", { class: "chip" }, `★ ${num(d.ntfy_rating, 1)} from NTFY customers`) : null),
    el("div", { class: "nutri" },
      [["kcal", d.kcal, 0], ["protein g", d.protein_g, 1], ["carbs g", d.carb_g, 1], ["fat g", d.fat_g, 1],
       ["fiber g", d.fiber_g, 1], ["sugar g", d.sugar_g, 1], ["salt g", d.salt_g, 2], ["weight g", d.weight_g, 0]]
        .map(([label, v, digits]) => el("div", {}, el("b", {}, num(v, digits)), el("span", {}, label)))),
  );
  // From "Comment", the form goes right under the title; otherwise after the nutrition facts.
  if (day.can_rate) {
    if (focusFeedback) body.querySelector("h2").after(feedbackForm(meal));
    else body.append(feedbackForm(meal));
  }
  else if (meal.your_rating) body.append(el("h3", {}, "Your rating"), stars(meal.your_rating, null, true),
    meal.your_comment ? el("p", { class: "composition" }, `“${meal.your_comment}”`) : null);

  sheet.replaceChildren(el("div", { class: "sheet-card", onclick: (e) => e.stopPropagation() },
    el("button", { class: "close", "aria-label": "Close", onclick: closeSheet }, "×"),
    el("div", { class: "sheet-photo" }, el("img", { src: d.image_url, alt: "" })),
    body));
  sheet.onclick = closeSheet;
  sheet.hidden = false;

  // Ingredients and allergens come with the full dish record.
  try {
    const full = await getJson(`/api/v1/dishes/${d.id}`);
    if (sheet.hidden) return;
    const extra = [
      full.allergens?.length ? [el("h3", {}, "Allergens"), el("div", { class: "allergens" }, full.allergens.map((a) => el("span", {}, a)))] : null,
      full.your_history ? el("p", { class: "closed-note" }, `You've had this ${full.your_history.times_eaten}×, last on ${fmtDate(full.your_history.last_eaten, { day: "numeric", month: "long" })}.`) : null,
      full.composition ? [el("h3", {}, "Ingredients"), el("p", { class: "composition" }, full.composition)] : null,
      full.may_contain ? el("p", { class: "closed-note" }, full.may_contain) : null,
    ];
    body.append(...extra.flat().filter(Boolean));
  } catch { /* the summary is enough */ }
}

function feedbackForm(meal) {
  let rating = meal.your_rating || 0;
  const picked = new Set();
  const text = el("textarea", { placeholder: "Your comment (optional, goes to the caterer)", maxlength: "500" });
  text.value = meal.your_comment || "";
  const save = el("button", { class: "save", disabled: !rating }, "Save rating");
  const starBox = el("div");
  const drawStars = () => starBox.replaceChildren(stars(rating, (r) => { rating = r; save.disabled = false; drawStars(); }, true));
  drawStars();
  const quick = el("div", { class: "quick" }, QUICK_COMMENTS.map(([pl, en]) => {
    const b = el("button", { title: en, onclick: () => {
      picked.has(pl) ? picked.delete(pl) : picked.add(pl);
      b.classList.toggle("on", picked.has(pl));
      const typed = text.value.split(/,\s*/).filter((p) => p && !QUICK_COMMENTS.some(([q]) => q === p));
      text.value = [...picked, ...typed].join(", ");
    } }, pl);
    if ((meal.your_comment || "").includes(pl)) { picked.add(pl); b.classList.add("on"); }
    return b;
  }));
  save.onclick = async () => {
    save.disabled = true;
    if (await saveRating(meal, rating, text.value.trim())) closeSheet();
    else save.disabled = false;
  };
  return el("div", { class: "feedback" }, el("h3", {}, meal.your_rating ? "Your rating" : "How was it?"), starBox, quick, text, save);
}

// ---------- wiring ----------

document.querySelectorAll(".tabs button").forEach((b) => b.addEventListener("click", () => show(b.dataset.view)));
document.addEventListener("pointerdown", () => {
  state.lastTouch = Date.now();
  if (document.body.classList.contains("night")) renderClock();
}, true);
// Swallow the touch that wakes the screen at night, so it doesn't also press a button.
$("night").addEventListener("click", (e) => { e.stopPropagation(); document.body.classList.remove("night"); });

setInterval(() => {
  renderClock();
  renderSync();
  if (Date.now() - state.lastTouch > IDLE_MS) {
    if (!$("sheet").hidden) closeSheet();
    if (state.view !== "today") show("today");
  }
}, 15 * 1000);
// Refresh the data, and re-render on the minute so "up next" follows the clock.
setInterval(() => { if ($("sheet").hidden) load(); }, REFRESH_MS);
setInterval(() => { if ($("sheet").hidden && state.view === "today") render(); }, 60 * 1000);

renderClock();
load();
