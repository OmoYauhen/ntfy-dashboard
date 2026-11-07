"use strict";

const ADVANCE_DELAY_MS = 700;
const REFRESH_EVERY_MS = 15 * 60 * 1000;
const PRELOAD_CONCURRENCY = 6;
const URGENT_MS = 24 * 3600 * 1000;
const SEEN_KEY = "ntfy-seen-menus";

// `row` is the meal that keyboard picks go to.
const state = { days: [], current: 0, row: 0 };

const isPicked = (day) => day.meals.every((m) => m.user_selected);
const hasMenu = (day) => day.meals.some((m) => m.options.length);
const isOpen = (day) => day.editable && hasMenu(day) && !isPicked(day);
const pickedCount = (day) => day.meals.filter((m) => m.user_selected).length;
const currentDay = () => state.days[state.current];
const msLeft = (day) => (day.change_deadline ? new Date(day.change_deadline) - Date.now() : Infinity);
const isUrgent = (day) => day.editable && hasMenu(day) && !isPicked(day) && msLeft(day) < URGENT_MS;

/// "2d 5h", "5h 20m", "12m".
function formatLeft(ms) {
  const minutes = Math.max(0, Math.floor(ms / 60000));
  const d = Math.floor(minutes / 1440);
  const h = Math.floor((minutes % 1440) / 60);
  const m = minutes % 60;
  if (d) return `${d}d ${h}h`;
  if (h) return `${h}h ${m}m`;
  return `${m}m`;
}

// Days whose menu the user has looked at; a day with a menu not in here is marked "new".
const seen = new Set(JSON.parse(localStorage.getItem(SEEN_KEY) || "[]"));
function markSeen(day) {
  if (!hasMenu(day) || seen.has(day.date)) return;
  seen.add(day.date);
  localStorage.setItem(SEEN_KEY, JSON.stringify([...seen]));
}
const isNew = (day) => hasMenu(day) && day.editable && !isPicked(day) && !seen.has(day.date);

const firstUnpicked = (day) => Math.max(0, day.meals.findIndex((m) => !m.user_selected));

