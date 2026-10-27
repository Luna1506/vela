use super::binder::{Binder, group, join_args, split_args};
use crate::config::{Config, FileBackend, HorizontalPosition, TERMINAL_PRESETS, Theme};
use crate::launch;
use crate::paths;
use crate::search::files::RootBackend;
use crate::ui::daemon::Daemon;
use adw::prelude::*;
use gtk::{gdk, gio, glib};
use std::rc::Rc;

fn page() -> adw::PreferencesPage {
    adw::PreferencesPage::new()
}

// ------------------------------------------------------------------ General

/// Choices for the main monitor: follow focus, every connected monitor, and
/// the configured one if it is unplugged right now.
fn main_monitor_choices(current: &str) -> Vec<(String, String)> {
    let mut choices = vec![(String::new(), "Focused monitor".to_owned())];
    for m in crate::hyprland::monitors() {
        let label = if m.description.is_empty() {
            m.name.clone()
        } else {
            format!("{} · {}", m.name, m.description)
        };
        choices.push((crate::hyprland::monitor_spec(&m), label));
    }
    let current = current.trim();
    if !current.is_empty() && !choices.iter().any(|(spec, _)| spec == current) {
        let name = current.strip_prefix("desc:").unwrap_or(current);
        choices.push((current.to_owned(), format!("Not connected · {name}")));
    }
    choices
}

fn main_monitor_row(b: &Binder) -> adw::ComboRow {
    let row = adw::ComboRow::builder()
        .title("Main monitor")
        .subtitle("The launcher opens here whenever this monitor is connected")
        .build();
    let choices: Rc<std::cell::RefCell<Vec<(String, String)>>> = Rc::default();
    let syncing = Rc::new(std::cell::Cell::new(false));
    // Monitors come and go (dock), so the list is rebuilt whenever it is shown.
    let sync = {
        let (row, choices, syncing) = (row.downgrade(), choices.clone(), syncing.clone());
        move |current: &str| {
            let Some(row) = row.upgrade() else { return };
            let list = main_monitor_choices(current);
            let labels: Vec<&str> = list.iter().map(|(_, l)| l.as_str()).collect();
            syncing.set(true);
            row.set_model(Some(&gtk::StringList::new(&labels)));
            row.set_selected(list.iter().position(|(spec, _)| spec == current.trim()).unwrap_or(0) as u32);
            syncing.set(false);
            *choices.borrow_mut() = list;
        }
    };
    let sync = Rc::new(sync);
    sync(&b.store.get().general.main_monitor);
    {
        let (choices, syncing, b) = (choices.clone(), syncing.clone(), b.clone());
        row.connect_selected_notify(move |r| {
            if syncing.get() {
                return;
            }
            if let Some((spec, _)) = choices.borrow().get(r.selected() as usize) {
                let spec = spec.clone();
                b.write(|c| c.general.main_monitor = spec);
            }
        });
    }
    {
        let (sync, b) = (sync.clone(), b.clone());
        row.connect_map(move |_| sync(&b.store.get().general.main_monitor));
    }
    b.on_refresh(move |c| sync(&c.general.main_monitor));
    row
}

pub fn launcher(b: &Binder) -> adw::PreferencesPage {
    let p = page();

    let launcher = group("Window", "Where the launcher opens and how big it gets.");
    launcher.add(&b.spin(
        "Width",
        "Logical pixels",
        360.0,
        2400.0,
        10.0,
        0,
        |c| c.general.width.into(),
        |c, v| c.general.width = v as u32,
    ));
    launcher.add(&b.spin(
        "Maximum height",
        "Content scrolls beyond this height",
        200.0,
        2000.0,
        10.0,
        0,
        |c| c.general.max_height.into(),
        |c, v| c.general.max_height = v as u32,
    ));
    launcher.add(&b.spin(
        "Vertical position",
        "Distance from the top of the screen in percent",
        0.0,
        60.0,
        1.0,
        0,
        |c| c.general.vertical_position.into(),
        |c, v| c.general.vertical_position = v as u32,
    ));
    let align = b.combo(
        "Horizontal position",
        "",
        &["Centred", "Left", "Right"],
        |c| HorizontalPosition::ALL.iter().position(|&h| h == c.general.horizontal_position).unwrap_or(0),
        |c, i| c.general.horizontal_position = HorizontalPosition::ALL[i.min(2)],
    );
    launcher.add(&align);
    let side = b.spin(
        "Distance from the edge",
        "Logical pixels, when on the left or right",
        0.0,
        800.0,
        4.0,
        0,
        |c| c.general.side_margin.into(),
        |c, v| c.general.side_margin = v as u32,
    );
    launcher.add(&side);
    launcher.add(&main_monitor_row(b));
    p.add(&launcher);
    p.add(&launcher_clock_group(b));
    {
        let side = side.downgrade();
        let sync = move |c: &Config| {
            if let Some(side) = side.upgrade() {
                side.set_sensitive(c.general.horizontal_position != HorizontalPosition::Center);
            }
        };
        sync(&b.store.get());
        b.store.subscribe(move |_, c| sync(c));
    }

    let behaviour = group("Behaviour", "");
    behaviour.add(&b.switch(
        "Close when clicking elsewhere",
        "Clicking elsewhere hides the launcher",
        |c| c.general.close_on_focus_loss,
        |c, v| c.general.close_on_focus_loss = v,
    ));
    behaviour.add(&b.spin(
        "Maximum results",
        "Rows in the result list",
        1.0,
        100.0,
        1.0,
        0,
        |c| c.general.max_results.into(),
        |c, v| c.general.max_results = v as u32,
    ));
    p.add(&behaviour);

    p.add(&grid_group(b));
    p
}

// ------------------------------------------------------------------- System

