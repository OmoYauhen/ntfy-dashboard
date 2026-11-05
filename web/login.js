"use strict";

// Only follow local paths after login, never another site.
const next = (() => {
  const target = new URLSearchParams(location.search).get("next") || "/";
  return target.startsWith("/") && !target.startsWith("//") ? target : "/";
})();

fetch("/api/session").then((r) => r.json()).then((s) => {
  if (s.logged_in) location.replace(next);
});

$("login").addEventListener("submit", async (e) => {
  e.preventDefault();
  const error = $("error");
  const button = $("submit");
  error.hidden = true;
  button.disabled = true;
  button.textContent = "Logging in…";
  try {
    const resp = await fetch("/api/login", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ email: $("email").value, password: $("password").value }),
    });
    const data = await resp.json().catch(() => ({}));
    if (!resp.ok) throw new Error(data.error || resp.statusText);
    $("password").value = "";
    location.replace(next);
  } catch (err) {
    error.textContent = err.message;
    error.hidden = false;
    button.disabled = false;
    button.textContent = "Log in";
  }
});
