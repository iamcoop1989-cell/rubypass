# RuBypass GUI — Design Spec
Date: 2026-04-20

## Overview

Cross-platform desktop utility (macOS, Windows, Linux) that manages split-tunnel routing for Russian IP addresses. Wraps the existing bash-script logic in a Tauri application with a system tray icon and a compact status window.

Primary users: non-technical friends and family. Distribution via direct download from Synology NAS Drive (no app store, no package manager).

---

## Architecture

### Stack
- **Framework:** Tauri 2.x (Rust backend + HTML/CSS/JS frontend)
- **Backend language:** Rust
- **Frontend:** Single-page HTML/CSS/JS (no framework — vanilla JS is sufficient)
- **Config storage:** JSON file at `{app_data_dir}/rubypass/config.json`
- **Subnet list:** `{app_data_dir}/rubypass/ru_subnets.txt` (~11k CIDR lines)

### Component split

```
rubypass/
├── src-tauri/          # Rust backend
│   ├── src/
│   │   ├── main.rs         # Tauri setup, tray icon, window
│   │   ├── routing.rs      # Cross-platform route add/delete/detect
│   │   ├── updater.rs      # RIPE NCC download + CIDR parsing
│   │   ├── scheduler.rs    # Auto-update schedule (daily/weekly/monthly)
│   │   ├── config.rs       # Read/write config.json
│   │   └── status.rs       # Gateway detection, route count, VPN detection
│   └── tauri.conf.json
├── src/                # HTML/CSS/JS frontend
│   ├── index.html
│   ├── style.css
│   └── main.js
└── ru_subnets.txt      # Bundled fallback list (can update at runtime)
```

### Cross-platform routing commands

| Platform | Add route | Delete route | Get gateway | Detect VPN |
|---|---|---|---|---|
| macOS | `sudo route add -net <cidr> <gw>` | `sudo route delete -net <cidr> <gw>` | `ipconfig getoption en0 router` | `utun*` interface in `netstat -rn` |
| Windows | `route ADD <net> MASK <mask> <gw>` | `route DELETE <net>` | `ipconfig` parse | TAP/TUN adapter in `ipconfig` |
| Linux | `sudo ip route add <cidr> via <gw>` | `sudo ip route del <cidr>` | `ip route` default | `tun*`/`ppp*` interface in `ip link` |

Routes are added in parallel (up to 50 concurrent system calls) to keep startup under ~5 seconds.

---

## UI

### Tray icon
Three states with distinct icons (bundled as PNG assets):
- **Green** 🟢 — bypass active
- **Red** 🔴 — bypass inactive
- **Yellow** 🟡 — operation in progress (adding/removing routes)

Right-click context menu:
- Status line (e.g. "● Активен")
- "Открыть" — shows/focuses the main window
- Separator
- "Выключить" / "Включить" — toggle bypass
- "Выйти"

### Main window (320×460px, resizable off)
Always-on-top: no. Closes to tray on X button.

Sections (top to bottom):

1. **Header** — app name + window controls (macOS traffic lights / Windows buttons via Tauri decorations)
2. **Status + toggle** — large status label ("Активен" / "Выключен") + color, toggle switch, subtitle
3. **Stats grid** (2×2):
   - Подсети (total in file)
   - Маршруты (active routes through physical interface)
   - Шлюз (detected gateway IP, tooltip: "Адрес вашего роутера")
   - VPN (detected interface name, e.g. `utun2`, or "не обнаружен")
4. **Список IP section**:
   - Last updated date
   - "Обновить" button (manual trigger)
   - Auto-update schedule dropdown: Никогда / Ежедневно / Еженедельно / Ежемесячно
5. **Settings row**:
   - "Автозапуск" toggle

### Theme
Dark theme only (matches mockup): background `#12141f`, surface `#1a1d2e`, accent green `#22c55e`, accent blue `#3b82f6`, warning amber `#f59e0b`, error red `#ef4444`.

---

## Behaviour

### First launch
1. Show main window (not hidden to tray) so user sees the app.
2. If `ru_subnets.txt` missing → automatically trigger update.
3. After update completes → automatically enable bypass.
4. On subsequent launches: start hidden in tray, restore previous bypass state.

### Toggle (enable bypass)
1. Icon → yellow (loading)
2. Detect gateway; if not found → show error notification, abort
3. Add ~11k routes in parallel
4. Icon → green; update stats

### Toggle (disable bypass)
1. Icon → yellow
2. Delete routes in parallel
3. Icon → red; update stats

### Update IP list
1. Show spinner on "Обновить" button
2. Download `https://ftp.ripe.net/pub/stats/ripencc/delegated-ripencc-extended-latest`
3. Parse: filter `RU|ipv4`, convert address count → CIDR prefix length
4. Save to `ru_subnets.txt`; update "last updated" timestamp in config
5. If bypass was active: restart routes (stop → start) to apply new list

### Auto-update schedule
Stored in config as `update_schedule: "never" | "daily" | "weekly" | "monthly"`. Checked on app start and by an internal timer. If schedule elapsed since last update → trigger update silently in background.

### Network change detection
When the system switches networks (home → café, WiFi → Ethernet), routes added with the old gateway become invalid. The app subscribes to OS network change events:
- macOS: `SCDynamicStore` / `Network.framework` path monitor
- Windows: `NotifyIpInterfaceChange` WinAPI
- Linux: `rtnetlink` socket

On network change event: if bypass is active → automatically run stop + start with the newly detected gateway. Show a brief toast: "Сеть изменилась, маршруты обновлены."

### Auto-start
- macOS: `launchctl` plist in `~/Library/LaunchAgents/`
- Windows: registry key `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`
- Linux: `.desktop` file in `~/.config/autostart/`

Tauri plugin `tauri-plugin-autostart` handles all three.

---

## Distribution & Build

Build via GitHub Actions (or local `cargo tauri build`):
- macOS → `.dmg` (universal binary: x86_64 + arm64)
- Windows → `.exe` (NSIS installer, no admin required for tray-only apps; admin required at first `route` call — UAC prompt)
- Linux → `.AppImage`

Output archives uploaded to Synology NAS Drive. No auto-update mechanism (manual download of new version).

**Note on sudo/admin:**
- macOS: `route` requires root. Strategy: bundle a small privileged helper (`rubypass-helper`) installed via `SMJobBless` during first launch. macOS shows a one-time system authorization dialog. Subsequent route operations call the helper silently.
- Linux: use `polkit` rule to allow the helper binary without password prompt. Fallback: prompt via `pkexec`.
- Windows: app manifest sets `requestedExecutionLevel: requireAdministrator` → UAC prompt on launch. `route ADD/DELETE` then work without additional elevation.

**Error handling (user-facing):**
- Gateway not found → toast notification: "Не удалось определить шлюз. Проверьте подключение к роутеру."
- Update download fails → toast: "Не удалось обновить список IP. Проверьте интернет-соединение."
- Route add partially fails → warn in stats (show mismatch between Подсети and Маршруты counts)
- VPN not detected when enabling → warn: "VPN не обнаружен. Убедитесь, что VPN включён."

---

## Config schema

```json
{
  "bypass_enabled": true,
  "autostart": true,
  "update_schedule": "weekly",
  "last_updated": "2026-04-20T10:00:00Z",
  "gateway_override": null
}
```

`gateway_override`: optional manual gateway IP for edge cases where auto-detection fails.

---

## Out of scope
- Auto-update of the app itself (no Tauri updater)
- IPv6 support
- Per-app routing (routes apply system-wide)
- VPN kill-switch
- Multiple gateway/interface profiles