pub fn system(daemon: &Rc<Daemon>, b: &Binder) -> adw::PreferencesPage {
    let p = page();
    p.add(&super::vela_group::updates_group(daemon, b));
    p.add(&super::vela_group::components_group(daemon));

    let term = group("Terminal", "Used for Claude and for applications that need a terminal.");
    let mut names: Vec<&str> = TERMINAL_PRESETS.iter().map(|(n, _)| *n).collect();
    names.push("Custom");
    let custom_idx = names.len() - 1;
    let preset_idx = |c: &Config| {
        TERMINAL_PRESETS
            .iter()
            .position(|(n, _)| *n == c.terminal.executable.trim())
            .unwrap_or(TERMINAL_PRESETS.len())
    };
    let preset = b.combo("Terminal", "", &names, preset_idx, |c, i| {
        if let Some((name, _)) = TERMINAL_PRESETS.get(i) {
            c.terminal.executable = (*name).into();
            c.terminal.exec_args = None;
        }
    });
    term.add(&preset);
    let exe = b.entry("Terminal executable", |c| c.terminal.executable.clone(), |c, v| c.terminal.executable = v);
    add_exec_status(&exe, b, |c| c.terminal.executable.clone());
    term.add(&exe);
    let default_args = b.switch(
        "Default arguments",
        "Use the known arguments of this terminal to run a command",
        |c| c.terminal.exec_args.is_none(),
        |c, v| {
            c.terminal.exec_args = if v { None } else { Some(c.terminal.effective_exec_args()) };
        },
    );
    term.add(&default_args);
    let args = adw::EntryRow::builder().title("Arguments before the command").build();
    let set_args_row = {
        let args = args.clone();
        move |c: &Config| {
            let text = join_args(&c.terminal.effective_exec_args());
            if args.text() != text {
                args.set_text(&text);
            }
            args.set_sensitive(c.terminal.exec_args.is_some());
        }
    };
    set_args_row(&b.store.get());
    {
        let b2 = b.clone();
        args.connect_changed(move |r| {
            if let Some(v) = split_args(&r.text()) {
                b2.write(|c| c.terminal.exec_args = Some(v));
            }
        });
        let s = set_args_row.clone();
        b.store.subscribe(move |old, new| {
            if old.terminal != new.terminal {
                s(new);
            }
        });
    }
    preset.connect_selected_notify(move |r| {
        if r.selected() as usize == custom_idx {
            exe.grab_focus();
        }
    });
    term.add(&args);
    p.add(&term);

    let apps = group("Applications", "");
    apps.add(&b.switch(
        "Separate systemd scope per application",
        "Launched apps survive restarts of the vela service (requires systemd-run)",
        |c| c.general.systemd_scope,
        |c, v| c.general.systemd_scope = v,
    ));
    p.add(&apps);

    let file = group(
        "Configuration",
        "Settings are saved automatically. The file can also be edited by hand; changes are picked up live.",
    );
    let path = daemon.store.path();
    let row = adw::ActionRow::builder()
        .title("Configuration file")
        .subtitle(paths::display_path(&path))
        .subtitle_selectable(true)
        .build();
    let open = gtk::Button::builder()
        .icon_name("document-open-symbolic")
        .tooltip_text("Open in the default editor")
        .valign(gtk::Align::Center)
        .css_classes(["flat"])
        .build();
    open.connect_clicked(move |_| {
        if let Ok(spec) = launch::open_path_spec(&path) {
            let _ = launch::spawn_detached(&spec, false);
        }
    });
    row.add_suffix(&open);
    file.add(&row);

    let reload_apps = adw::ButtonRow::builder()
        .title("Reload applications")
        .start_icon_name("view-refresh-symbolic")
        .build();
    let d = daemon.clone();
    reload_apps.connect_activated(move |_| d.rescan_apps());
    file.add(&reload_apps);

    let restore = adw::ButtonRow::builder()
        .title("Restore defaults…")
        .start_icon_name("edit-undo-symbolic")
        .css_classes(["destructive-action"])
        .build();
    let store = daemon.store.clone();
    restore.connect_activated(move |r| {
        let dialog = adw::AlertDialog::new(
            Some("Restore default settings?"),
            Some("All settings, pinned applications and custom actions are reset. This cannot be undone."),
        );
        dialog.add_responses(&[("cancel", "Cancel"), ("restore", "Restore defaults")]);
        dialog.set_response_appearance("restore", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("cancel"));
        let store = store.clone();
        dialog.connect_response(None, move |_, resp| {
            if resp == "restore" {
                store.restore_defaults();
            }
        });
        dialog.present(Some(r));
    });
    file.add(&restore);
    p.add(&file);

    let about = group("About", "");
    about.add(
        &adw::ActionRow::builder()
            .title("Vela")
            .subtitle(format!("Version {}", env!("CARGO_PKG_VERSION")))
            .build(),
    );
    if crate::components::has(crate::components::Component::Launcher) {
        about.add(
            &adw::ActionRow::builder()
                .title("Keyboard")
                .subtitle("Tap Super to open · Enter launches · Shift+Enter asks Claude · Ctrl+Enter shows a file in its folder · Esc closes")
                .build(),
        );
    }
    p.add(&about);
    p
}

/// Shows below an executable entry whether the command can be found.
fn add_exec_status(row: &adw::EntryRow, b: &Binder, get: fn(&Config) -> String) {
    let icon = gtk::Image::new();
    icon.set_valign(gtk::Align::Center);
    let update = {
        let icon = icon.clone();
        move |c: &Config| match paths::find_executable(&get(c)) {
            Some(p) => {
                icon.set_icon_name(Some("emblem-ok-symbolic"));
                icon.set_tooltip_text(Some(&format!("Found: {}", p.display())));
                icon.remove_css_class("error");
                icon.add_css_class("success");
            }
            None => {
                icon.set_icon_name(Some("dialog-warning-symbolic"));
                icon.set_tooltip_text(Some("Not found in PATH"));
                icon.remove_css_class("success");
                icon.add_css_class("error");
            }
        }
    };
    update(&b.store.get());
    b.store.subscribe(move |_, new| update(new));
    row.add_suffix(&icon);
}

// --------------------------------------------------------------- Appearance

fn grid_group(b: &Binder) -> adw::PreferencesGroup {
    let grid = group("Application grid", "");
    grid.add(&b.spin(
        "Tile size",
        "",
        56.0,
        240.0,
        2.0,
        0,
        |c| c.appearance.tile_size.into(),
        |c, v| c.appearance.tile_size = v as u32,
    ));
    grid.add(&b.spin(
        "Icon size",
        "",
        16.0,
        192.0,
        2.0,
        0,
        |c| c.appearance.icon_size.into(),
        |c, v| c.appearance.icon_size = v as u32,
    ));
    grid.add(&b.spin(
        "Spacing",
        "",
        0.0,
        64.0,
        1.0,
        0,
        |c| c.appearance.spacing.into(),
        |c, v| c.appearance.spacing = v as u32,
    ));
    grid.add(&b.spin(
        "Columns",
        "0 = automatic, based on width and tile size",
        0.0,
        16.0,
        1.0,
        0,
        |c| c.appearance.columns.into(),
        |c, v| c.appearance.columns = v as u32,
    ));
    grid.add(&b.switch(
        "Show names",
        "Application names below the icons",
        |c| c.appearance.show_labels,
        |c, v| c.appearance.show_labels = v,
    ));
    grid.add(&b.switch(
        "Tile background",
        "Off: the icons sit directly on the launcher",
        |c| c.appearance.tile_background,
        |c, v| c.appearance.tile_background = v,
    ));
    grid.add(&b.switch(
        "Outline the selected tile",
        "An accent-coloured ring in addition to the highlight",
        |c| c.appearance.tile_outline,
        |c, v| c.appearance.tile_outline = v,
    ));
    grid
}

fn hex_to_rgba(hex: &str) -> gdk::RGBA {
    gdk::RGBA::parse(hex).unwrap_or(gdk::RGBA::new(0.48, 0.64, 0.97, 1.0))
}

