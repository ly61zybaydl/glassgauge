**English** | [简体中文](README.md)

# glassgauge

> A liquid-glass usage widget for the [mirasim](https://mirasim.ai) relay on Windows,
> with one-click subscription-account switching. Tauri v2 + native DirectX.

A **mirasim usage widget** for the Windows desktop, styled after iOS 26 liquid glass:
the glass refracts the **live content actually behind the window** (not a wallpaper
snapshot), with edge displacement. Built on Tauri v2 with a native DirectX render
pipeline; resident footprint ~40 MB. Ships with a zero-dependency account-switcher
CLI (see `cli/`).

![glass](docs/design/glass-sample.png)

## Repository layout

- `ui/`, `src-tauri/` — the glassgauge widget itself (front end + Tauri/Rust native engine)
- `cli/` — the same account switching as a command line tool (Node, zero deps, shares the widget's on-disk layout; the two are interchangeable)
- `docs/design/` — design docs for the widget and the refraction engine

## What it does

- Auto-discovers the local mirasim relay (the port is assigned dynamically — it scans
  and claims by response shape) and polls `GET /v1/limits`.
- Always-expanded panel with three window cards (5 hours / 7 days / 30 days), each
  showing used %, the raw used/budget amounts (v0.8.0, k-abbreviated, same as mirasim's
  own panel), an even-pace tick, ahead/behind, and a reset countdown.
- **Subscription-account switching** (v0.2.0): open the "account" row in the panel to
  list saved snapshots, then click one to switch the logged-in mirasim account — no
  re-entering an email code. Perfect for hopping to a spare account when a quota runs
  out. Accounts are identified by **email** (v0.4.0, decoded from the token JWT — see below).
- **Transparency control** (v0.3.0): the ⚙ in the header opens sliders for veil (α,
  where 0 = pure clear glass) and frost (blur σ). Dragging applies instantly and is
  written back to `config.json` — in `refract` mode via the engine's `SetCfg` hot
  reload, in `wallpaper` mode by rebuilding the CSS filter; `live` mode's blur is fixed
  by DWM, so only the veil is offered there. The veil can go all the way to **100% = a
  solid white panel** (v0.7.0; at veil ≥ 50% the text/ticks switch to dark and cards get a
  faint dark tint, so it stays readable over a dark wallpaper).
- **Self-heals on boot** (v0.7.0): the widget usually starts on login and may come up
  before mirasim, so it shows "waiting for Mirasim…" and retries briskly (≤5s), recovering
  within seconds once the relay is ready. The poll loop is wrapped in try/finally plus a
  watchdog, so no exception can ever leave it stuck on "unavailable".
- Free dragging, multi-monitor aware (including mixed DPI scaling), remembers its
  position, and degrades to the last-known data when the relay drops.

## Account switching

The login state lives in the `auth` block of `~/.mirasim/setting.json` (the tokens are
`mrs1:`-prefixed ciphertext bound to the machine's `secret.key`, so snapshots are only
valid on the same machine). glassgauge and the bundled `cli/` tool share one on-disk
layout (`~/.mirasim/_account_switcher/{profiles,backups}`) and can be used interchangeably.

- **First-time collection:** while logged in as account A, click "＋ save current login
  as a snapshot"; then in mirasim sign out, sign in as account B, and save again. After
  that, click whichever you want to switch to.
- Before every switch it re-saves the current account's latest login state to its
  snapshot (keeping the refreshToken fresh) and backs up the whole `setting.json`
  (keeps the last 20; roll back with the CLI's `restore` if needed).
- A switch only rewrites the `auth` field; the mirasim server hot-reloads within a few
  seconds. An in-flight session may glitch briefly, and if the switch coincides with the
  app refreshing and rewriting the token file it can be overwritten — just click once more.
- The panel still expands when the relay is down, so switching stays available (the quota
  cards show an empty state) — which is exactly when a broken login needs rescuing.
- Listing and deleting snapshots (✕, two-step confirm) happen entirely inside the panel;
  tokens never enter the WebView — `invoke` only ever returns metadata.
- **Accounts are shown by email:** `auth.token` (the `mrs1:` ciphertext) decrypts locally
  to a JWT whose `email` claim is the account's address. The chain is: DPAPI restores
  `secret.key` (`CryptUnprotectData`, decryptable by the current user) → AES-256-GCM
  decrypts the token → read the JWT. Everything is local and offline; the ciphertext never
  leaves the process. If it can't be decrypted (a snapshot from another machine, or the key
  is unreadable) it falls back to the account's display name. New snapshots default to being
  named after the email's local part.
- **The plan badge and expiry follow the account** (v0.5.0): the header's plan badge and
  "plan expiry" come from the same JWT's `plan` / `plan_exp` claims, so they refresh the
  moment you switch accounts. They fall back to `planLabel` / `validUntil` in the config
  only when the token can't be decrypted (so those two config keys are just defaults now).
- **Usage is attributed to the right account** (v0.6.0): the `/v1/limits` response carries a
  `subject` (= the account's userId). After a switch the relay takes a few seconds to tens of
  seconds to move its limits to the new account, and during that window `subject` still points
  at the old account. The widget uses this: when `subject` doesn't equal the logged-in userId
  it stops showing those numbers (otherwise you'd see the previous account's usage), shows
  "syncing the new account's usage…" instead, and fast-polls (every 3s, up to ~150s) until the
  relay catches up. If it stays stale for a long time, restarting Mirasim forces a refresh.

## Liquid-glass engine

Three glass modes (the `mode` config key):

| Mode | What the glass shows | Notes |
| --- | --- | --- |
| `refract` (default) | the **live picture** behind the window | DXGI desktop-duplication capture (event-driven + dirty-region filtered, zero cost when static) → Direct2D Gaussian blur → displacement-map refraction → saturation → 20px rounded-corner AA → composited behind the WebView content via DirectComposition |
| `wallpaper` | the wallpaper, cropped and refracted at the window's physical position | automatic fallback when `refract` is unavailable (lock screen / UAC / exclusive fullscreen / remote desktop); can also be forced |
| `live` | DWM acrylic real-time blur | system material, fixed ~8px corners |

**Screenshot note:** in `refract` mode the widget must be excluded from screen capture
(otherwise the glass captures itself in a feedback loop), so **it is invisible in
screenshots / screen recordings**. To capture it, tick "screenshot mode (glass falls back
to wallpaper)" in the tray, capture, then untick to return to live glass.

## Build

```powershell
# Requires: Rust (MSVC), tauri-cli 2.x, the WebView2 runtime (bundled with Win10 2004+)
cd src-tauri && cargo build          # debug
tauri build                          # release + NSIS installer
cargo test                           # Rust unit tests (geometry / displacement map / discovery / token)
node --test ui/tests/*.test.js       # JS unit tests (derived math / crop mapping / account view)
```

## Configuration

`%APPDATA%\glassgauge\config.json` (auto-generated on first run), hot-reloaded by the
tray's "refresh now":

```jsonc
{
  "mode": "refract",          // refract | live | wallpaper
  "expand": "always",         // always = always expanded | hover = expand on hover
  "autostart": true,          // start on login (HKCU Run key, follows the exe's location)
  "accent": "auto",           // accent: auto = sample from wallpaper (avoiding green) | blue | amber | ink | "#hex"
  "ink": "#000000",           // optional: pin the text color (omit = auto black/white by wallpaper brightness)
  "planLabel": "MAX",         // badge text (fallback only, when the plan can't be read from the token)
  "validUntil": "2027-08-11", // plan-expiry fallback (normally taken from the token's plan_exp)
  "refreshSeconds": 60,
  "alwaysOnTop": true,
  "glass": {
    "alpha": 0.03,            // veil density (0 = pure glass)
    "blur": 4,                // frost amount (0 = fully clear, 14 = heavy frost)
    "displacement": 24,       // edge refraction bend strength
    "band": 16,               // refraction edge-band width
    "radiusCollapsed": 20,    // glass corner radius
    "saturate": 1.12
  }
}
```

The transparency sliders in the panel (⚙) write `alpha` and `blur` here for you.

## Design docs

- [Overall widget design](docs/design/2026-08-18-mirasim-usage-glass-widget-design.md)
- [Native real-time refraction engine design](docs/design/2026-08-18-glassgauge-native-refraction-engine-design.md)
- [Engine implementation plan](docs/design/2026-08-18-glassgauge-refraction-engine-plan.md)

Debug builds carry verification entry points: `GG_SPIKE=b|a|cap|pipe` (layer order /
capture exclusion / capture channel / whole-pipeline self-check), `GG_DUMP_ONCE=1`
(auto-dump a glass frame PNG 2.5s after start), and a tray "dump glass frame".

## License

MIT — see [LICENSE](LICENSE).

## Known limitations

- The mouse cursor is not mirrored in the glass (capture excludes the cursor — same as iOS).
- DRM-protected video regions show up black in the glass.
- Requires Windows 10 2004+ (for the capture-exclusion API); earlier systems fall back to
  wallpaper mode automatically.
