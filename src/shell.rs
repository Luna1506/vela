//! Settings for the Quickshell control center (`shell/`), which reads them
//! from `vela shell-config` instead of parsing the TOML itself: sanitized
//! values plus a ready-made colour palette derived from the vela theme.

use crate::config::{Config, MediaPosition};
use crate::theme::{self, Rgb, hex, mix, on_color, palette, parse_hex};
use serde_json::json;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Colour tokens the shell expects (all `#rrggbb`); surfaces and state
/// layers are derived in `shell/Theme.qml` from `tint` and the opacities.
pub const SHELL_COLOR_KEYS: [&str; 11] = [
    "background",
    "text",
    "textMuted",
    "textDisabled",
    "tint",
    "primary",
    "textOnPrimary",
    "primaryMuted",
    "error",
    "textOnError",
    "errorSurface",
];

/// Family from a GTK font name like `'Noto Sans Bold 10'` (gsettings form).
pub fn font_family(name: &str) -> Option<String> {
    const STYLES: [&str; 12] = [
        "Bold",
        "Italic",
        "Oblique",
        "Regular",
        "Medium",
        "Light",
        "Thin",
        "Heavy",
        "Black",
        "Semi-Bold",
        "Book",
        "Condensed",
    ];
    let mut words: Vec<&str> = name.trim().trim_matches('\'').split_whitespace().collect();
    while words
        .last()
        .is_some_and(|w| w.parse::<f64>().is_ok() || STYLES.iter().any(|s| s.eq_ignore_ascii_case(w)))
    {
        words.pop();
    }
    (!words.is_empty()).then(|| words.join(" "))
}

/// The font GTK (and so the launcher and settings) uses; asked once.
fn system_font() -> String {
    static FONT: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    FONT.get_or_init(|| {
        std::process::Command::new("gsettings")
            .args(["get", "org.gnome.desktop.interface", "font-name"])
            .output()
            .ok()
            .and_then(|o| font_family(&String::from_utf8_lossy(&o.stdout)))
            .unwrap_or_else(|| "Adwaita Sans".into())
    })
    .clone()
}

/// One JSON line with everything the control center needs.
pub fn shell_json(cfg: &Config) -> String {
    shell_json_for(cfg, crate::components::installed())
}