fn rgba_to_hex(c: &gdk::RGBA) -> String {
    let ch = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("#{:02x}{:02x}{:02x}", ch(c.red()), ch(c.green()), ch(c.blue()))
}

/// Accent presets offered as colour dots (Tokyo Night-ish, plus neutrals).
pub const ACCENTS: [&str; 8] = ["#7aa2f7", "#bb9af7", "#f7768e", "#ff9e64", "#e0af68", "#9ece6a", "#2ac3de", "#c0caf5"];

/// Themes as clickable swatches instead of a drop-down.
fn theme_picker(b: &Binder) -> gtk::Widget {
    let flow = gtk::FlowBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .homogeneous(true)
        .max_children_per_line(5)
        .min_children_per_line(3)
        .column_spacing(12)
        .row_spacing(12)
        .margin_top(6)
        .margin_bottom(6)
        .build();
    let mut first: Option<gtk::ToggleButton> = None;
    let mut buttons = Vec::new();
    for t in Theme::ALL {
        let swatch = gtk::Box::builder()
            .css_classes(["vela-swatch", &format!("vela-swatch-{}", t.label().to_lowercase())])
            .build();
        swatch.set_size_request(96, 60);
        let label = gtk::Label::new(Some(t.label()));
        let content = gtk::Box::new(gtk::Orientation::Vertical, 8);
        content.append(&swatch);
        content.append(&label);
        let btn = gtk::ToggleButton::builder()
            .child(&content)
            .css_classes(["flat", "vela-theme-chip"])
            .tooltip_text(t.label())
            .build();
        if let Some(f) = &first {
            btn.set_group(Some(f));
        } else {
            first = Some(btn.clone());
        }
        btn.set_active(b.store.get().appearance.theme == t);
        let b2 = b.clone();
        btn.connect_toggled(move |btn| {
            if btn.is_active() {
                b2.write(|c| c.appearance.theme = t);
            }
        });
        flow.insert(&btn, -1);
        buttons.push((t, btn.downgrade()));
    }
    b.on_refresh(move |c| {
        for (t, w) in &buttons {
            if let Some(btn) = w.upgrade() {
                btn.set_active(c.appearance.theme == *t);
            }
        }
    });
    flow.upcast()
}

/// Preset dots plus a custom colour button.
fn accent_row(b: &Binder) -> adw::ActionRow {
    let row = adw::ActionRow::builder()
        .title("Accent colour")
        .subtitle("Selection, sliders and active tiles")
        .build();
    let dots = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    dots.set_valign(gtk::Align::Center);
    let mut first: Option<gtk::ToggleButton> = None;
    let mut all = Vec::new();
    for (i, hex) in ACCENTS.iter().enumerate() {
        let dot = gtk::ToggleButton::builder()
            .css_classes(["vela-accent-dot", &format!("vela-accent-{i}")])
            .tooltip_text(*hex)
            .valign(gtk::Align::Center)
            .build();
        if let Some(f) = &first {
            dot.set_group(Some(f));
        } else {
            first = Some(dot.clone());
        }
        let b2 = b.clone();
        let hex = hex.to_string();
        dot.connect_toggled(move |d| {
            if d.is_active() {
                let hex = hex.clone();
                b2.write(|c| c.appearance.accent = hex);
            }
        });
        dots.append(&dot);
        all.push(dot);
    }
    let custom = gtk::ColorDialogButton::new(Some(gtk::ColorDialog::builder().with_alpha(false).build()));
    custom.set_valign(gtk::Align::Center);
    custom.set_tooltip_text(Some("Custom colour"));
    let b2 = b.clone();
    custom.connect_rgba_notify(move |btn| {
        let hex = rgba_to_hex(&btn.rgba());
        b2.write(|c| c.appearance.accent = hex);
    });
    dots.append(&custom);
    row.add_suffix(&dots);
    let sync = {
        let all: Vec<_> = all.iter().map(|d| d.downgrade()).collect();
        let custom = custom.downgrade();
        move |c: &Config| {
            let accent = c.appearance.accent.to_lowercase();
            for (i, w) in all.iter().enumerate() {
                if let Some(d) = w.upgrade() {
                    d.set_active(ACCENTS[i] == accent);
                }
            }
            if let Some(btn) = custom.upgrade() {
                btn.set_rgba(&hex_to_rgba(&accent));
            }
        }
    };
    sync(&b.store.get());
    b.store.subscribe(move |_, c| sync(c));
    row
}

/// Blur switch and strength of one backdrop; strength only matters with blur.
fn backdrop_rows(
    b: &Binder,
    expander: &adw::ExpanderRow,
    blur: fn(&Config) -> bool,
    set_blur: fn(&mut Config, bool),
    strength: fn(&Config) -> f64,
    set_strength: fn(&mut Config, f64),
) {
    expander.add_row(&b.switch("Blur", "Off = only dim the rest of the screen", blur, set_blur));
    let row = b.spin(
        "Strength",
        "1 = Hyprland's blur; each step blurs once more (and a little darker)",
        1.0,
        4.0,
        1.0,
        0,
        strength,
        set_strength,
    );
    row.set_sensitive(blur(&b.store.get()));
    let w = row.downgrade();
    // subscribe, not on_refresh: that one only runs on reloads, not when the Blur switch flips.
    b.store.subscribe(move |_, c| {
        if let Some(r) = w.upgrade() {
            r.set_sensitive(blur(c));
        }
    });
    expander.add_row(&row);
}

