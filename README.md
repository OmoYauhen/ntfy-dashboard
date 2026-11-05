# ntfy-dashboard

An at-a-glance meal picker for [Nice To Fit You](https://ntfy.pl). It replaces the app's swipe-through
menu with one screen per day: 5 meal rows, each showing every dish you can choose. Click a tile to
pick that dish. When all 5 meals of a day are picked, it moves on to the next open day.

Rust (axum) backend that talks to the NTFY API. Plain HTML/CSS/JS frontend, embedded in the binary.

## Development

With Nix, `nix develop` gives a shell with Rust, clippy, rustfmt, rust-analyzer, sqlite and
cargo-watch (pinned to nixos-26.05, the same as the package build). Without Nix, any Rust ≥ 1.89
with a C compiler works (SQLite is built in).

```sh
nix develop
cargo run -- --demo        # no account needed: public menu, choices kept in memory
cargo run                  # real account: open http://127.0.0.1:8080 and log in
cargo test
cargo watch -x 'run -- --demo'
```

Build or run the release package directly:

```sh
nix build                  # ./result/bin/ntfy-dashboard
nix run . -- --demo
```

Options (environment variables, or `.env`; see `.env.example`):
- `NTFY_BIND` sets the listen address (default `127.0.0.1:8080`).
- `NTFY_ALLOWED_USERS` is a comma-separated list of NTFY user ids that may log in. Set it whenever
  other people can reach the dashboard; empty allows any NTFY account.
- `NTFY_DEMO=1` is the same as `--demo`.
- `NTFY_READONLY=1` shows your real plan but refuses to send changes to NTFY.
- `NTFY_DATA_DIR` sets where the per-account archives `ntfy-<user id>.db` live (default `.`).
- `NTFY_IMAGE_CACHE` sets the photo cache directory (default `./cache/images`).

## NixOS

The flake exports `nixosModules.default`. `nix/module.nix` also works on its own for configs that
aren't flakes. It runs the server as the `ntfy-dashboard` system user, keeps archives and photos in
`/var/lib/ntfy-dashboard`, and applies systemd sandboxing.

```nix
# flake.nix of your system
inputs.ntfy-dashboard.url = "github:OmoYauhen/ntfy-dashboard";
# in the host's modules
imports = [ inputs.ntfy-dashboard.nixosModules.default ];

# or, without flakes
imports = [ "${builtins.fetchGit { url = "git@github.com:OmoYauhen/ntfy-dashboard.git"; rev = "…"; }}/nix/module.nix" ];

services.ntfy-dashboard = {
  enable = true;
  port = 8440;                 # listens on 127.0.0.1; put a TLS reverse proxy in front
  allowedUsers = [ 1234567 ];  # your NTFY user id (logged on every login)
};
```

Behind the reverse proxy, the session cookie gets `Secure` when the proxy sends
`X-Forwarded-Proto: https`, and failed logins are counted per `X-Forwarded-For` client.

It runs at <https://ntfy.yauhen.io> on n1x1e, deployed from the
[nixie](https://github.com/OmoYauhen/nixie) hive (`modules/ntfy-dashboard.nix`).

## Login

There are no stored credentials. The login page checks your email and password with NTFY and the
server keeps them in memory only (wiped on logout), so it can log in again when NTFY's session
expires. They're gone after **Log out** or a server restart; until you log in again the hourly
archive sync pauses.

Each browser that logs in gets its own session cookie (HttpOnly, 30 days). Everything except dish
photos requires it, so people who can reach the URL can't see or change your plan. Logging out
ends every browser's session.

Only accounts in `NTFY_ALLOWED_USERS` can log in; others are rejected even with a correct password.
Failed logins are limited to 5 per client and 20 overall per 15 minutes, so the login page can't
be used to guess NTFY passwords or get your NTFY account locked.

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
- `src/images.rs`: photo disk cache and prefetching.
- `src/main.rs`: HTTP server, login and sessions, sync loop, exports.
- `web/`: frontend.
- `nix/package.nix`, `nix/module.nix`, `flake.nix`: package, NixOS module, dev shell.
- `API.md`: reverse-engineered API notes.

## License

MIT, see [LICENSE](LICENSE).