/// `shell_json` for a given selection: tiles of what isn't installed stay off.
pub fn shell_json_for(cfg: &Config, installed: &crate::components::Installed) -> String {
    use crate::components::Component;
    let a = &cfg.appearance;
    let p = palette(a.theme);
    let accent: Rgb = parse_hex(&a.accent).unwrap_or((0x7a, 0xa2, 0xf7));
    let (error, on_error, error_surface): (Rgb, Rgb, Rgb) = if theme::is_light(a.theme) {
        ((0xb3, 0x26, 0x1e), (0xff, 0xff, 0xff), (0xf9, 0xde, 0xdc))
    } else {
        ((0xf2, 0xb8, 0xb5), (0x60, 0x14, 0x10), mix(p.bg, (0xf2, 0xb8, 0xb5), 0.14))
    };
    let colors = json!({
        "background": hex(p.bg),
        "text": hex(p.fg),
        "textMuted": hex(p.dim),
        "textDisabled": hex(mix(p.bg, p.dim, 0.55)),
        "tint": hex(p.tint),
        "primary": hex(accent),
        "textOnPrimary": hex(on_color(accent)),
        "primaryMuted": hex(mix(p.bg, accent, 0.35)),
        "error": hex(error),
        "textOnError": hex(on_error),
        "errorSurface": hex(error_surface),
    });
    let animation_scale = if a.animations { 1.0 / a.animation_speed } else { 0.0 };
    let pn = &cfg.panel;
    let backdrop = pn.backdrop && theme::backdrop_visible(a.backdrop_dim, pn.backdrop_blur);
    let clock_on_backdrop = pn.clock_on_backdrop && backdrop && pn.backdrop_blur;
    let media_position = if clock_on_backdrop { pn.media_player_position } else { MediaPosition::Tiles };
    json!({
        "idle": {
            "suspend": cfg.idle.suspend,
            "suspendAfterMin": cfg.idle.suspend_after_min,
        },
        "colors": colors,
        "appearance": {
            "light": theme::is_light(a.theme),
            "radius": a.border_radius,
            "fontScale": a.font_scale,
            "opacity": cfg.general.opacity,
            "surfaceOpacity": a.surface_opacity,
            "animationScale": animation_scale,
            "backdropDim": a.backdrop_dim,
            "fontFamily": system_font(),
        },
        "panel": {
            "width": pn.width,
            "closeOnFocusLoss": pn.close_on_focus_loss,
            "backdrop": backdrop,
            "backdropBlur": pn.backdrop_blur,
            "backdropLayers": theme::backdrop_layers(pn.backdrop_strength, pn.backdrop_blur),
            "backdropLayerAlpha": theme::backdrop_alpha(a.backdrop_dim, pn.backdrop_strength, pn.backdrop_blur),
            "popupTimeoutMs": pn.popup_timeout_secs * 1000,
            "popupMaxVisible": pn.popup_max_visible,
            "criticalPopupsStay": pn.critical_popups_stay,
            "groupCollapsedCount": pn.group_collapsed_count,
            "compactNotifications": pn.compact_notifications,
            "workspaceOsd": pn.workspace_osd,
            "nightLightTemperature": pn.night_light_temperature,
            "clockCentered": pn.clock_centered,
            "clockOnBackdrop": clock_on_backdrop,
            "mediaPlayer": pn.media_player,
            "mediaPlayerAny": pn.media_player_any,
            "mediaPlayerPosition": media_position.id(),
            "mediaPlayerCover": pn.media_player_cover,
            "mediaPlayerCoverBackground": pn.media_player_cover_background,
            "mediaPlayerProgress": pn.media_player_progress,
            "mediaPlayerShuffleRepeat": pn.media_player_shuffle_repeat,
            "mediaPlayerScale": pn.media_player_scale,
            "mediaPlayerCoverBlur": pn.media_player_cover_blur,
            "mediaPlayerButtonScale": pn.media_player_button_scale,
            "mediaPlayerButtonBackground": pn.media_player_button_background,
            "backdropClockSize": pn.backdrop_clock_size,
            "clockFont": Some(pn.clock_font.trim()).filter(|f| !f.is_empty()).map_or_else(system_font, str::to_string),
            "claudeUsage": pn.claude_usage && installed.has(Component::Claude),
            "claudeUsageSubtle": pn.claude_usage_subtle,
            "claudeUsageOnlyDefault": pn.claude_usage_only_default,
            "claudeUsageHidden": pn.claude_usage_hidden,
            "updatesTile": pn.updates_tile && installed.has(Component::Updates),
            "quickshareTile": pn.quickshare_tile && installed.has(Component::Quickshare),
        },
        "quickshare": {
            "available": installed.has(Component::Quickshare),
            "name": cfg.quickshare.effective_name(),
        },
        "pulse": {
            "smoothGraphs": cfg.pulse.smooth_graphs,
            "startPage": cfg.pulse.start_page,
            "rangeSecs": cfg.pulse.range_secs,
            "confirmEnd": cfg.pulse.confirm_end,
            "heatMap": cfg.pulse.heat_map,
            "showKernel": cfg.pulse.show_kernel,
            "sensorView": cfg.pulse.sensor_view,
            "cpuTemp": cfg.pulse.cpu_temp,
            "throttleTint": cfg.pulse.throttle_tint,
            "keys": {
                "force": cfg.pulse.key_force,
                "end": cfg.pulse.key_end,
                "restart": cfg.pulse.key_restart,
                "pause": cfg.pulse.key_pause,
                "efficiency": cfg.pulse.key_efficiency,
            },
            "intervalMs": cfg.pulse.interval_ms,
            "record": cfg.pulse.record,
            "claude": installed.has(Component::Claude),
        },
    })
    .to_string()
}

/// Re-reads the config when the file changes and yields the new JSON line
/// if it differs from the last one. Polling (mtime + size) is enough here
/// and survives the atomic rename of saves, which breaks inotify watches.
pub struct ConfigWatcher {
    path: PathBuf,
    stamp: Option<(Option<SystemTime>, u64)>,
    last: Option<String>,
    error: Option<String>,
}

impl ConfigWatcher {
    pub fn new(path: PathBuf) -> ConfigWatcher {
        ConfigWatcher {
            path,
            stamp: None,
            last: None,
            error: None,
        }
    }