pub fn appearance(b: &Binder) -> adw::PreferencesPage {
    let p = page();
    p.set_description("Shared by the launcher and the control center.");

    let theme = group("Theme", "");
    theme.add(&theme_picker(b));
    p.add(&theme);

    let style = group("Colour, shape and text", "");
    style.add(&accent_row(b));
    style.add(&b.spin(
        "Corner radius",
        "",
        0.0,
        64.0,
        1.0,
        0,
        |c| c.appearance.border_radius.into(),
        |c, v| c.appearance.border_radius = v as u32,
    ));
    style.add(&b.spin(
        "Background opacity",
        "Launcher, panel and popups; 1 = opaque",
        0.0,
        1.0,
        0.01,
        2,
        |c| c.general.opacity,
        |c, v| c.general.opacity = v,
    ));
    style.add(&b.spin(
        "Surface opacity",
        "Tiles, cards and hovered rows",
        0.0,
        1.0,
        0.01,
        2,
        |c| c.appearance.surface_opacity,
        |c, v| c.appearance.surface_opacity = v,
    ));
    style.add(&b.spin(
        "Text size",
        "Scale factor",
        0.6,
        2.0,
        0.05,
        2,
        |c| c.appearance.font_scale,
        |c, v| c.appearance.font_scale = v,
    ));
    p.add(&style);

    let motion = group("Motion", "");
    let anim = b.expander(
        "Animations",
        "Opening, closing, lists and state changes",
        |c| c.appearance.animations,
        |c, v| c.appearance.animations = v,
    );
    anim.add_row(&b.spin(
        "Speed",
        "1 = normal, 2 = twice as fast",
        0.25,
        4.0,
        0.05,
        2,
        |c| c.appearance.animation_speed,
        |c, v| c.appearance.animation_speed = v,
    ));
    motion.add(&anim);
    p.add(&motion);

    let blur = group(
        "Blur behind",
        "Blurs and dims everything else on the monitor while open. Hyprland renders the blur (decoration.blur, via vela.lua); it has no per-surface strength, so stronger levels stack blurred layers.",
    );
    let launcher = b.expander("The launcher", "", |c| c.appearance.backdrop, |c, v| c.appearance.backdrop = v);
    backdrop_rows(
        b,
        &launcher,
        |c| c.appearance.backdrop_blur,
        |c, v| c.appearance.backdrop_blur = v,
        |c| c.appearance.backdrop_strength.into(),
        |c, v| c.appearance.backdrop_strength = v as u32,
    );
    blur.add(&launcher);
    let panel = b.expander("The control center", "", |c| c.panel.backdrop, |c, v| c.panel.backdrop = v);
    backdrop_rows(
        b,
        &panel,
        |c| c.panel.backdrop_blur,
        |c, v| c.panel.backdrop_blur = v,
        |c| c.panel.backdrop_strength.into(),
        |c, v| c.panel.backdrop_strength = v as u32,
    );
    blur.add(&panel);
    blur.add(&b.spin(
        "Dimming",
        "0 = blur only, 0.8 = much darker",
        0.0,
        0.8,
        0.02,
        2,
        |c| c.appearance.backdrop_dim,
        |c, v| c.appearance.backdrop_dim = v,
    ));
    p.add(&blur);
    p
}

// -------------------------------------------------------- Control center

fn open_control_center_row() -> adw::ButtonRow {
    let open = adw::ButtonRow::builder()
        .title("Open the control center")
        .start_icon_name("view-reveal-symbolic")
        .build();
    open.connect_activated(|_| {
        if let Ok(exe) = std::env::current_exe() {
            // vela-daemon → vela next to it.
            let vela = exe.with_file_name("vela");
            let spec = launch::SpawnSpec {
                argv: vec![vela.to_string_lossy().into_owned(), "panel".into(), "open".into()],
                name: "vela".into(),
                ..Default::default()
            };
            let _ = launch::spawn_detached(&spec, false);
        }
    });
    open
}

pub fn panel(b: &Binder) -> adw::PreferencesPage {
    let p = page();
    p.set_description("Quick settings, sound, network and notifications (vela shell). Colours, motion and blur are under Appearance.");

    let layout = group("Layout", "");
    layout.add(&b.spin(
        "Width",
        "Logical pixels",
        320.0,
        800.0,
        10.0,
        0,
        |c| c.panel.width.into(),
        |c, v| c.panel.width = v as u32,
    ));
    let centred = b.switch(
        "Centred clock",
        "Clock and date in the middle of the panel",
        |c| c.panel.clock_centered,
        |c, v| c.panel.clock_centered = v,
    );
    layout.add(&centred);
    let on_blur = b.switch(
        "Clock on the blur",
        "Clock and date large in the middle of the blurred screen; needs Appearance → Blur behind → The control center with Blur on",
        |c| c.panel.clock_on_backdrop,
        |c, v| c.panel.clock_on_backdrop = v,
    );
    layout.add(&on_blur);
    let size = b.spin(
        "Clock size",
        "Height of the time on the blur in logical pixels; the date scales along",
        48.0,
        320.0,
        4.0,
        0,
        |c| c.panel.backdrop_clock_size.into(),
        |c, v| c.panel.backdrop_clock_size = v as u32,
    );
    layout.add(&size);
    layout.add(&clock_font_row(b, |c| &c.panel.clock_font, |c, f| c.panel.clock_font = f));
    layout.add(&copy_clock_style_row(b, "Same style as the launcher clock", Config::panel_clock_like_launcher));
    let blurred = |c: &Config| c.panel.backdrop && c.panel.backdrop_blur;
    let sync = {
        let (centred, on_blur, size) = (centred.downgrade(), on_blur.downgrade(), size.downgrade());
        move |c: &Config| {
            if let (Some(centred), Some(on_blur), Some(size)) = (centred.upgrade(), on_blur.upgrade(), size.upgrade()) {
                let on = blurred(c) && c.panel.clock_on_backdrop;
                on_blur.set_sensitive(blurred(c));
                size.set_sensitive(on);
                centred.set_sensitive(!on);
            }
        }
    };
    sync(&b.store.get());
    // Not on_refresh: that one only runs on reloads, not when a switch here flips.
    b.store.subscribe(move |_, c| sync(c));
    layout.add(&b.switch(
        "Updates tile",
        "Pending system updates as a tile; opens Settings → Updates",
        |c| c.panel.updates_tile,
        |c, v| c.panel.updates_tile = v,
    ));
    if crate::components::has(crate::components::Component::Quickshare) {
        layout.add(&b.switch(
            "Quick Share tile",
            "Send and receive files with phones nearby; the tile opens Quick Share",
            |c| c.panel.quickshare_tile,
            |c, v| c.panel.quickshare_tile = v,
        ));
    }
    p.add(&layout);

    let behaviour = group("Behaviour", "");
    behaviour.add(&b.switch(
        "Close when clicking elsewhere",
        "Off: the rest of the screen stays usable while the panel is open",
        |c| c.panel.close_on_focus_loss,
        |c, v| c.panel.close_on_focus_loss = v,
    ));
    behaviour.add(&b.switch(
        "Workspace indicator",
        "Dots at the top of the screen when switching workspaces",
        |c| c.panel.workspace_osd,
        |c, v| c.panel.workspace_osd = v,
    ));
    p.add(&behaviour);

    p.add(&media_player_group(b));
    p.add(&claude_usage_group(b));

    let actions = group("", "");
    actions.add(&open_control_center_row());
    p.add(&actions);
    p
}

