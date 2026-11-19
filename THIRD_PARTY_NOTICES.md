# Third-party notices

The Windows release bundles these programs next to `Wraith.exe`:

- **Xray-core** v26.9.30 — Mozilla Public License 2.0. Source: https://github.com/XTLS/Xray-core/tree/v26.9.30
- **Wintun** 0.14.1 — prebuilt binaries license by WireGuard LLC (`LICENSE-Wintun.txt` in the release archive). https://www.wintun.net
- **geoip.dat / geosite.dat** — from the v2fly / Loyalsoldier projects.

Rust dependencies (egui/eframe, serde, ureq, rustls, tray-icon, windows-sys and others) are listed with their licenses in `Cargo.lock` and on crates.io.

## flag-icons

Server flags come from flag-icons v7.6.0 by Panayiotis Lipiridis, MIT License ([assets/LICENSE-flag-icons.txt](assets/LICENSE-flag-icons.txt)). The SVGs are packed into `assets/flags.png`; Nordic-cross flags are cropped from the 4x3 set so the cross stays left of centre. Upstream: https://github.com/lipis/flag-icons.