    pub fn poll(&mut self) -> Option<String> {
        let meta = std::fs::metadata(&self.path).ok();
        let stamp = (meta.as_ref().and_then(|m| m.modified().ok()), meta.as_ref().map_or(0, |m| m.len()));
        if self.stamp == Some(stamp) {
            return None;
        }
        self.stamp = Some(stamp);
        let cfg = match std::fs::read_to_string(&self.path) {
            Ok(text) => match Config::from_toml(&text) {
                Ok(cfg) => cfg,
                Err(e) => {
                    self.error = Some(format!("invalid config {}: {e:#}", self.path.display()));
                    return None;
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Config::default(),
            Err(e) => {
                self.error = Some(format!("cannot read {}: {e}", self.path.display()));
                return None;
            }
        };
        let line = shell_json(&cfg);
        if self.last.as_ref() == Some(&line) {
            return None;
        }
        self.last = Some(line.clone());
        Some(line)
    }

    pub fn take_error(&mut self) -> Option<String> {
        self.error.take()
    }
}

/// Directory with `shell.qml`: `$VELA_SHELL_DIR`, else the first candidate.
pub fn find_shell_dir(override_dir: Option<&OsStr>, candidates: &[PathBuf]) -> Option<PathBuf> {
    let has_shell = |d: &Path| d.join("shell.qml").is_file();
    if let Some(d) = override_dir.filter(|d| !d.is_empty()).map(PathBuf::from) {
        return has_shell(&d).then_some(d);
    }
    candidates.iter().find(|d| has_shell(d)).cloned()
}

/// Where `vela shell` looks for the QML: next to a source checkout (for
/// `target/release/vela`), the user install and the system package.
pub fn shell_dir_candidates(exe: Option<&Path>) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(bin) = exe.and_then(Path::parent) {
        dirs.push(bin.join("../../shell"));
        dirs.push(bin.join("../share/vela/shell"));
    }
    dirs.push(crate::paths::home_dir().join(".local/share/vela/shell"));
    dirs.push(PathBuf::from("/usr/share/vela/shell"));
    dirs
}

#[cfg(test)]
mod tests {

    #[test]
    fn font_family_drops_size_and_style() {
        assert_eq!(font_family("'Noto Sans  10'").as_deref(), Some("Noto Sans"));
        assert_eq!(font_family("'Adwaita Sans 11'").as_deref(), Some("Adwaita Sans"));
        assert_eq!(font_family("'Cantarell Bold Italic 11.5'").as_deref(), Some("Cantarell"));
        assert_eq!(font_family("''"), None);
    }

    use super::*;
    use crate::config::{Config, Theme};
    use serde_json::Value;

    fn json(cfg: &Config) -> Value {
        serde_json::from_str(&shell_json(cfg)).unwrap()
    }

    fn is_hex(v: &Value) -> bool {
        let s = v.as_str().unwrap_or("");
        s.len() == 7 && s.starts_with('#') && s[1..].chars().all(|c| c.is_ascii_hexdigit())
    }

    #[test]
    fn is_a_single_line() {
        assert!(!shell_json(&Config::default()).contains('\n'), "one line per update for the SplitParser");
    }

    #[test]
    fn tiles_of_missing_components_stay_hidden() {
        use crate::components::{Component, Installed};
        let j: Value = serde_json::from_str(&shell_json_for(&Config::default(), &Installed::only(&[Component::Panel]))).unwrap();
        assert_eq!(j["panel"]["claudeUsage"], false);
        assert_eq!(j["panel"]["updatesTile"], false);
        let j: Value = serde_json::from_str(&shell_json_for(&Config::default(), &Installed::all())).unwrap();
        assert_eq!(j["panel"]["claudeUsage"], true);
        assert_eq!(j["panel"]["updatesTile"], true);
        assert_eq!(j["pulse"]["smoothGraphs"], true);
        assert_eq!(j["pulse"]["startPage"], "overview");
        assert_eq!(j["pulse"]["rangeSecs"], 60);
    }

    #[test]
    fn quickshare_tile_needs_component() {
        use crate::components::{Component, Installed};
        let mut cfg = Config::default();
        cfg.quickshare.name = "Desk".into();
        let j: Value = serde_json::from_str(&shell_json_for(&cfg, &Installed::only(&[Component::Panel]))).unwrap();
        assert_eq!(j["panel"]["quickshareTile"], false);
        assert_eq!(j["quickshare"]["available"], false);
        let j: Value = serde_json::from_str(&shell_json_for(&cfg, &Installed::only(&[Component::Panel, Component::Quickshare]))).unwrap();
        assert_eq!(j["panel"]["quickshareTile"], true);
        assert_eq!(j["quickshare"]["available"], true);
        assert_eq!(j["quickshare"]["name"], "Desk");
        cfg.panel.quickshare_tile = false;
        let j: Value = serde_json::from_str(&shell_json_for(&cfg, &Installed::all())).unwrap();
        assert_eq!(j["panel"]["quickshareTile"], false);
        assert_eq!(j["quickshare"]["available"], true);
    }