/// Mini player: on/off, which players, where (only with the clock on the
/// blur, which frees the top and is when the Claude card may give way) and
/// how it looks.
fn media_player_group(b: &Binder) -> adw::PreferencesGroup {
    use crate::config::MediaPosition;
    let g = group("Mini player", "Title, controls and progress of the music playing (MPRIS).");
    let on = b.switch("Show the mini player", "", |c| c.panel.media_player, |c, v| c.panel.media_player = v);
    let any = b.switch(
        "Any player",
        "Off: only Spotify. On: whatever plays, browsers included",
        |c| c.panel.media_player_any,
        |c, v| c.panel.media_player_any = v,
    );
    let top = b.switch(
        "Above the volume",
        "Where the clock was; needs Clock on the blur",
        |c| c.panel.media_player_position == MediaPosition::Top,
        |c, v| set_media_position(c, MediaPosition::Top, v),
    );
    let bottom = b.switch(
        "Instead of the Claude card",
        "At the bottom while something plays; needs Clock on the blur",
        |c| c.panel.media_player_position == MediaPosition::Bottom,
        |c, v| set_media_position(c, MediaPosition::Bottom, v),
    );
    let cover = b.switch(
        "Album art",
        "Next to the title",
        |c| c.panel.media_player_cover,
        |c, v| c.panel.media_player_cover = v,
    );
    let cover_bg = b.switch(
        "Album art as background",
        "Blurred behind the whole card",
        |c| c.panel.media_player_cover_background,
        |c, v| c.panel.media_player_cover_background = v,
    );
    let progress = b.switch(
        "Progress bar",
        "Elapsed time and length; click or drag to seek",
        |c| c.panel.media_player_progress,
        |c, v| c.panel.media_player_progress = v,
    );
    let shuffle_repeat = b.switch(
        "Shuffle and repeat",
        "Buttons next to the progress bar, when the player supports them",
        |c| c.panel.media_player_shuffle_repeat,
        |c, v| c.panel.media_player_shuffle_repeat = v,
    );
    let size = b.spin(
        "Size",
        "1 = normal; album art, text and buttons scale along",
        0.75,
        1.75,
        0.05,
        2,
        |c| c.panel.media_player_scale,
        |c, v| c.panel.media_player_scale = v,
    );
    let blur = b.spin(
        "Album art blur",
        "0 = sharp, 1 = strongest",
        0.0,
        1.0,
        0.05,
        2,
        |c| c.panel.media_player_cover_blur,
        |c, v| c.panel.media_player_cover_blur = v,
    );
    let button_size = b.spin(
        "Button size",
        "Relative to the player; 1 = normal",
        0.6,
        1.25,
        0.05,
        2,
        |c| c.panel.media_player_button_scale,
        |c, v| c.panel.media_player_button_scale = v,
    );
    let button_bg = b.switch(
        "Button backgrounds",
        "Off: icons only; shuffle and repeat show they're on in the accent colour",
        |c| c.panel.media_player_button_background,
        |c, v| c.panel.media_player_button_background = v,
    );
    for r in [&on, &any, &top, &bottom, &cover, &cover_bg] {
        g.add(r);
    }
    g.add(&blur);
    g.add(&progress);
    g.add(&shuffle_repeat);
    g.add(&size);
    g.add(&button_size);
    g.add(&button_bg);
    let sync = {
        let rows = [&any, &top, &bottom, &cover, &cover_bg, &progress, &shuffle_repeat, &button_bg].map(|r| r.downgrade());
        let (size, blur, button_size) = (size.downgrade(), blur.downgrade(), button_size.downgrade());
        move |c: &Config| {
            let [any, top, bottom, cover, cover_bg, progress, shuffle_repeat, button_bg] = &rows;
            let movable = c.panel.media_player && c.panel.clock_on_backdrop && c.panel.backdrop && c.panel.backdrop_blur;
            for r in [any, cover, cover_bg, progress, shuffle_repeat, button_bg]
                .into_iter()
                .filter_map(|w| w.upgrade())
            {
                r.set_sensitive(c.panel.media_player);
            }
            if let (Some(size), Some(blur), Some(button_size)) = (size.upgrade(), blur.upgrade(), button_size.upgrade()) {
                size.set_sensitive(c.panel.media_player);
                button_size.set_sensitive(c.panel.media_player);
                blur.set_sensitive(c.panel.media_player && c.panel.media_player_cover_background);
            }
            // One switch flipping the position flips the other one off.
            for (w, pos) in [(top, MediaPosition::Top), (bottom, MediaPosition::Bottom)] {
                if let Some(r) = w.upgrade() {
                    r.set_sensitive(movable);
                    r.set_active(c.panel.media_player_position == pos);
                }
            }
        }
    };
    sync(&b.store.get());
    b.store.subscribe(move |_, c| sync(c));
    g
}

fn set_media_position(c: &mut Config, pos: crate::config::MediaPosition, on: bool) {
    if on {
        c.panel.media_player_position = pos;
    } else if c.panel.media_player_position == pos {
        c.panel.media_player_position = crate::config::MediaPosition::Tiles;
    }
}

/// Font family of the panel clock: the current time as a preview in it, GTK's
/// font dialog (lists each family in itself) and a reset to the UI font.
/// The launcher's own clock on its blurred backdrop; needs that backdrop with Blur.
fn launcher_clock_group(b: &Binder) -> adw::PreferencesGroup {
    let g = group(
        "Clock",
        "Large on the blurred screen: above a centred launcher, on the free side of one on the left or right.",
    );
    let on = b.switch(
        "Clock on the blur",
        "Needs Appearance → Blur behind → The launcher with Blur on",
        |c| c.general.clock,
        |c, v| c.general.clock = v,
    );
    let size = b.spin(
        "Clock size",
        "Height of the time in logical pixels; the date scales along",
        48.0,
        320.0,
        4.0,
        0,
        |c| c.general.clock_size.into(),
        |c, v| c.general.clock_size = v as u32,
    );
    let font = clock_font_row(b, |c| &c.general.clock_font, |c, f| c.general.clock_font = f);
    let copy = copy_clock_style_row(b, "Same style as the control center clock", Config::launcher_clock_like_panel);
    g.add(&on);
    g.add(&size);
    g.add(&font);
    g.add(&copy);
    let sync = {
        let (on, size, font, copy) = (on.downgrade(), size.downgrade(), font.downgrade(), copy.downgrade());
        move |c: &Config| {
            let blurred = c.appearance.backdrop && c.appearance.backdrop_blur;
            if let Some(on) = on.upgrade() {
                on.set_sensitive(blurred);
            }
            for r in [
                size.upgrade().map(|r| r.upcast::<gtk::Widget>()),
                font.upgrade().map(|r| r.upcast()),
                copy.upgrade().map(|r| r.upcast()),
            ]
            .into_iter()
            .flatten()
            {
                r.set_sensitive(blurred && c.general.clock);
            }
        }
    };
    sync(&b.store.get());
    b.store.subscribe(move |_, c| sync(c));
    g
}

/// Button that copies the other clock's size and font into this one.
fn copy_clock_style_row(b: &Binder, title: &str, copy: fn(&mut Config)) -> adw::ButtonRow {
    let row = adw::ButtonRow::builder().title(title).start_icon_name("edit-copy-symbolic").build();
    let b = b.clone();
    row.connect_activated(move |_| {
        b.write(copy);
        // Spin rows only follow the config on refresh.
        b.refresh(&b.store.get());
    });
    row
}