function renderDays() {
  $("days").replaceChildren(
    ...state.days.map((day, i) =>
      el(
        "button",
        {
          class: [i === state.current && "current", isPicked(day) && "done", !(day.editable && hasMenu(day)) && "locked",
            isUrgent(day) && "urgent", isNew(day) && "new"].filter(Boolean).join(" "),
          title: [isNew(day) && "New menu.", !day.editable ? "Menu can no longer be changed"
            : day.change_deadline ? `Can be changed for ${formatLeft(msLeft(day))} more` : ""].filter(Boolean).join(" "),
          onclick: () => show(i),
        },
        dateLabel(day.date, { weekday: "short", day: "numeric", month: "numeric" }),
        el("span", { class: "count" },
          !day.editable ? "locked"
            : !hasMenu(day) ? "no menu"
            : `${pickedCount(day)}/${day.meals.length}${isUrgent(day) ? ` · ${formatLeft(msLeft(day))}` : ""}`),
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
  const node = el(
    "button",
    {
      class: `tile${selected ? " selected" : ""}${selected && !meal.user_selected ? " default" : ""}${dish.history ? " eaten-before" : ""}`,
      title: dish.history ? `You had this dish ${dish.history.eaten}×, last on ${dish.history.last_eaten}` : null,
      // Not `disabled`: locked tiles must still open their details.
      "aria-disabled": String(!day.editable),
      "aria-pressed": String(selected),
      "aria-label": dish.name,
      onclick: (e) => choose(day, meal, dish, e.currentTarget),
    },
    el("img", { src: imageUrl(dish.image), alt: "" }),
    index < 9 ? el("kbd", {}, String(index + 1)) : null,
    badges(dish),
    // A span, since a button can't contain another button.
    el("span", {
      class: "info", role: "button", tabindex: "0", title: "Details", "aria-label": `Details: ${dish.name}`,
      onclick: (e) => { e.stopPropagation(); openZoom(day, meal, dish, node); },
      onkeydown: (e) => {
        if (e.key !== "Enter" && e.key !== " ") return;
        e.preventDefault();
        e.stopPropagation();
        openZoom(day, meal, dish, node);
      },
    }, "i"),
    el("span", { class: "name" }, dish.name),
    el("span", { class: "facts" }, facts(dish)),
  );
  return node;
}

// The details card grows out of its tile: it starts exactly over the tile (scaled down, clipped
// to the tile's height) and animates to full size, revealing the details. Closing reverses it.
const ZOOM_MS = 260;
const ZOOM_WIDTH = 400;
const zoom = { anchor: null, hideTimer: null };

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
  if (zoom.anchor === anchor) return closeZoom();
  const card = $("zoom");
  clearTimeout(zoom.hideTimer);
  const selected = meal.selected === dish.id;

  const img = el("img", { src: imageUrl(dish.image), alt: "" });
  const sharp = new Image();
  sharp.onload = () => { if (img.isConnected) img.src = sharp.src; };
  sharp.src = imageUrl(dish.image, "MEDIUM-RETINA");

  let action = null;
  if (day.editable && selected && meal.user_selected) {
    action = el("p", { class: "zoom-action" }, "✓ Your choice");
  } else if (day.editable) {
    action = el("button", {
      class: "action zoom-choose",
      onclick: () => { closeZoom(true); choose(day, meal, dish, null); },
    }, selected ? "Keep this dish" : "Choose this dish");
  }
  card.replaceChildren(...[
    el("button", { class: "zoom-close", "aria-label": "Close", onclick: () => closeZoom() }, "×"),
    el("div", { class: "zoom-media" }, img, badges(dish)),
    el("div", { class: "zoom-head" },
      el("h3", {}, dish.name),
      el("p", { class: "facts" }, facts(dish)),
      selected && !meal.user_selected && day.editable ? el("p", { class: "hint" }, "Default dish, not chosen yet.") : null,
      dish.history ? el("p", { class: "hint" }, `You had it ${dish.history.eaten}×, last on ${dish.history.last_eaten}.`) : null),
    dishDetails(dish),
    action ? el("div", { class: "zoom-foot" }, action) : null,
  ].filter(Boolean));
  card.className = [selected && "selected", selected && !meal.user_selected && "default", dish.history && "eaten-before"]
    .filter(Boolean).join(" ");

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
  const top = Math.max(8, Math.min(Math.max(8, r.top - imageGrowth / 2), innerHeight - height - 8));
  card.style.left = `${left}px`;
  card.style.top = `${top}px`;

  zoom.anchor = anchor;
  zoom.geometry = { left, top, width, height };
  Object.assign(card.style, tileTransform(r, left, top, width, height));
  card.style.opacity = "0.4";
  card.getBoundingClientRect(); // commit the start state before animating
  card.style.transition = "";
  Object.assign(card.style, { transform: "none", clipPath: "inset(0 0 0 0 round 14px)", opacity: "1" });
  card.querySelector(".zoom-choose, .zoom-close").focus({ preventScroll: true });
}

/// Shrinks the card back onto its tile (or just fades it when `fade`, e.g. after choosing).
function closeZoom(fade = false) {
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
  } else {
    const progress = isPicked(day) ? "All meals chosen" : `${pickedCount(day)} of ${day.meals.length} meals chosen`;
    status.textContent = progress;
    if (day.change_deadline) {
      const at = new Date(day.change_deadline).toLocaleString(undefined,
        { weekday: "short", day: "numeric", month: "short", hour: "2-digit", minute: "2-digit" });
      status.append(" · ", el("span", { class: `deadline${isUrgent(day) ? " urgent" : ""}`, title: `Locks ${at}` },
        `locks in ${formatLeft(msLeft(day))}`));
    }
  }

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
  markSeen(currentDay());
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

/// Loads (or refreshes) the plan. A refresh keeps the day being viewed and the active meal.
async function load({ refresh = false } = {}) {
  const msg = $("message");
  if (!refresh) {
    msg.hidden = false;
    msg.textContent = "Loading menu…";
  }
  try {
    const resp = await fetch("/api/days");
    const data = await resp.json();
    if (!resp.ok) throw new Error(data.error || resp.statusText);
    state.loadedAt = Date.now();
    $("mode").hidden = !(data.demo || data.read_only);
    $("mode").textContent = data.demo ? "demo" : "read-only";
    $("history-link").hidden = data.demo;

    // First visit ever: treat what's already published as seen, so only later days show as new.
    if (!localStorage.getItem(SEEN_KEY)) data.days.forEach(markSeen);
    const viewing = refresh ? currentDay()?.date : null;
    const row = state.row;
    const newMenus = refresh ? data.days.filter((d) => isNew(d) && !state.days.some((o) => o.date === d.date && hasMenu(o))) : [];
    state.days = data.days;
    if (!state.days.length) {
      msg.hidden = false;
      msg.textContent = "No upcoming deliveries with a menu to choose.";
      return;
    }
    msg.hidden = true;
    const keep = state.days.findIndex((d) => d.date === viewing);
    if (keep !== -1) {
      state.current = keep;
      state.row = Math.min(row, currentDay().meals.length - 1);
      renderDay();
    } else {
      const firstOpen = state.days.findIndex(isOpen);
      show(firstOpen === -1 ? 0 : firstOpen);
    }
    if (newMenus.length) toast(`New menu: ${newMenus.map((d) => dateLabel(d.date, { weekday: "short", day: "numeric" })).join(", ")}`);
    preloadImages();
  } catch (err) {
    if (refresh) return; // keep showing the last plan; the next refresh will try again
    msg.textContent = `Could not load deliveries: ${err.message}`;
  }
}

/// Warms the browser cache with every tile photo, nearest days first, so switching days is instant.
const preloaded = new Set();
function preloadImages() {
  const byDistance = state.days
    .map((day, i) => ({ day, distance: Math.abs(i - state.current) }))
    .sort((a, b) => a.distance - b.distance);
  const queue = byDistance
    .flatMap(({ day }) => day.meals.flatMap((m) => m.options.map((d) => imageUrl(d.image))))
    .filter((url) => url && !preloaded.has(url) && preloaded.add(url));
  const next = () => {
    const url = queue.shift();
    if (!url) return;
    const img = new Image();
    img.onload = img.onerror = next;
    img.src = url;
  };
  for (let i = 0; i < PRELOAD_CONCURRENCY; i++) next();
}

// New menus are published daily and the server syncs hourly: refresh the open page now and then,
// and when coming back to it after a while. Skip while a details card is open.
function refreshIfStale(maxAge) {
  if (!zoom.anchor && state.loadedAt && Date.now() - state.loadedAt > maxAge) load({ refresh: true });
}
setInterval(() => refreshIfStale(REFRESH_EVERY_MS - 1000), REFRESH_EVERY_MS);
document.addEventListener("visibilitychange", () => document.visibilityState === "visible" && refreshIfStale(5 * 60 * 1000));

// Keep countdowns current, and lock a day the moment its deadline passes.
setInterval(() => {
  if (!state.days.length) return;
  for (const day of state.days) if (day.editable && msLeft(day) <= 0) day.editable = false;
  if (!zoom.anchor) renderDay();
}, 30 * 1000);

$("prev").addEventListener("click", () => show(state.current - 1));
$("next").addEventListener("click", () => show(state.current + 1));
$("keep-rest").addEventListener("click", keepRest);
// A click outside the open card only closes it; it must not also pick the tile underneath.
document.addEventListener("click", (e) => {
  if (!zoom.anchor || $("zoom").contains(e.target)) return;
  const info = e.target.closest?.(".info");
  if (info && !zoom.anchor.contains(info)) return; // another tile's details: let it open
  e.stopPropagation();
  e.preventDefault();
  closeZoom();
}, true);
addEventListener("resize", () => closeZoom(true));

// Keyboard flow: 1-9 picks a dish in the active meal, Enter keeps its current dish,
// ↑/↓ moves between meals, ←/→ between days.
document.addEventListener("keydown", (e) => {
  if (zoom.anchor) {
    if (e.key === "Escape") { closeZoom(); e.preventDefault(); }
    return;
  }
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
