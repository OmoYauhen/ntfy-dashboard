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
NTFY_EMAIL=… NTFY_PASSWORD=… cargo run   # real account on http://127.0.0.1:8080
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
- `NTFY_EMAIL` and `NTFY_PASSWORD`, or `NTFY_CREDENTIALS_FILE` (a file holding `email:password`), or the
  systemd credential `ntfy-credentials`: the NTFY account. Required except in demo mode.
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

# or, without flakes: the module from a release tag, plus that release's prebuilt binary
let
  version = "0.1.1";
  src = builtins.fetchTarball {
    url = "https://github.com/OmoYauhen/ntfy-dashboard/archive/v${version}.tar.gz";
    sha256 = "…";
  };
in {
  imports = [ "${src}/nix/module.nix" ];
  services.ntfy-dashboard.package = pkgs.stdenvNoCC.mkDerivation {
    pname = "ntfy-dashboard";
    inherit version;
    src = pkgs.fetchurl {
      url = "https://github.com/OmoYauhen/ntfy-dashboard/releases/download/v${version}/ntfy-dashboard-v${version}-x86_64-unknown-linux-musl";
      hash = "sha256-…";
    };
    dontUnpack = true;
    installPhase = "install -Dm755 $src $out/bin/ntfy-dashboard";
    meta.mainProgram = "ntfy-dashboard";
  };
}

services.ntfy-dashboard = {
  enable = true;
  port = 8440;                 # listens on 127.0.0.1 only
  credentialsFile = ./secrets/ntfy-dashboard.cred;
};
```

`credentialsFile` is a `systemd-creds` blob holding `email:password`. Create it on the machine that
runs the service; with `--with-key=tpm2` only that machine's TPM can decrypt it, so the blob is safe in
git and in the Nix store. In fish:

```fish
read -P 'NTFY email: ' email; read -s -P 'NTFY password: ' pw
printf '%s:%s' $email $pw | sudo systemd-creds encrypt --with-key=tpm2 --name=ntfy-credentials - ntfy-dashboard.cred
set -e pw
```

systemd decrypts it at start into a memory-only directory that only the service can read. For a
plain file from agenix or sops-nix instead, set `encryptedCredentials = false` and point
`credentialsFile` at its runtime path (a string, so it isn't copied into the store).

Without `package`, the module compiles the server from source with the host's nixpkgs.

## Releases

Every merge to `main` runs the tests, builds a fully static musl binary (the web UI is embedded),
smoke-tests it in demo mode and publishes it as a GitHub Release with the next patch version
(`.github/workflows/release.yml`). The binary runs on any x86_64 Linux, NixOS included, with no
dependencies.

It runs on fenek, configured in the `fl` flake, behind nginx at <http://ntfy.yauhen.io>. That
name resolves to fenek's Tailscale address, so only the tailnet can reach it.

## Access

There is no login screen. The server logs in to NTFY with the configured credentials at startup,
retrying until it works, and stays logged in. Retries come every minute after network errors, but
only every 30 minutes if NTFY rejects the password, so a wrong password can't get the account
locked. Until it's connected, the header shows the reason.

Because anyone who can reach the dashboard can see and change your plan, it must only be
reachable from a private network. The module listens on `127.0.0.1` by default.

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
- `src/main.rs`: HTTP server, startup login, sync loop, exports.
- `web/`: frontend.
- `nix/package.nix`, `nix/module.nix`, `flake.nix`: package, NixOS module, dev shell.
- `API.md`: reverse-engineered API notes.

## License

MIT, see [LICENSE](LICENSE).