fn clock_font_row(b: &Binder, get: fn(&Config) -> &String, set: fn(&mut Config, String)) -> adw::ActionRow {
    let row = adw::ActionRow::builder().title("Clock font").build();
    let preview = gtk::Label::builder().valign(gtk::Align::Center).build();
    let dialog = gtk::FontDialog::builder().title("Clock font").build();
    let button = gtk::FontDialogButton::builder()
        .dialog(&dialog)
        .level(gtk::FontLevel::Family)
        .use_font(true)
        .valign(gtk::Align::Center)
        .build();
    let reset = gtk::Button::builder()
        .icon_name("edit-undo-symbolic")
        .tooltip_text("Back to the UI font")
        .valign(gtk::Align::Center)
        .css_classes(["flat"])
        .build();
    row.add_suffix(&preview);
    row.add_suffix(&button);
    row.add_suffix(&reset);

    let updating = Rc::new(std::cell::Cell::new(false));
    let show = {
        let (row, preview, button, reset, updating) = (row.downgrade(), preview.downgrade(), button.downgrade(), reset.downgrade(), updating.clone());
        move |c: &Config| {
            let (Some(row), Some(preview), Some(button), Some(reset)) = (row.upgrade(), preview.upgrade(), button.upgrade(), reset.upgrade()) else {
                return;
            };
            let family = get(c).trim();
            row.set_subtitle(if family.is_empty() { "The UI font" } else { family });
            reset.set_visible(!family.is_empty());
            let ui = preview.pango_context().font_description().and_then(|d| d.family()).unwrap_or_default();
            let mut desc = gtk::pango::FontDescription::new();
            desc.set_family(if family.is_empty() { &ui } else { family });
            let attrs = gtk::pango::AttrList::new();
            let mut sized = desc.clone();
            sized.set_size(20 * gtk::pango::SCALE);
            attrs.insert(gtk::pango::AttrFontDesc::new(&sized));
            preview.set_attributes(Some(&attrs));
            let now = glib::DateTime::now_local().ok().and_then(|t| t.format("%H:%M").ok());
            preview.set_text(now.as_deref().unwrap_or("12:34"));
            updating.set(true);
            button.set_font_desc(&desc);
            updating.set(false);
        }
    };
    show(&b.store.get());
    {
        let (b, updating) = (b.clone(), updating.clone());
        button.connect_font_desc_notify(move |btn| {
            if updating.get() {
                return;
            }
            let family = btn.font_desc().and_then(|d| d.family()).map(|f| f.to_string()).unwrap_or_default();
            b.write(move |c| set(c, family));
        });
    }
    {
        let b = b.clone();
        reset.connect_clicked(move |_| b.write(|c| set(c, String::new())));
    }
    // The preview shows the time it was last drawn at; refresh it whenever the page is shown.
    {
        let (store, show) = (b.store.clone(), show.clone());
        row.connect_map(move |_| show(&store.get()));
    }
    b.store.subscribe(move |_, c| show(c));
    row
}

/// Section switch, "only ~/.claude" and one switch per Claude Code profile.
fn claude_usage_group(b: &Binder) -> adw::PreferencesGroup {
    let g = group(
        "Claude usage",
        "5-hour and 7-day limits at the bottom of the panel, per Claude Code profile (~/.claude and ccacct profiles). Uses the unofficial endpoint behind Claude Code's /usage.",
    );
    g.add(&b.switch("Show Claude usage", "", |c| c.panel.claude_usage, |c, v| c.panel.claude_usage = v));
    g.add(&b.switch(
        "Subtle style",
        "Grey text under a line at the bottom edge instead of a card",
        |c| c.panel.claude_usage_subtle,
        |c, v| c.panel.claude_usage_subtle = v,
    ));
    g.add(&b.switch(
        "Only the default account",
        "Just the profile in ~/.claude",
        |c| c.panel.claude_usage_only_default,
        |c, v| c.panel.claude_usage_only_default = v,
    ));
    // One switch per profile; rebuilt whenever the page is shown, so new
    // ccacct profiles appear without restarting vela.
    type Rows = Vec<(String, adw::SwitchRow)>;
    let rows: Rc<std::cell::RefCell<Rows>> = Rc::default();
    let sync = {
        let rows = rows.clone();
        move |c: &Config| {
            for (name, r) in rows.borrow().iter() {
                r.set_active(!c.panel.claude_usage_hidden.contains(name));
                r.set_sensitive(c.panel.claude_usage && (name == "default" || !c.panel.claude_usage_only_default));
            }
        }
    };
    let sync = Rc::new(sync);
    let rebuild = {
        let (rows, g, b, sync) = (rows.clone(), g.clone(), b.clone(), sync.clone());
        move || {
            for (_, r) in rows.borrow_mut().drain(..) {
                g.remove(&r);
            }
            for (name, dir) in crate::claude_usage::profiles(&paths::home_dir()) {
                let row = adw::SwitchRow::builder().title(&name).subtitle(paths::display_path(&dir)).build();
                g.add(&row);
                rows.borrow_mut().push((name, row));
            }
            sync(&b.store.get());
            // Handlers after the initial state, so building writes nothing.
            for (name, row) in rows.borrow().iter() {
                let (b2, n) = (b.clone(), name.clone());
                row.connect_active_notify(move |r| {
                    let (show, n) = (r.is_active(), n.clone());
                    b2.write(move |c| {
                        let hidden = c.panel.claude_usage_hidden.contains(&n);
                        if show == hidden {
                            c.panel.claude_usage_hidden.retain(|h| *h != n);
                            if !show {
                                c.panel.claude_usage_hidden.push(n);
                            }
                        }
                    });
                });
            }
        }
    };
    rebuild();
    g.connect_map(move |_| rebuild());
    {
        let sync = sync.clone();
        b.on_refresh(move |c| sync(c));
    }
    b.store.subscribe(move |old, new| {
        if old.panel != new.panel {
            sync(new);
        }
    });
    g
}

pub fn notifications(b: &Binder) -> adw::PreferencesPage {
    let p = page();
    let popups = group("Popups", "Shown in the top right corner while the panel is closed.");
    popups.add(&b.spin(
        "Duration",
        "Seconds; apps may ask for less",
        1.0,
        60.0,
        1.0,
        0,
        |c| c.panel.popup_timeout_secs.into(),
        |c, v| c.panel.popup_timeout_secs = v as u32,
    ));
    popups.add(&b.spin(
        "At once",
        "",
        1.0,
        10.0,
        1.0,
        0,
        |c| c.panel.popup_max_visible.into(),
        |c, v| c.panel.popup_max_visible = v as u32,
    ));
    popups.add(&b.switch(
        "Keep critical notifications",
        "Critical popups stay until dismissed",
        |c| c.panel.critical_popups_stay,
        |c, v| c.panel.critical_popups_stay = v,
    ));
    p.add(&popups);

    let style = group("Style", "");
    style.add(&b.switch(
        "Compact",
        "One small row per app in the panel; click it or rest the pointer on it to see everything. Popups stay full size.",
        |c| c.panel.compact_notifications,
        |c, v| c.panel.compact_notifications = v,
    ));
    p.add(&style);

    let list = group("In the panel", "");
    list.add(&b.spin(
        "Notifications per app",
        "Shown before “Show more”",
        1.0,
        10.0,
        1.0,
        0,
        |c| c.panel.group_collapsed_count.into(),
        |c, v| c.panel.group_collapsed_count = v as u32,
    ));
    p.add(&list);
    p
}

