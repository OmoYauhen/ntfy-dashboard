# nfty-dashboard

An at-a-glance meal picker for [Nice To Fit You](https://ntfy.pl). It replaces the app's swipe-through
menu with one screen per day: 5 meal rows, each showing every dish you can choose. Click a tile to
pick that dish. When all 5 meals of a day are picked, it moves on to the next open day.

Rust (axum) backend that talks to the NTFY API. Plain HTML/CSS/JS frontend, embedded in the binary.

## Run

```sh
# credentials: NFTY_EMAIL + NFTY_PASSWORD, or an `email:password` file (default ./nfty_creds.txt)
cargo run --release
# open http://127.0.0.1:8080
```

Demo mode needs no account. It builds days from the public menu (the dishes served by the
selectable diets) and keeps selections in memory:

```sh
cargo run --release -- --demo
```

Options (environment variables):
- `NFTY_BIND` sets the listen address (default `127.0.0.1:8080`).
- `NFTY_CREDS` sets the path to the credentials file.
- `NFTY_DEMO=1` is the same as `--demo`.

## UI

- The top strip lists upcoming days with how many meals are picked (`3/5`). Locked days are dimmed.
- A solid green tile is your choice. A dashed tile is the caterer's default, which you haven't confirmed.
- Use ← / → or the arrow buttons to move between days.
- Hover over a tile to see its full name and allergens.

## Layout

- `src/orion.rs`: API client (login and refresh, deliveries, menu, item change, image proxy).
- `src/model.rs`: turns API responses into days → meals → dishes.
- `src/main.rs`: HTTP server (`/api/days`, `/api/select`, `/img/{id}`).
- `web/`: frontend.
- `API.md`: reverse-engineered API notes.
