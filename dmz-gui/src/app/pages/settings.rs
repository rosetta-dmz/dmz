use std::path::{Path, PathBuf};

use eframe::egui;
use serde::{Deserialize, Serialize};

use crate::app::theme::ThemeFile;

const PREFERENCES_KEY: &str = "dmz.appearance";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum WidgetSize {
    Small,
    Medium,
    Large,
}

impl WidgetSize {
    fn label(self) -> &'static str {
        match self {
            Self::Small => "Small",
            Self::Medium => "Medium",
            Self::Large => "Large",
        }
    }

    fn scale(self) -> f32 {
        match self {
            Self::Small => 0.9,
            Self::Medium => 0.95,
            Self::Large => 1.0,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
struct Preferences {
    theme_name: String,
    widget_size: WidgetSize,
    show_status_bar: bool,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            theme_name: String::from("System"),
            widget_size: WidgetSize::Large,
            show_status_bar: true,
        }
    }
}

pub(super) struct AppearanceState {
    preferences: Preferences,
    imported_themes: Vec<ThemeFile>,
    themes_dir: Option<PathBuf>,
    gallery_open: bool,
}

impl Default for AppearanceState {
    fn default() -> Self {
        Self {
            preferences: Preferences::default(),
            imported_themes: Vec::new(),
            themes_dir: eframe::storage_dir("DMZ").map(|dir| dir.join("themes")),
            gallery_open: false,
        }
    }
}

impl AppearanceState {
    pub(super) fn load(storage: Option<&dyn eframe::Storage>, status_message: &mut String) -> Self {
        let preferences = storage
            .and_then(|storage| storage.get_string(PREFERENCES_KEY))
            .map(
                |stored| match serde_json::from_str::<Preferences>(&stored) {
                    Ok(preferences) => preferences,
                    Err(error) => {
                        *status_message = format!("Could not load appearance settings: {error}");
                        Preferences::default()
                    }
                },
            )
            .unwrap_or_default();

        let themes_dir = eframe::storage_dir("DMZ").map(|dir| dir.join("themes"));
        let imported_themes = match themes_dir.as_deref() {
            Some(directory) => match load_themes(directory) {
                Ok(themes) => themes,
                Err(error) => {
                    *status_message = format!("Could not load local themes: {error}");
                    Vec::new()
                }
            },
            None => {
                *status_message = "Could not locate the DMZ application data directory.".into();
                Vec::new()
            }
        };

        let mut state = Self {
            preferences,
            imported_themes,
            themes_dir,
            gallery_open: false,
        };
        if state.preferences.theme_name != "System" && state.current_theme().is_none() {
            state.preferences.theme_name = String::from("System");
        }
        state
    }

    pub(super) fn save(&self, storage: &mut dyn eframe::Storage) {
        match serde_json::to_string(&self.preferences) {
            Ok(serialized) => storage.set_string(PREFERENCES_KEY, serialized),
            Err(error) => {
                tracing::error!(%error, "could not serialize appearance preferences");
            }
        }
    }

    pub(super) fn apply(&self, ctx: &egui::Context) -> Result<egui::Visuals, String> {
        let mut style = egui::Style::default();
        let scale = self.preferences.widget_size.scale();
        style.spacing.item_spacing *= scale;
        style.spacing.button_padding *= scale;
        style.spacing.interact_size *= scale;
        for font in style.text_styles.values_mut() {
            font.size *= scale;
        }

        if self.preferences.theme_name == "System" {
            ctx.set_theme(egui::ThemePreference::System);
            for (theme, name) in [
                (egui::Theme::Dark, "Mac Dark"),
                (egui::Theme::Light, "Mac Light"),
            ] {
                let palette = ThemeFile::builtins()
                    .into_iter()
                    .find(|candidate| candidate.name == name)
                    .ok_or_else(|| format!("Built-in theme {name:?} is unavailable"))?;
                let mut themed_style = style.clone();
                themed_style.visuals = palette.visuals()?;
                ctx.set_style_of(theme, themed_style);
            }
            let active = ctx.theme();
            return Ok(ctx.style_of(active).visuals.clone());
        }

        let theme = self.current_theme().ok_or_else(|| {
            format!(
                "Selected theme {:?} is not available",
                self.preferences.theme_name
            )
        })?;
        let visuals = theme.visuals()?;
        ctx.set_theme(theme.egui_theme());
        style.visuals = visuals.clone();
        ctx.set_style_of(egui::Theme::Dark, style.clone());
        ctx.set_style_of(egui::Theme::Light, style);
        Ok(visuals)
    }

