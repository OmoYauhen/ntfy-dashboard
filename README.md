# ntfy-dashboard

An at-a-glance meal picker for [Nice To Fit You](https://ntfy.pl). It replaces the app's swipe-through
menu with one screen per day: 5 meal rows, each showing every dish you can choose. Click a tile to
pick that dish. When all 5 meals of a day are picked, it moves on to the next open day.

Rust (axum) backend that talks to the NTFY API. Plain HTML/CSS/JS frontend, embedded in the binary.

## Run

```sh
cargo run --release
# open http://127.0.0.1:8080 and log in with your NTFY account
```

Demo mode needs no account. It builds days from the public menu (the dishes served by the
selectable diets) and keeps selections in memory:

```sh
cargo run --release -- --demo
```

Options (environment variables, or `.env`):
- `NTFY_BIND` sets the listen address (default `127.0.0.1:8080`).
- `NTFY_DEMO=1` is the same as `--demo`.
- `NTFY_READONLY=1` shows your real plan but refuses to send changes to NTFY.
- `NTFY_DATA_DIR` sets where the per-account archives `ntfy-<user id>.db` live (default `.`).
- `NTFY_IMAGE_CACHE` sets the photo cache directory (default `./cache/images`).

## Login

There are no stored credentials. The login page checks your email and password with NTFY and the
server keeps them in memory only (wiped on logout), so it can log in again when NTFY's session
expires. They're gone after **Log out** or a server restart; until you log in again the hourly
archive sync pauses.

Each browser that logs in gets its own session cookie (HttpOnly, 30 days). Everything except dish
photos requires it, so people who can reach the URL can't see or change your plan. Logging out
ends every browser's session.

## Planner

- The top strip lists upcoming days with how many meals are picked (`3/5`). Locked days and days whose
  menu isn't published yet are dimmed.
- A solid green tile is your choice. A dashed tile is the caterer's default, which you haven't confirmed.
- Tiles show NTFY's community rating (★). Dishes you had before (from your archive, any calorie size) get an
  amber frame and a `had 2×` badge.
- The ⓘ button on a tile opens it as a bigger card (animated, with a slow photo pan) showing everything:
  composition, allergens, all nutrients, weight, tags. Close it with ×, Esc or a click outside.
- Keyboard: `1`–`9` picks a dish in the highlighted meal and moves to the next one, `Enter` keeps the
  current dish, `↑`/`↓` changes the meal, `←`/`→` changes the day.
- **Keep the rest** confirms the current dish for every meal you haven't chosen yet.
- When all meals of a day are chosen, it moves on to the next open day.
- Each day shows how long it can still be changed (`locks in 2d 5h`). Under 24 hours the time also
  appears in the day strip, in amber. A day locks on screen the moment its deadline passes.
- Days whose menu was published since you last looked get a green dot. The page reloads the plan
  every 15 minutes (and when you come back to the tab), and tells you about newly published menus.
- Switching days is instant. The server downloads every dish photo into a local cache after each
  hourly sync (about 100 MB for a month of menus), and the browser preloads them, nearest days first.

Every change is checked against NTFY's reply and logged in the archive.

## History

NTFY's API only returns deliveries from about three days back, so the dashboard archives everything
it sees into SQLite (`ntfy-<user id>.db`, one file per account): every meal with the full raw API JSON, all dishes with
composition and nutrients, your ratings and comments, prices, diets, orders, and a log of the
choices you made here. It syncs at startup, every hour, whenever the planner loads, and on
**Sync now**. Keep it running, or at least start it every couple of days, so no day is missed.

`/history` shows per-day totals, averages, your most-eaten dishes and your orders.
`/api/export.csv` gives one row per meal with every nutrient; `/api/export.json` dumps the whole archive.

## Layout

- `src/orion.rs`: API client (login and refresh, deliveries, menu, item change, image proxy).
- `src/model.rs`: turns API responses into days → meals → dishes.
- `src/db.rs`: SQLite archive (history, dish stats, exports).
- `src/main.rs`: HTTP server (`/api/days`, `/api/select`, `/img/{id}`).
- `web/`: frontend.
- `API.md`: reverse-engineered API notes.

## License

MIT, see [LICENSE](LICENSE).
