"use strict";

const ADVANCE_DELAY_MS = 700;

const state = { days: [], current: 0 };
const $ = (id) => document.getElementById(id);

const isPicked = (day) => day.meals.every((m) => m.user_selected);
const pickedCount = (day) => day.meals.filter((m) => m.user_selected).length;

const dateLabel = (iso, opts) =>
  new Date(`${iso}T12:00:00`).toLocaleDateString(undefined, opts);

function el(tag, attrs = {}, ...children) {
  const node = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (k === "class") node.className = v;
    else if (k.startsWith("on")) node.addEventListener(k.slice(2), v);
    else if (v !== false && v != null) node.setAttribute(k, v === true ? "" : v);
  }
  node.append(...children.filter((c) => c != null));
  return node;
}

let toastTimer;
function toast(text, error = false) {
  const t = $("toast");
  t.textContent = text;
  t.className = error ? "error" : "";
  t.hidden = false;
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => (t.hidden = true), error ? 6000 : 2500);
}

function renderDays() {
  const nav = $("days");
  nav.replaceChildren(
    ...state.days.map((day, i) =>
      el(
        "button",
        {
          class: [i === state.current && "current", isPicked(day) && "done", !day.editable && "locked"]
            .filter(Boolean).join(" "),
          title: day.editable ? "" : "Menu can no longer be changed",
          onclick: () => show(i),
        },
        dateLabel(day.date, { weekday: "short", day: "numeric", month: "numeric" }),
        el("span", { class: "count" }, day.editable ? `${pickedCount(day)}/${day.meals.length}` : "locked"),
      ),
    ),
  );
}

function facts(dish) {
  const parts = [];
  if (dish.kcal != null) parts.push(`${Math.round(dish.kcal)} kcal`);
  if (dish.protein != null) parts.push(`P ${Math.round(dish.protein)}`);
  if (dish.carb != null) parts.push(`C ${Math.round(dish.carb)}`);
  if (dish.fat != null) parts.push(`F ${Math.round(dish.fat)}`);
  return parts.join(" · ");
}

function tile(day, meal, dish) {
  const selected = meal.selected === dish.id;
  return el(
    "button",
    {
      class: `tile${selected ? " selected" : ""}${selected && !meal.user_selected ? " default" : ""}`,
      disabled: !day.editable,
      title: [dish.name, selected && !meal.user_selected ? "(default dish, not chosen yet)" : null, dish.allergens.length ? `Alergeny: ${dish.allergens.join(", ")}` : null]
        .filter(Boolean).join("\n"),
      "aria-pressed": String(selected),
      onclick: (e) => choose(day, meal, dish, e.currentTarget),
    },
    el("img", { src: dish.image ? `/img/${encodeURIComponent(dish.image)}?r=MEDIUM` : null, alt: "", loading: "lazy" }),
    el("span", { class: "name" }, dish.name),
    el("span", { class: "facts" }, facts(dish)),
  );
}

function renderDay() {
  const day = state.days[state.current];
  $("prev").disabled = state.current === 0;
  $("next").disabled = state.current >= state.days.length - 1;
  $("day-title").textContent = dateLabel(day.date, { weekday: "long", day: "numeric", month: "long" });

  const status = $("day-status");
  status.className = day.editable ? "" : "locked";
  if (!day.editable) status.textContent = "Menu locked for this day";
  else if (isPicked(day)) status.textContent = "All meals chosen";
  else status.textContent = `${pickedCount(day)} of ${day.meals.length} meals chosen`;

  $("meals").replaceChildren(
    ...day.meals.map((meal) =>
      el(
        "section",
        { class: `meal${meal.user_selected ? " picked" : ""}` },
        el("div", { class: "meal-label" },
          el("strong", {}, meal.name || meal.key),
          el("span", {}, `${meal.options.length} options`)),
        el("div", { class: "tiles" }, ...meal.options.map((dish) => tile(day, meal, dish))),
      ),
    ),
  );
  renderDays();
}

function show(index) {
  state.current = Math.max(0, Math.min(index, state.days.length - 1));
  renderDay();
  window.scrollTo({ top: 0, behavior: "smooth" });
}

async function choose(day, meal, dish, button) {
  if (!day.editable) return;
  const wasPicked = isPicked(day);
  const previous = { selected: meal.selected, user_selected: meal.user_selected };
  const changed = meal.selected !== dish.id;

  meal.selected = dish.id;
  meal.user_selected = true;
  renderDay();

  if (changed) {
    try {
      button.classList.add("saving");
      const resp = await fetch("/api/select", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ delivery_id: meal.delivery_id, item_id: meal.item_id, product_id: dish.id }),
      });
      if (!resp.ok) throw new Error((await resp.json().catch(() => ({}))).error || resp.statusText);
    } catch (err) {
      Object.assign(meal, previous);
      renderDay();
      toast(`Could not save: ${err.message}`, true);
      return;
    }
  }

  if (!wasPicked && isPicked(day) && state.days[state.current] === day) {
    const next = state.days.findIndex((d, i) => i > state.current && d.editable && !isPicked(d));
    toast(next === -1 ? "All days planned 🎉" : "Day complete → next day");
    if (next !== -1) setTimeout(() => state.days[state.current] === day && show(next), ADVANCE_DELAY_MS);
  }
}

async function load() {
  const msg = $("message");
  msg.hidden = false;
  msg.textContent = "Loading menu…";
  try {
    const resp = await fetch("/api/days");
    const data = await resp.json();
    if (!resp.ok) throw new Error(data.error || resp.statusText);
    $("mode").hidden = !data.demo;
    state.days = data.days;
    if (!state.days.length) {
      msg.textContent = "No upcoming deliveries with a menu to choose.";
      return;
    }
    msg.hidden = true;
    const firstOpen = state.days.findIndex((d) => d.editable && !isPicked(d));
    show(firstOpen === -1 ? 0 : firstOpen);
  } catch (err) {
    msg.textContent = `Could not load deliveries: ${err.message}`;
  }
}

$("prev").addEventListener("click", () => show(state.current - 1));
$("next").addEventListener("click", () => show(state.current + 1));
document.addEventListener("keydown", (e) => {
  if (e.key === "ArrowLeft") show(state.current - 1);
  if (e.key === "ArrowRight") show(state.current + 1);
});

load();