    pub(super) fn show(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        status_message: &mut String,
    ) {
        let muted = ui.visuals().widgets.noninteractive.fg_stroke.color;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                super::super::components::page_header(
                    ui,
                    "Appearance",
                    "Personalize the look and feel of your DMZ workspace.",
                );

                super::super::components::section(ui, |ui| {
                    ui.heading("Theme");
                    ui.label(
                        egui::RichText::new("Choose a palette or import a theme file.")
                            .color(muted),
                    );
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.label(egui::RichText::new("Current theme").color(muted).size(12.0));
                            ui.label(
                                egui::RichText::new(self.preferences.theme_name.as_str()).strong(),
                            );
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if super::super::components::action_button(ui, "Browse themes", true)
                                .clicked()
                            {
                                self.gallery_open = true;
                            }
                            if super::super::components::action_button(ui, "Import JSON...", false)
                                .clicked()
                            {
                                self.import_theme(status_message);
                            }
                        });
                    });
                });

                ui.add_space(16.0);
                super::super::components::section(ui, |ui| {
                    ui.heading("Interface");
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.label("Widget size");
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            egui::ComboBox::from_id_salt("widget_size_combo")
                                .selected_text(self.preferences.widget_size.label())
                                .width(140.0)
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(
                                        &mut self.preferences.widget_size,
                                        WidgetSize::Small,
                                        "Small",
                                    );
                                    ui.selectable_value(
                                        &mut self.preferences.widget_size,
                                        WidgetSize::Medium,
                                        "Medium",
                                    );
                                    ui.selectable_value(
                                        &mut self.preferences.widget_size,
                                        WidgetSize::Large,
                                        "Large",
                                    );
                                });
                        });
                    });
                    ui.separator();
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.label("Show status bar");
                            ui.label(
                                egui::RichText::new(
                                    "Show the current workspace status in the main area.",
                                )
                                .color(muted)
                                .size(12.0),
                            );
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            super::super::components::toggle(
                                ui,
                                &mut self.preferences.show_status_bar,
                            );
                        });
                    });
                });
            });

        self.show_gallery(ctx);
    }

    pub(super) fn show_status_bar(&self) -> bool {
        self.preferences.show_status_bar
    }

    fn current_theme(&self) -> Option<ThemeFile> {
        ThemeFile::builtins()
            .into_iter()
            .find(|theme| theme.name == self.preferences.theme_name)
            .or_else(|| {
                self.imported_themes
                    .iter()
                    .find(|theme| theme.name == self.preferences.theme_name)
                    .cloned()
            })
    }

    fn show_gallery(&mut self, ctx: &egui::Context) {
        let themes = ThemeFile::builtins()
            .into_iter()
            .chain(self.imported_themes.iter().cloned())
            .collect::<Vec<_>>();
        let mut selected = None;
        egui::Window::new("Theme gallery")
            .open(&mut self.gallery_open)
            .default_size(egui::vec2(540.0, 480.0))
            .min_size(egui::vec2(440.0, 280.0))
            .resizable(true)
            .show(ctx, |ui| {
                ui.label(
                    egui::RichText::new("Pick a palette for your DMZ workspace.")
                        .color(ui.visuals().widgets.noninteractive.fg_stroke.color),
                );
                ui.add_space(8.0);
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for theme in &themes {
                            let is_selected = self.preferences.theme_name == theme.name;
                            super::super::components::card(ui, is_selected, |ui| {
                                ui.horizontal(|ui| {
                                    ui.vertical(|ui| {
                                        ui.label(
                                            egui::RichText::new(theme.name.as_str())
                                                .strong()
                                                .size(15.0),
                                        );
                                        let meta = theme
                                            .author
                                            .as_deref()
                                            .map(|author| {
                                                format!("{author} · {}", theme.appearance_label())
                                            })
                                            .unwrap_or_else(|| theme.appearance_label().to_owned());
                                        ui.label(
                                            egui::RichText::new(meta)
                                                .color(
                                                    ui.visuals()
                                                        .widgets
                                                        .noninteractive
                                                        .fg_stroke
                                                        .color,
                                                )
                                                .size(12.0),
                                        );
                                    });
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            if is_selected {
                                                ui.label(
                                                    egui::RichText::new("Selected")
                                                        .color(ui.visuals().selection.stroke.color)
                                                        .strong(),
                                                );
                                            } else if super::super::components::action_button(
                                                ui,
                                                "Use theme",
                                                true,
                                            )
                                            .clicked()
                                            {
                                                selected = Some(theme.name.clone());
                                            }
                                        },
                                    );
                                });
                                ui.add_space(8.0);
                                match theme.palette_preview() {
                                    Ok(colors) => {
                                        ui.horizontal(|ui| {
                                            for color in colors {
                                                super::super::components::swatch(ui, color);
                                            }
                                        });
                                    }
                                    Err(error) => {
                                        ui.colored_label(
                                            ui.visuals().error_fg_color,
                                            format!("Invalid theme palette: {error}"),
                                        );
                                    }
                                }
                            });
                            ui.add_space(8.0);
                        }
                        if themes.is_empty() {
                            ui.label("No themes are available.");
                        }
                    });
            });

        if let Some(name) = selected {
            self.preferences.theme_name = name;
        }
    }

    fn import_theme(&mut self, status_message: &mut String) {
        let Some(source) = rfd::FileDialog::new()
            .add_filter("DMZ theme JSON", &["json"])
            .pick_file()
        else {
            return;
        };

        match self.import_theme_file(&source) {
            Ok(theme) => {
                self.preferences.theme_name = theme.name.clone();
                *status_message = format!("Imported and applied theme '{}'.", theme.name);
            }
            Err(error) => {
                *status_message = format!("Theme import failed: {error}");
            }
        }
    }

    fn import_theme_file(&mut self, source: &Path) -> Result<ThemeFile, String> {
        let contents = std::fs::read_to_string(source)
            .map_err(|error| format!("cannot read {}: {error}", source.display()))?;
        let theme: ThemeFile = serde_json::from_str(&contents)
            .map_err(|error| format!("invalid theme JSON: {error}"))?;
        theme.validate()?;

        if ThemeFile::builtins()
            .iter()
            .any(|builtin| builtin.name == theme.name)
        {
            return Err(format!(
                "theme name '{}' is reserved for a built-in theme",
                theme.name
            ));
        }
        let directory = self
            .themes_dir
            .as_ref()
            .ok_or_else(|| "DMZ application data directory is unavailable".to_string())?;
        let slug = theme_slug(&theme.name);
        if slug.is_empty() {
            return Err("theme name must contain at least one letter or number".into());
        }
        std::fs::create_dir_all(directory)
            .map_err(|error| format!("cannot create {}: {error}", directory.display()))?;
        let destination = directory.join(format!("{slug}.json"));
        if destination.exists() {
            let existing = std::fs::read_to_string(&destination)
                .map_err(|error| format!("cannot read {}: {error}", destination.display()))?;
            let existing: ThemeFile = serde_json::from_str(&existing)
                .map_err(|error| format!("invalid existing theme file: {error}"))?;
            if existing.name != theme.name {
                return Err(format!(
                    "theme name '{}' conflicts with existing theme '{}'",
                    theme.name, existing.name
                ));
            }
        }
        let serialized = serde_json::to_vec_pretty(&theme)
            .map_err(|error| format!("cannot serialize theme: {error}"))?;
        std::fs::write(&destination, serialized)
            .map_err(|error| format!("cannot write {}: {error}", destination.display()))?;

        self.imported_themes
            .retain(|existing| existing.name != theme.name);
        self.imported_themes.push(theme.clone());
        Ok(theme)
    }
}