    #[test]
    fn carries_shared_appearance_and_panel_settings() {
        let mut cfg = Config::default();
        cfg.appearance.accent = "#ff0000".into();
        cfg.appearance.border_radius = 30;
        cfg.appearance.font_scale = 1.25;
        cfg.appearance.backdrop_dim = 0.3;
        cfg.general.opacity = 0.7;
        cfg.panel.width = 500;
        cfg.panel.backdrop = true;
        cfg.panel.clock_centered = true;
        let j = json(&cfg);
        assert_eq!(j["colors"]["primary"], "#ff0000");
        assert_eq!(j["appearance"]["radius"], 30);
        assert_eq!(j["appearance"]["fontScale"], 1.25);
        assert_eq!(j["appearance"]["opacity"], 0.7);
        assert_eq!(j["appearance"]["backdropDim"], 0.3);
        assert_eq!(j["panel"]["width"], 500);
        assert_eq!(j["panel"]["backdrop"], true);
        assert_eq!(j["panel"]["popupTimeoutMs"], 5000);
        assert_eq!(j["panel"]["clockCentered"], true);
        assert_eq!(j["panel"]["claudeUsage"], true);
        assert_eq!(j["panel"]["claudeUsageSubtle"], false);
        assert_eq!(j["panel"]["compactNotifications"], true);
        assert_eq!(j["panel"]["claudeUsageOnlyDefault"], false);
        assert_eq!(j["panel"]["claudeUsageHidden"], serde_json::json!([]));
        assert_eq!(j["panel"]["updatesTile"], true);
        assert_eq!(j["idle"]["suspend"], true);
        assert_eq!(j["idle"]["suspendAfterMin"], 30.0);
        assert_eq!(j["panel"]["backdropLayers"], 1);
        cfg.panel.backdrop_strength = 3;
        let j = json(&cfg);
        assert_eq!(j["panel"]["backdropLayers"], 3);
        assert_eq!(j["panel"]["backdropLayerAlpha"], crate::theme::backdrop_layer_alpha(0.3, 3));
        assert_eq!(j["panel"]["backdropBlur"], true);
        cfg.appearance.backdrop_blur = false;
        assert_eq!(json(&cfg)["panel"]["backdropBlur"], true, "the launcher's switch");
        cfg.panel.backdrop_blur = false;
        let j = json(&cfg);
        assert_eq!(
            (j["panel"]["backdropBlur"].clone(), j["panel"]["backdropLayers"].clone()),
            (false.into(), 1.into())
        );
        assert_eq!(j["panel"]["backdropLayerAlpha"], 0.3);
        cfg.appearance.backdrop_dim = 0.0;
        assert_eq!(json(&cfg)["panel"]["backdrop"], false, "nothing to draw");
    }

    #[test]
    fn clock_on_backdrop_needs_a_blurred_backdrop() {
        let mut cfg = Config::default();
        cfg.panel.clock_on_backdrop = true;
        assert_eq!(json(&cfg)["panel"]["clockOnBackdrop"], false, "no backdrop");
        cfg.panel.backdrop = true;
        assert_eq!(json(&cfg)["panel"]["clockOnBackdrop"], true);
        cfg.panel.backdrop_blur = false;
        assert_eq!(json(&cfg)["panel"]["clockOnBackdrop"], false, "only dimmed");
        cfg.panel.backdrop_blur = true;
        cfg.panel.clock_on_backdrop = false;
        assert_eq!(json(&cfg)["panel"]["clockOnBackdrop"], false);
    }

    #[test]
    fn media_player_moves_only_with_the_clock_on_the_blur() {
        let mut cfg = Config::default();
        let j = json(&cfg);
        assert_eq!(j["panel"]["mediaPlayer"], true);
        assert_eq!(j["panel"]["mediaPlayerAny"], false);
        assert_eq!(j["panel"]["mediaPlayerPosition"], "tiles");
        assert_eq!(
            (
                j["panel"]["mediaPlayerCover"].clone(),
                j["panel"]["mediaPlayerCoverBackground"].clone(),
                j["panel"]["mediaPlayerProgress"].clone()
            ),
            (true.into(), false.into(), true.into())
        );
        assert_eq!(j["panel"]["mediaPlayerShuffleRepeat"], true);
        assert_eq!(
            (j["panel"]["mediaPlayerScale"].clone(), j["panel"]["mediaPlayerCoverBlur"].clone()),
            (1.0.into(), 0.75.into())
        );
        assert_eq!(
            (j["panel"]["mediaPlayerButtonScale"].clone(), j["panel"]["mediaPlayerButtonBackground"].clone()),
            (1.0.into(), true.into())
        );
        cfg.panel.media_player_position = MediaPosition::Top;
        assert_eq!(json(&cfg)["panel"]["mediaPlayerPosition"], "tiles", "the clock still sits at the top");
        cfg.panel.backdrop = true;
        cfg.panel.clock_on_backdrop = true;
        assert_eq!(json(&cfg)["panel"]["mediaPlayerPosition"], "top");
        cfg.panel.media_player_position = MediaPosition::Bottom;
        assert_eq!(json(&cfg)["panel"]["mediaPlayerPosition"], "bottom");
        cfg.panel.backdrop_blur = false;
        assert_eq!(json(&cfg)["panel"]["mediaPlayerPosition"], "tiles");
    }

