# ntfy-dashboard

An at-a-glance meal picker for [Nice To Fit You](https://ntfy.pl). It replaces the app's swipe-through
menu with one screen per day: 5 meal rows, each showing every dish you can choose. Click a tile to
pick that dish. When all 5 meals of a day are picked, it moves on to the next open day.

Rust (axum) backend that talks to the NTFY API. Plain HTML/CSS/JS frontend, embedded in the binary.

## Run

```sh
cp .env.example .env   # then put your NTFY email and password in it
cargo run --release
# open http://127.0.0.1:8080
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
- `NTFY_DB` sets the path to the local archive (default `./ntfy.db`).

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

Every change is checked against NTFY's reply and logged in the archive.

## History

NTFY's API only returns deliveries from about three days back, so the dashboard archives everything
it sees into SQLite (`ntfy.db`): every meal with the full raw API JSON, all dishes with
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