fn load_themes(directory: &Path) -> Result<Vec<ThemeFile>, String> {
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("cannot read {}: {error}", directory.display())),
    };
    let mut themes = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| format!("cannot read theme directory entry: {error}"))?;
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let contents = std::fs::read_to_string(&path)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        let theme: ThemeFile = serde_json::from_str(&contents)
            .map_err(|error| format!("invalid theme {}: {error}", path.display()))?;
        theme.validate()?;
        themes.push(theme);
    }
    Ok(themes)
}

fn theme_slug(name: &str) -> String {
    name.chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preferences_round_trip_and_default_missing_fields() {
        let preferences = Preferences::default();
        let serialized = serde_json::to_string(&preferences).expect("serializes");
        let restored: Preferences = serde_json::from_str(&serialized).expect("deserializes");
        assert_eq!(restored.theme_name, "System");
        assert_eq!(restored.widget_size, WidgetSize::Large);
        assert!(restored.show_status_bar);

        let partial: Preferences =
            serde_json::from_str(r#"{"theme_name":"Dark Slate"}"#).expect("defaults missing keys");
        assert_eq!(partial.widget_size, WidgetSize::Large);
        assert!(partial.show_status_bar);
    }

    #[test]
    fn default_appearance_follows_system_theme() {
        let state = AppearanceState::default();
        let ctx = egui::Context::default();

        state.apply(&ctx).expect("default theme applies");

        assert_eq!(state.preferences.theme_name, "System");
        assert_eq!(ctx.theme(), ctx.system_theme().unwrap_or(egui::Theme::Dark));
        assert_eq!(
            ctx.style_of(egui::Theme::Dark).visuals.panel_fill,
            egui::Color32::from_rgb(20, 20, 22)
        );
        assert_eq!(
            ctx.style_of(egui::Theme::Light).visuals.panel_fill,
            egui::Color32::from_rgb(246, 246, 248)
        );
    }

    #[test]
    fn selecting_mac_light_switches_egui_to_light_theme() {
        let mut state = AppearanceState::default();
        state.preferences.theme_name = String::from("Mac Light");
        let ctx = egui::Context::default();

        state.apply(&ctx).expect("Mac Light theme applies");

        assert_eq!(ctx.theme(), egui::Theme::Light);
        assert_eq!(
            ctx.style_of(egui::Theme::Light).visuals.panel_fill,
            egui::Color32::from_rgb(246, 246, 248)
        );
    }
}
