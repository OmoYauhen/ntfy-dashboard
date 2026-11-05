"use strict";

// Helpers shared by the planner and the history page.

const $ = (id) => document.getElementById(id);

const dateLabel = (iso, opts) => new Date(`${iso.slice(0, 10)}T12:00:00`).toLocaleDateString(undefined, opts);

function el(tag, attrs = {}, ...children) {
  const node = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (k === "class") node.className = v;
    else if (k.startsWith("on")) node.addEventListener(k.slice(2), v);
    else if (v !== false && v != null) node.setAttribute(k, v === true ? "" : v);
  }
  node.append(...children.flat(Infinity).filter((c) => c != null && c !== false));
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

const imageUrl = (id, resolution = "MEDIUM") => (id ? `/img/${encodeURIComponent(id)}?r=${resolution}` : null);
const round = (v, digits = 0) => (v == null ? null : Number(v).toFixed(digits).replace(/\.0+$/, ""));

/// Full nutrition and ingredient details of a dish, as a definition list.
/// `d` uses the dashboard dish fields (kcal, protein, ..., composition, allergens, may_contain).
function dishDetails(d) {
  const rows = [
    ["Energy", d.kcal != null ? `${round(d.kcal)} kcal${d.kj != null ? ` / ${round(d.kj)} kJ` : ""}` : null],
    ["Weight", d.weight != null ? `${round(d.weight)} g` : null],
    ["Protein", d.protein != null ? `${round(d.protein, 1)} g` : null],
    ["Carbs", d.carb != null ? `${round(d.carb, 1)} g${d.sugar != null ? ` (sugar ${round(d.sugar, 1)} g)` : ""}` : null],
    ["Fat", d.fat != null ? `${round(d.fat, 1)} g${d.saturated_fat != null ? ` (saturated ${round(d.saturated_fat, 1)} g)` : ""}` : null],
    ["Fiber", d.fiber != null ? `${round(d.fiber, 1)} g` : null],
    ["Salt", d.salt != null ? `${round(d.salt, 2)} g` : null],
    ["NTFY rating", d.rating != null ? `★ ${round(d.rating, 2)} (${d.ratings_count ?? 0} ratings)` : null],
    ["Allergens", d.allergens?.length ? d.allergens.join(", ") : null],
    ["Tags", d.tags?.length ? d.tags.join(", ") : null],
  ].filter(([, v]) => v);
  return el("div", { class: "details" },
    el("dl", {}, rows.map(([k, v]) => [el("dt", {}, k), el("dd", {}, v)])),
    d.composition ? el("p", { class: "composition" }, d.composition) : null,
    d.may_contain ? el("p", { class: "may-contain" }, d.may_contain) : null,
  );
}