fn minutes_row(b: &Binder, get: fn(&Config) -> f64, set: fn(&mut Config, f64)) -> adw::SpinRow {
    b.spin("After", "Minutes without input", 0.5, 720.0, 0.5, 1, get, set)
}

pub fn power(b: &Binder) -> adw::PreferencesPage {
    let p = page();

    let idle = group("While you're away", "Run by vela idle (hypridle), which vela.lua starts with Hyprland.");
    let dim = b.expander("Dim the screen", "Also the keyboard backlight", |c| c.idle.dim, |c, v| c.idle.dim = v);
    dim.add_row(&minutes_row(b, |c| c.idle.dim_after_min, |c, v| c.idle.dim_after_min = v));
    idle.add(&dim);
    let lock = b.expander("Lock the screen", "", |c| c.idle.lock, |c, v| c.idle.lock = v);
    lock.add_row(&minutes_row(b, |c| c.idle.lock_after_min, |c, v| c.idle.lock_after_min = v));
    idle.add(&lock);
    let off = b.expander("Turn the screen off", "", |c| c.idle.screen_off, |c, v| c.idle.screen_off = v);
    off.add_row(&minutes_row(b, |c| c.idle.screen_off_after_min, |c, v| c.idle.screen_off_after_min = v));
    idle.add(&off);
    let suspend = b.expander(
        "Suspend",
        "Off: the computer never goes to sleep on its own",
        |c| c.idle.suspend,
        |c, v| c.idle.suspend = v,
    );
    suspend.add_row(&minutes_row(b, |c| c.idle.suspend_after_min, |c, v| c.idle.suspend_after_min = v));
    idle.add(&suspend);
    idle.add(&b.switch(
        "Lock before sleep",
        "Also when suspending by hand or closing the lid",
        |c| c.idle.lock_before_sleep,
        |c, v| c.idle.lock_before_sleep = v,
    ));
    p.add(&idle);

    let night = group("Night light", "Toggled in the control center.");
    night.add(&b.spin(
        "Temperature",
        "Kelvin; lower is warmer",
        1000.0,
        6500.0,
        100.0,
        0,
        |c| c.panel.night_light_temperature.into(),
        |c, v| c.panel.night_light_temperature = v as u32,
    ));
    p.add(&night);
    p
}

// ------------------------------------------------------------------- Search

fn status_text(s: &crate::search::engine::IndexStatus) -> String {
    let mut parts = Vec::new();
    if !s.plocate_installed {
        parts.push("plocate is not installed — using the built-in index".to_string());
    }
    for (root, backend) in &s.roots {
        let how = match backend {
            RootBackend::Plocate => "plocate",
            RootBackend::Builtin => "built-in index",
        };
        parts.push(format!("{} → {how}", paths::display_path(root)));
    }
    if s.indexing {
        parts.push("indexing…".into());
    } else if s.indexed_entries > 0 {
        let took = s.last_build_ms.map(|ms| format!(" in {ms} ms")).unwrap_or_default();
        parts.push(format!("{} paths indexed{took}", s.indexed_entries));
    }
    parts.join("\n")
}

pub fn search(daemon: &Rc<Daemon>, b: &Binder) -> adw::PreferencesPage {
    let p = page();
    let sources = group("Sources", "What typing into the launcher searches.");
    sources.add(&b.switch("Applications", "", |c| c.search.apps, |c, v| c.search.apps = v));
    sources.add(&b.switch("Files and folders", "", |c| c.search.files, |c, v| c.search.files = v));
    sources.add(&b.switch(
        "Ask Claude",
        "Offer sending the input to Claude Code",
        |c| c.search.claude,
        |c, v| c.search.claude = v,
    ));
    p.add(&sources);

    let limits = group("Results", "");
    limits.add(&b.spin(
        "Application results",
        "",
        1.0,
        50.0,
        1.0,
        0,
        |c| c.search.max_app_results.into(),
        |c, v| c.search.max_app_results = v as u32,
    ));
    limits.add(&b.spin(
        "File results",
        "",
        1.0,
        100.0,
        1.0,
        0,
        |c| c.search.max_file_results.into(),
        |c, v| c.search.max_file_results = v as u32,
    ));
    limits.add(&b.spin(
        "Minimum length for file search",
        "Characters",
        1.0,
        10.0,
        1.0,
        0,
        |c| c.search.min_file_query_len.into(),
        |c, v| c.search.min_file_query_len = v as u32,
    ));
    limits.add(&b.spin(
        "File search delay",
        "Milliseconds after the last keystroke",
        0.0,
        2000.0,
        10.0,
        0,
        |c| c.search.debounce_ms.into(),
        |c, v| c.search.debounce_ms = v as u32,
    ));
    p.add(&limits);

    let files = group("File search", "");
    let labels: Vec<&str> = FileBackend::ALL.iter().map(|m| m.label()).collect();
    files.add(&b.combo(
        "Backend",
        "Automatic uses plocate for folders its database covers and the built-in index otherwise",
        &labels,
        |c| FileBackend::ALL.iter().position(|m| *m == c.search.file_backend).unwrap_or(0),
        |c, i| c.search.file_backend = FileBackend::ALL[i.min(FileBackend::ALL.len() - 1)],
    ));
    files.add(&b.switch(
        "Include hidden files",
        "Files and folders starting with a dot",
        |c| c.search.include_hidden,
        |c, v| c.search.include_hidden = v,
    ));
    files.add(&b.switch("Include folders", "", |c| c.search.include_directories, |c, v| c.search.include_directories = v));
    files.add(&b.entry(
        "Excluded folder names (comma separated)",
        |c| c.search.exclude.join(", "),
        |c, v| {
            c.search.exclude = v.split(',').map(|s| s.trim().to_owned()).filter(|s| !s.is_empty()).collect();
        },
    ));
    files.add(&b.spin(
        "Rebuild built-in index every",
        "Minutes (it also refreshes when the launcher opens)",
        1.0,
        1440.0,
        1.0,
        0,
        |c| c.search.index_interval_minutes.into(),
        |c, v| c.search.index_interval_minutes = v as u32,
    ));
    p.add(&files);

    // Search roots
    let roots = group("Searched folders", "");
    let add = gtk::Button::builder()
        .icon_name("list-add-symbolic")
        .tooltip_text("Add folder")
        .css_classes(["flat"])
        .build();
    roots.set_header_suffix(Some(&add));
    let rows: Rc<std::cell::RefCell<Vec<adw::ActionRow>>> = Rc::default();
    let rebuild = {
        let roots = roots.clone();
        let rows = rows.clone();
        let store = b.store.clone();
        move |c: &Config| {
            for r in rows.borrow_mut().drain(..) {
                roots.remove(&r);
            }
            for (i, root) in c.search.file_roots.iter().enumerate() {
                let row = adw::ActionRow::builder().title(root.as_str()).build();
                row.add_prefix(&gtk::Image::from_icon_name("folder-symbolic"));
                let del = gtk::Button::builder()
                    .icon_name("user-trash-symbolic")
                    .valign(gtk::Align::Center)
                    .css_classes(["flat"])
                    .tooltip_text("Remove")
                    .build();
                let store = store.clone();
                del.connect_clicked(move |_| {
                    store.update(|c| {
                        if i < c.search.file_roots.len() {
                            c.search.file_roots.remove(i);
                        }
                    })
                });
                row.add_suffix(&del);
                roots.add(&row);
                rows.borrow_mut().push(row);
            }
        }
    };
    rebuild(&b.store.get());
    let r2 = rebuild.clone();
    b.store.subscribe(move |old, new| {
        if old.search.file_roots != new.search.file_roots {
            r2(new);
        }
    });
    let store = b.store.clone();
    add.connect_clicked(move |btn| {
        let dialog = gtk::FileDialog::builder().title("Add a folder to search").build();
        let store = store.clone();
        let win = btn.root().and_downcast::<gtk::Window>();
        dialog.select_folder(win.as_ref(), gio::Cancellable::NONE, move |res| {
            if let Ok(folder) = res
                && let Some(path) = folder.path()
            {
                let s = paths::display_path(&path);
                store.update(|c| {
                    if !c.search.file_roots.contains(&s) {
                        c.search.file_roots.push(s);
                    }
                });
            }
        });
    });
    p.add(&roots);

    let status = group("Index", "");
    let status_row = adw::ActionRow::builder()
        .title("Status")
        .subtitle("Waiting for the first search…")
        .subtitle_lines(0)
        .build();
    if let Some(s) = daemon.index_status.borrow().as_ref() {
        status_row.set_subtitle(&status_text(s));
    }
    let w = status_row.downgrade();
    daemon.subscribe_status(move |s| {
        if let Some(r) = w.upgrade() {
            r.set_subtitle(&status_text(s));
        }
    });
    let rebuild_btn = gtk::Button::builder().label("Rebuild now").valign(gtk::Align::Center).build();
    let d = daemon.clone();
    rebuild_btn.connect_clicked(move |_| d.engine.rebuild_index());
    status_row.add_suffix(&rebuild_btn);
    status.add(&status_row);
    status.add(
        &adw::ActionRow::builder()
            .title("plocate and btrfs")
            .subtitle(
                "On btrfs, updatedb skips /home as a bind mount. To let plocate cover your home folder, set PRUNE_BIND_MOUNTS = \"no\" in /etc/updatedb.conf and run sudo updatedb. The built-in index works without this.",
            )
            .subtitle_lines(0)
            .build(),
    );
    p.add(&status);
    p
}