    #[test]
    fn clock_font_falls_back_to_the_ui_font() {
        let mut cfg = Config::default();
        let j = json(&cfg);
        assert_eq!(j["panel"]["clockFont"], j["appearance"]["fontFamily"]);
        assert_eq!(j["panel"]["backdropClockSize"], 128);
        cfg.panel.clock_font = "  JetBrains Mono ".into();
        assert_eq!(json(&cfg)["panel"]["clockFont"], "JetBrains Mono");
    }

    #[test]
    fn animation_scale_follows_speed_and_switch() {
        let mut cfg = Config::default();
        cfg.appearance.animation_speed = 2.0;
        assert_eq!(json(&cfg)["appearance"]["animationScale"], 0.5);
        cfg.appearance.animations = false;
        assert_eq!(json(&cfg)["appearance"]["animationScale"], 0.0);
    }

    #[test]
    fn every_theme_yields_a_full_palette() {
        for t in Theme::ALL {
            let mut cfg = Config::default();
            cfg.appearance.theme = t;
            let j = json(&cfg);
            for key in SHELL_COLOR_KEYS {
                assert!(is_hex(&j["colors"][key]), "{t:?}: {key} = {}", j["colors"][key]);
            }
        }
    }

    #[test]
    fn light_theme_has_dark_text_and_readable_accent_text() {
        let mut cfg = Config::default();
        cfg.appearance.theme = Theme::Light;
        let j = json(&cfg);
        assert_eq!(j["colors"]["text"], "#1c1c22");
        assert_eq!(j["colors"]["tint"], "#000000");
        cfg.appearance.accent = "#ffff00".into();
        assert_eq!(json(&cfg)["colors"]["textOnPrimary"], "#101014", "dark text on a bright accent");
        cfg.appearance.accent = "#202080".into();
        assert_eq!(json(&cfg)["colors"]["textOnPrimary"], "#ffffff", "light text on a dark accent");
    }
    #[test]
    fn watcher_emits_on_start_and_on_real_changes_only() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let mut w = ConfigWatcher::new(path.clone());
        let first = w.poll().expect("defaults when the file doesn't exist yet");
        assert_eq!(json_of(&first)["panel"]["width"], 420);
        assert!(w.poll().is_none(), "nothing changed");

        std::fs::write(&path, "[panel]\nwidth = 500\n").unwrap();
        assert_eq!(json_of(&w.poll().unwrap())["panel"]["width"], 500);
        // Rewritten with the same content (e.g. atomic save): no new line.
        std::fs::write(&path, "[panel]\nwidth = 500\n# comment\n").unwrap();
        assert!(w.poll().is_none());

        // A broken file keeps the last good values and reports once.
        std::fs::write(&path, "[panel\nwidth = ").unwrap();
        assert!(w.poll().is_none());
        assert!(w.take_error().is_some());
        std::fs::write(&path, "[panel]\nwidth = 600\n").unwrap();
        assert_eq!(json_of(&w.poll().unwrap())["panel"]["width"], 600);
    }

    #[test]
    fn shell_dir_prefers_the_override() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("shell.qml"), "").unwrap();
        assert_eq!(find_shell_dir(Some(dir.path().as_os_str()), &[]), Some(dir.path().to_path_buf()));
        let empty = tempfile::tempdir().unwrap();
        assert_eq!(
            find_shell_dir(None, &[empty.path().to_path_buf(), dir.path().to_path_buf()]),
            Some(dir.path().to_path_buf())
        );
        assert_eq!(find_shell_dir(None, &[empty.path().to_path_buf()]), None);
    }

    fn json_of(s: &str) -> Value {
        serde_json::from_str(s).unwrap()
    }
}
