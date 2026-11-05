"use strict";

const ADVANCE_DELAY_MS = 700;

// `row` is the meal that keyboard picks go to.
const state = { days: [], current: 0, row: 0 };

const isPicked = (day) => day.meals.every((m) => m.user_selected);
const hasMenu = (day) => day.meals.some((m) => m.options.length);
const isOpen = (day) => day.editable && hasMenu(day) && !isPicked(day);
const pickedCount = (day) => day.meals.filter((m) => m.user_selected).length;
const currentDay = () => state.days[state.current];
const firstUnpicked = (day) => Math.max(0, day.meals.findIndex((m) => !m.user_selected));

function renderDays() {
  $("days").replaceChildren(
    ...state.days.map((day, i) =>
      el(
        "button",
        {
          class: [i === state.current && "current", isPicked(day) && "done", !(day.editable && hasMenu(day)) && "locked"]
            .filter(Boolean).join(" "),
          title: day.editable ? "" : "Menu can no longer be changed",
          onclick: () => show(i),
        },
        dateLabel(day.date, { weekday: "short", day: "numeric", month: "numeric" }),
        el("span", { class: "count" },
          !day.editable ? "locked" : !hasMenu(day) ? "no menu" : `${pickedCount(day)}/${day.meals.length}`),
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

function badges(dish) {
  const h = dish.history;
  return el("span", { class: "badges" },
    dish.rating != null ? el("span", { class: "stars", title: `${dish.ratings_count} NTFY ratings` }, `★ ${dish.rating.toFixed(1)}`) : null,
    h ? el("span", { class: "eaten", title: `Last eaten ${h.last_eaten}` },
      `had ${h.eaten}×${h.your_rating != null ? ` · you ★${round(h.your_rating, 1)}` : ""}`) : null,
  );
}

function tile(day, meal, dish, index) {
  const selected = meal.selected === dish.id;
  return el(
    "button",
    {
      class: `tile${selected ? " selected" : ""}${selected && !meal.user_selected ? " default" : ""}${dish.history ? " eaten-before" : ""}`,
      title: dish.history ? `You had this dish ${dish.history.eaten}×, last on ${dish.history.last_eaten}` : null,
      disabled: !day.editable,
      "aria-pressed": String(selected),
      "aria-label": dish.name,
      onclick: (e) => choose(day, meal, dish, e.currentTarget),
      // Mouse only: on touch screens a tap should just pick the dish.
      onpointerenter: (e) => e.pointerType === "mouse" && scheduleZoom(day, meal, dish, e.currentTarget),
      onpointerleave: (e) => { if (!$("zoom").contains(e.relatedTarget)) closeZoom(); },
    },
    el("img", { src: imageUrl(dish.image), alt: "", loading: "lazy" }),
    index < 9 ? el("kbd", {}, String(index + 1)) : null,
    badges(dish),
    el("span", { class: "name" }, dish.name),
    el("span", { class: "facts" }, facts(dish)),
  );
}

// Hovering a tile grows it into a bigger card on top: the card starts exactly over the tile
// (scaled down, clipped to the tile's height) and animates to full size, revealing the details.
const ZOOM_DELAY_MS = 120;
const ZOOM_MS = 260;
const ZOOM_WIDTH = 400;
const zoom = { timer: null, anchor: null, hideTimer: null };

function scheduleZoom(day, meal, dish, anchor) {
  clearTimeout(zoom.timer);
  // Moving from one open card straight onto another tile opens it without delay.
  zoom.timer = setTimeout(() => openZoom(day, meal, dish, anchor), zoom.anchor ? 0 : ZOOM_DELAY_MS);
}

/// Transform + clip that make the (unscaled) card cover exactly `rect`.
function tileTransform(rect, left, top, width, height) {
  const scale = rect.width / width;
  const visible = rect.height / scale;
  return {
    transform: `translate(${rect.left - left}px, ${rect.top - top}px) scale(${scale})`,
    clipPath: `inset(0 0 ${Math.max(0, height - visible)}px 0 round ${10 / scale}px)`,
  };
}

function openZoom(day, meal, dish, anchor) {
  if (!anchor.isConnected) return;
  const card = $("zoom");
  clearTimeout(zoom.hideTimer);
  const selected = meal.selected === dish.id;

  const img = el("img", { src: imageUrl(dish.image), alt: "" });
  const sharp = new Image();
  sharp.onload = () => { if (img.isConnected) img.src = sharp.src; };
  sharp.src = imageUrl(dish.image, "MEDIUM-RETINA");

  let action = null;
  if (day.editable) {
    action = !selected ? "Click to choose" : meal.user_selected ? "✓ Your choice" : "Default dish · click to keep it";
  }
  card.replaceChildren(...[
    el("div", { class: "zoom-media" }, img, badges(dish)),
    el("div", { class: "zoom-head" },
      el("h3", {}, dish.name),
      el("p", { class: "facts" }, facts(dish)),
      dish.history ? el("p", { class: "hint" }, `You had it ${dish.history.eaten}×, last on ${dish.history.last_eaten}.`) : null),
    dishDetails(dish),
    action ? el("p", { class: "zoom-action" }, action) : null,
  ].filter(Boolean));
  card.className = [selected && "selected", selected && !meal.user_selected && "default", !day.editable && "locked", dish.history && "eaten-before"]
    .filter(Boolean).join(" ");
  card.onclick = day.editable ? () => { closeZoom(true); choose(day, meal, dish, null); } : null;
  card.onpointerleave = (e) => { if (e.relatedTarget !== anchor) closeZoom(); };

  // Final geometry: wider than the tile, image centred over the tile's image, kept on screen.
  const r = anchor.getBoundingClientRect();
  const width = Math.min(innerWidth - 16, Math.max(ZOOM_WIDTH, r.width * 1.5));
  card.style.transition = "none";
  card.style.transform = "none";
  card.style.clipPath = "none";
  card.style.width = `${width}px`;
  card.hidden = false;
  const height = card.offsetHeight;
  const imageGrowth = (width - r.width) * 0.75;
  const left = Math.min(Math.max(8, r.left + r.width / 2 - width / 2), innerWidth - width - 8);
  const top = Math.min(Math.max(8, r.top - imageGrowth / 2), innerHeight - height - 8);
  card.style.left = `${left}px`;
  card.style.top = `${Math.max(8, top)}px`;

  const reopen = zoom.anchor === anchor;
  zoom.anchor = anchor;
  zoom.geometry = { left, top: Math.max(8, top), width, height };
  if (reopen) return;
  Object.assign(card.style, tileTransform(r, left, Math.max(8, top), width, height));
  card.style.opacity = "0.4";
  card.getBoundingClientRect(); // commit the start state before animating
  card.style.transition = "";
  Object.assign(card.style, { transform: "none", clipPath: "inset(0 0 0 0 round 14px)", opacity: "1" });
}

/// Shrinks the card back onto its tile (or just fades it when `fade`, e.g. after a click).
function closeZoom(fade = false) {
  clearTimeout(zoom.timer);
  const card = $("zoom");
  const anchor = zoom.anchor;
  zoom.anchor = null;
  if (card.hidden || !anchor) return;
  if (!fade && anchor.isConnected) {
    const { left, top, width, height } = zoom.geometry;
    Object.assign(card.style, tileTransform(anchor.getBoundingClientRect(), left, top, width, height));
  }
  card.style.opacity = "0";
  clearTimeout(zoom.hideTimer);
  zoom.hideTimer = setTimeout(() => { if (!zoom.anchor) card.hidden = true; }, ZOOM_MS);
}

function renderDay() {
  const day = currentDay();
  $("prev").disabled = state.current === 0;
  $("next").disabled = state.current >= state.days.length - 1;
  $("day-title").textContent = dateLabel(day.date, { weekday: "long", day: "numeric", month: "long" });

  const status = $("day-status");
  status.className = day.editable ? "" : "locked";
  if (!day.editable) status.textContent = "Menu locked for this day";
  else if (!hasMenu(day)) {
    status.textContent = day.menu_from
      ? `Menu not published yet, available from ${dateLabel(day.menu_from, { weekday: "long", day: "numeric", month: "long" })}`
      : "Menu not published yet";
  } else if (isPicked(day)) status.textContent = "All meals chosen";
  else status.textContent = `${pickedCount(day)} of ${day.meals.length} meals chosen`;

  const kcal = day.meals.reduce((sum, m) => sum + (m.options.find((d) => d.id === m.selected)?.kcal ?? 0), 0);
  $("day-kcal").textContent = kcal ? `${Math.round(kcal)} kcal` : "";
  $("keep-rest").hidden = !(day.editable && hasMenu(day) && !isPicked(day));

  // One column per option of the largest meal, so every meal stays on a single row.
  $("meals").style.setProperty("--cols", Math.max(1, ...day.meals.map((m) => m.options.length)));
  $("meals").replaceChildren(
    ...day.meals.map((meal, i) =>
      el(
        "section",
        {
          class: `meal${meal.user_selected ? " picked" : ""}${i === state.row && day.editable && hasMenu(day) ? " active" : ""}`,
          onclick: () => { state.row = i; },
        },
        el("div", { class: "meal-label" },
          el("strong", {}, meal.name || meal.key),
          el("span", {}, `${meal.options.length} options`)),
        el("div", { class: "tiles" }, ...meal.options.map((dish, j) => tile(day, meal, dish, j))),
      ),
    ),
  );
  renderDays();
}

function show(index) {
  state.current = Math.max(0, Math.min(index, state.days.length - 1));
  state.row = firstUnpicked(currentDay());
  closeZoom(true);
  renderDay();
}

async function save(meal, dish, fromId) {
  const resp = await fetch("/api/select", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({
      delivery_id: meal.delivery_id, item_id: meal.item_id, product_id: dish.id, from_product_id: fromId ?? null,
    }),
  });
  if (!resp.ok) throw new Error((await resp.json().catch(() => ({}))).error || resp.statusText);
}

/// Picks `dish` for `meal`. Choosing the current dish confirms it, which NTFY records as your choice.
async function choose(day, meal, dish, button) {
  if (!day.editable) return;
  const wasPicked = isPicked(day);
  const previous = { selected: meal.selected, user_selected: meal.user_selected };

  meal.selected = dish.id;
  meal.user_selected = true;
  const index = day.meals.indexOf(meal);
  if (state.days[state.current] === day && index === state.row) state.row = firstUnpicked(day);
  renderDay();

  try {
    button?.classList.add("saving");
    await save(meal, dish, previous.selected);
  } catch (err) {
    Object.assign(meal, previous);
    renderDay();
    toast(`Could not save: ${err.message}`, true);
    return;
  }
  afterPick(day, wasPicked);
}

function afterPick(day, wasPicked) {
  if (!wasPicked && isPicked(day) && currentDay() === day) {
    const next = state.days.findIndex((d, i) => i > state.current && isOpen(d));
    toast(next === -1 ? "All days planned 🎉" : "Day complete → next day");
    if (next !== -1) setTimeout(() => currentDay() === day && show(next), ADVANCE_DELAY_MS);
  }
}

/// Confirms the current dish of every meal not chosen yet.
async function keepRest() {
  const day = currentDay();
  const wasPicked = isPicked(day);
  const pending = day.meals.filter((m) => !m.user_selected && m.selected != null);
  const results = await Promise.allSettled(
    pending.map((meal) => save(meal, meal.options.find((d) => d.id === meal.selected) ?? { id: meal.selected }, meal.selected)),
  );
  results.forEach((r, i) => { if (r.status === "fulfilled") pending[i].user_selected = true; });
  const failed = results.filter((r) => r.status === "rejected");
  renderDay();
  if (failed.length) toast(`Could not save ${failed.length} meal(s): ${failed[0].reason.message}`, true);
  else afterPick(day, wasPicked);
}

async function load() {
  const msg = $("message");
  msg.hidden = false;
  msg.textContent = "Loading menu…";
  try {
    const resp = await fetch("/api/days");
    const data = await resp.json();
    if (!resp.ok) throw new Error(data.error || resp.statusText);
    $("mode").hidden = !(data.demo || data.read_only);
    $("mode").textContent = data.demo ? "demo" : "read-only";
    $("history-link").hidden = data.demo;
    state.days = data.days;
    if (!state.days.length) {
      msg.textContent = "No upcoming deliveries with a menu to choose.";
      return;
    }
    msg.hidden = true;
    const firstOpen = state.days.findIndex(isOpen);
    show(firstOpen === -1 ? 0 : firstOpen);
  } catch (err) {
    msg.textContent = `Could not load deliveries: ${err.message}`;
  }
}

$("prev").addEventListener("click", () => show(state.current - 1));
$("next").addEventListener("click", () => show(state.current + 1));
$("keep-rest").addEventListener("click", keepRest);
addEventListener("blur", () => closeZoom(true));
addEventListener("resize", () => closeZoom(true));

// Keyboard flow: 1-9 picks a dish in the active meal, Enter keeps its current dish,
// ↑/↓ moves between meals, ←/→ between days.
document.addEventListener("keydown", (e) => {
  closeZoom(true);
  if (e.ctrlKey || e.metaKey || e.altKey || !state.days.length) return;
  const day = currentDay();
  const meal = day.meals[state.row];
  if (e.key === "ArrowLeft") show(state.current - 1);
  else if (e.key === "ArrowRight") show(state.current + 1);
  else if (e.key === "ArrowUp" || e.key === "ArrowDown") {
    state.row = Math.max(0, Math.min(day.meals.length - 1, state.row + (e.key === "ArrowUp" ? -1 : 1)));
    renderDay();
  } else if (/^[1-9]$/.test(e.key) && meal?.options[Number(e.key) - 1]) {
    choose(day, meal, meal.options[Number(e.key) - 1], null);
  } else if (e.key === "Enter" && meal?.selected != null && !e.target.closest?.("button")) {
    choose(day, meal, meal.options.find((d) => d.id === meal.selected), null);
  } else return;
  e.preventDefault();
});

load();