// ------------------------------------------------------------------- Claude

pub fn claude(b: &Binder) -> adw::PreferencesPage {
    let p = page();
    let behaviour = group("Ask Claude", "");
    behaviour.add(&b.switch(
        "Always show “Ask Claude”",
        "Otherwise it only appears when nothing else matches",
        |c| c.claude.always_visible,
        |c, v| c.claude.always_visible = v,
    ));
    behaviour.add(&b.switch(
        "Shift+Enter asks Claude",
        "Send the whole input to Claude regardless of the selection",
        |c| c.claude.shift_enter,
        |c, v| c.claude.shift_enter = v,
    ));
    behaviour.add(&b.switch(
        "Prefer Claude for questions",
        "Put “Ask Claude” first when the input reads like a question",
        |c| c.claude.prefer_for_questions,
        |c, v| c.claude.prefer_for_questions = v,
    ));
    p.add(&behaviour);

    let cmd = group("Command", "The prompt is passed as a single argument — it is never interpreted by a shell.");
    let exe = b.entry("Claude executable", |c| c.claude.executable.clone(), |c, v| c.claude.executable = v);
    add_exec_status(&exe, b, |c| c.claude.executable.clone());
    cmd.add(&exe);

    let args = adw::EntryRow::builder().title("Arguments").text(join_args(&b.store.get().claude.args)).build();
    let b2 = b.clone();
    args.connect_changed(move |r| {
        if let Some(v) = split_args(&r.text()) {
            r.remove_css_class("error");
            b2.write(|c| c.claude.args = v);
        } else {
            r.add_css_class("error");
        }
    });
    let w = args.downgrade();
    b.on_refresh(move |c| {
        if let Some(r) = w.upgrade() {
            r.set_text(&join_args(&c.claude.args));
        }
    });
    cmd.add(&args);

    let wd = b.entry("Working directory", |c| c.claude.working_dir.clone(), |c, v| c.claude.working_dir = v);
    let pick = gtk::Button::builder()
        .icon_name("folder-open-symbolic")
        .valign(gtk::Align::Center)
        .css_classes(["flat"])
        .tooltip_text("Choose folder")
        .build();
    let store = b.store.clone();
    pick.connect_clicked(move |btn| {
        let dialog = gtk::FileDialog::builder().title("Working directory for Claude").build();
        let store = store.clone();
        let win = btn.root().and_downcast::<gtk::Window>();
        dialog.select_folder(win.as_ref(), gio::Cancellable::NONE, move |res| {
            if let Ok(f) = res
                && let Some(path) = f.path()
            {
                store.replace(
                    {
                        let mut c = (*store.get()).clone();
                        c.claude.working_dir = paths::display_path(&path);
                        c
                    },
                    true,
                );
            }
        });
    });
    wd.add_suffix(&pick);
    cmd.add(&wd);

    let preview = adw::ActionRow::builder()
        .title("Resulting command")
        .subtitle_selectable(true)
        .subtitle_lines(0)
        .build();
    let update = {
        let preview = preview.clone();
        move |c: &Config| {
            let mut argv = vec![c.terminal.executable.clone()];
            argv.extend(c.terminal.effective_exec_args());
            argv.push(c.claude.executable.clone());
            argv.extend(c.claude.args.iter().cloned());
            if !c.claude.args.iter().any(|a| a == "--") {
                argv.push("--".into());
            }
            argv.push("<your input>".into());
            preview.set_subtitle(&glib::markup_escape_text(&join_args(&argv)));
        }
    };
    update(&b.store.get());
    b.store.subscribe(move |_, new| update(new));
    cmd.add(&preview);
    p.add(&cmd);

    let term = group("Terminal", "Claude runs in the terminal configured under General → Terminal.");
    p.add(&term);
    p
}
