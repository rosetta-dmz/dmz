use eframe::egui;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct ThemeFile {
    #[serde(default = "default_schema")]
    pub schema: u32,
    pub name: String,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub appearance: Appearance,
    pub palette: ThemePalette,
}

fn default_schema() -> u32 {
    1
}

impl Default for ThemeFile {
    fn default() -> Self {
        Self {
            schema: 1,
            name: String::new(),
            author: None,
            appearance: Appearance::Dark,
            palette: ThemePalette::default(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum Appearance {
    Light,
    #[default]
    Dark,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct ThemePalette {
    #[serde(alias = "bg")]
    pub background: String,
    #[serde(alias = "card")]
    pub panel: String,
    #[serde(alias = "card_2")]
    pub panel_alt: String,
    #[serde(alias = "text_primary")]
    pub text: String,
    #[serde(default)]
    pub text_body: Option<String>,
    #[serde(alias = "text_dim")]
    pub muted: String,
    pub accent: String,
    pub border: String,
}

impl Default for ThemePalette {
    fn default() -> Self {
        Self {
            background: String::from("#141416"),
            panel: String::from("#1e1e22"),
            panel_alt: String::from("#242428"),
            text: String::from("#eeeeF4"),
            text_body: None,
            muted: String::from("#9696a5"),
            accent: String::from("#0a84ff"),
            border: String::from("#ffffff1a"),
        }
    }
}

impl ThemeFile {
    pub(super) fn builtins() -> [Self; 4] {
        [
            Self {
                name: String::from("Mac Dark"),
                ..Self::default()
            },
            Self {
                name: String::from("Mac Light"),
                appearance: Appearance::Light,
                palette: ThemePalette {
                    background: String::from("#f6f6f8"),
                    panel: String::from("#ffffff"),
                    panel_alt: String::from("#fafafc"),
                    text: String::from("#1c1c1e"),
                    text_body: Some(String::from("#2c2c2e")),
                    muted: String::from("#8e8e93"),
                    accent: String::from("#007aff"),
                    border: String::from("#00000014"),
                },
                ..Self::default()
            },
            Self {
                name: String::from("Dark Slate"),
                palette: ThemePalette {
                    background: String::from("#121a24"),
                    panel: String::from("#1b2634"),
                    panel_alt: String::from("#263547"),
                    text: String::from("#eff6ff"),
                    text_body: None,
                    muted: String::from("#a6b7ca"),
                    accent: String::from("#66c2ff"),
                    border: String::from("#34495f"),
                },
                ..Self::default()
            },
            Self {
                name: String::from("Field Ops"),
                palette: ThemePalette {
                    background: String::from("#151a15"),
                    panel: String::from("#1d251d"),
                    panel_alt: String::from("#293329"),
                    text: String::from("#e5ebdf"),
                    text_body: Some(String::from("#d4dfce")),
                    muted: String::from("#9aa893"),
                    accent: String::from("#8eaa5e"),
                    border: String::from("#3a4937"),
                },
                ..Self::default()
            },
        ]
    }

    pub(super) fn palette_preview(&self) -> Result<[egui::Color32; 6], String> {
        Ok([
            parse_color(&self.palette.background)?,
            parse_color(&self.palette.panel)?,
            parse_color(&self.palette.panel_alt)?,
            parse_color(&self.palette.accent)?,
            parse_color(
                self.palette
                    .text_body
                    .as_deref()
                    .unwrap_or(&self.palette.text),
            )?,
            parse_color(&self.palette.border)?,
        ])
    }

    pub(super) fn appearance_label(&self) -> &'static str {
        match self.appearance {
            Appearance::Light => "Light appearance",
            Appearance::Dark => "Dark appearance",
        }
    }

    pub(super) fn egui_theme(&self) -> egui::Theme {
        match self.appearance {
            Appearance::Light => egui::Theme::Light,
            Appearance::Dark => egui::Theme::Dark,
        }
    }

    pub(super) fn validate(&self) -> Result<(), String> {
        if self.schema != 1 {
            return Err(format!(
                "Unsupported theme schema {}; expected schema 1",
                self.schema
            ));
        }
        if self.name.trim().is_empty() {
            return Err("Theme name cannot be empty".to_string());
        }
        for (name, value) in [
            ("background", &self.palette.background),
            ("panel", &self.palette.panel),
            ("panel_alt", &self.palette.panel_alt),
            ("text", &self.palette.text),
            ("muted", &self.palette.muted),
            ("accent", &self.palette.accent),
            ("border", &self.palette.border),
        ] {
            parse_color(value).map_err(|error| format!("Invalid {name} color: {error}"))?;
        }
        Ok(())
    }

    pub(super) fn visuals(&self) -> Result<egui::Visuals, String> {
        self.validate()?;
        let mut visuals = match self.appearance {
            Appearance::Light => egui::Visuals::light(),
            Appearance::Dark => egui::Visuals::dark(),
        };
        visuals.window_fill = parse_color(&self.palette.background)?;
        visuals.panel_fill = parse_color(&self.palette.background)?;
        visuals.extreme_bg_color = parse_color(&self.palette.panel)?;
        visuals.faint_bg_color = parse_color(&self.palette.panel_alt)?;
        let text = parse_color(
            self.palette
                .text_body
                .as_deref()
                .unwrap_or(&self.palette.text),
        )?;
        let muted = parse_color(&self.palette.muted)?;
        let accent = parse_color(&self.palette.accent)?;
        let border = parse_color(&self.palette.border)?;
        visuals.override_text_color = Some(text);
        visuals.selection.bg_fill = accent.gamma_multiply(0.16);
        visuals.selection.stroke = egui::Stroke::new(1.0, accent);
        visuals.widgets.active.bg_fill = accent;
        visuals.widgets.active.fg_stroke.color = text;
        visuals.widgets.active.bg_stroke.color = accent;
        visuals.widgets.active.expansion = 0.0;
        visuals.widgets.hovered.bg_fill = parse_color(&self.palette.panel_alt)?;
        visuals.widgets.hovered.fg_stroke.color = text;
        visuals.widgets.hovered.bg_stroke.color = accent.gamma_multiply(0.55);
        visuals.widgets.hovered.expansion = 0.0;
        visuals.widgets.open.expansion = 0.0;
        visuals.widgets.inactive.bg_fill = parse_color(&self.palette.panel)?;
        visuals.widgets.inactive.fg_stroke.color = text;
        visuals.widgets.inactive.bg_stroke.color = border;
        // egui sizes button padding as `button_padding - bg_stroke.width`, so every
        // interactive state must share the same stroke width or buttons resize on hover/press.
        visuals.widgets.inactive.bg_stroke.width = visuals.widgets.hovered.bg_stroke.width;
        visuals.widgets.noninteractive.bg_fill = parse_color(&self.palette.panel)?;
        visuals.widgets.noninteractive.fg_stroke.color = muted;
        visuals.widgets.noninteractive.bg_stroke.color = border;
        Ok(visuals)
    }
}

fn parse_color(value: &str) -> Result<egui::Color32, String> {
    let hex = value.strip_prefix('#').unwrap_or(value);
    if hex.len() != 6 && hex.len() != 8 {
        return Err(format!("expected #RRGGBB or #RRGGBBAA, got {value:?}"));
    }
    if !hex.is_ascii() {
        return Err(format!("not a valid hex color: {value:?}"));
    }
    let channels = hex
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let pair = std::str::from_utf8(pair)
                .map_err(|_| format!("not a valid hex color: {value:?}"))?;
            u8::from_str_radix(pair, 16).map_err(|_| format!("not a valid hex color: {value:?}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(egui::Color32::from_rgba_unmultiplied(
        channels[0],
        channels[1],
        channels[2],
        channels.get(3).copied().unwrap_or(255),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_theme_with_six_digit_colors() {
        let theme = ThemeFile::builtins()[0].clone();
        assert!(theme.validate().is_ok());
    }

    #[test]
    fn mac_light_is_available_with_light_palette() {
        let theme = ThemeFile::builtins()
            .into_iter()
            .find(|theme| theme.name == "Mac Light")
            .expect("Mac Light is a built-in theme");
        assert!(matches!(theme.appearance, Appearance::Light));
        assert!(theme.validate().is_ok());

        let visuals = theme.visuals().expect("Mac Light visuals are valid");
        assert_eq!(visuals.panel_fill, parse_color("#f6f6f8").unwrap());
        assert_eq!(visuals.extreme_bg_color, parse_color("#ffffff").unwrap());
        assert_eq!(
            visuals.override_text_color,
            Some(parse_color("#2c2c2e").unwrap())
        );
        assert_eq!(
            visuals.widgets.active.bg_fill,
            parse_color("#007aff").unwrap()
        );
        assert_eq!(visuals.widgets.hovered.expansion, 0.0);
        assert_eq!(visuals.widgets.active.expansion, 0.0);
        assert_eq!(
            visuals.widgets.inactive.bg_stroke.width,
            visuals.widgets.hovered.bg_stroke.width
        );
        assert_eq!(
            visuals.widgets.inactive.bg_stroke.width,
            visuals.widgets.active.bg_stroke.width
        );
    }

    #[test]
    fn field_ops_is_available_with_valid_military_green_palette() {
        let theme = ThemeFile::builtins()
            .into_iter()
            .find(|theme| theme.name == "Field Ops")
            .expect("Field Ops is a built-in theme");
        assert!(matches!(theme.appearance, Appearance::Dark));
        assert!(theme.validate().is_ok());

        let visuals = theme.visuals().expect("Field Ops visuals are valid");
        assert_eq!(visuals.panel_fill, parse_color("#151a15").unwrap());
        assert_eq!(visuals.extreme_bg_color, parse_color("#1d251d").unwrap());
        assert_eq!(
            visuals.widgets.active.bg_fill,
            parse_color("#8eaa5e").unwrap()
        );
    }

    #[test]
    fn rejects_invalid_theme_colors() {
        let mut theme = ThemeFile::builtins()[0].clone();
        theme.palette.accent = "not-a-color".to_string();
        assert!(theme.validate().is_err());
    }

    #[test]
    fn parses_dmz_theme_json() {
        let json = r##"{
            "name":"Test",
            "palette":{
                "background":"#111419",
                "panel":"#181d22",
                "panel_alt":"#242a30",
                "text":"#ffffff",
                "muted":"#939ba7",
                "accent":"#3faaff",
                "border":"#292f36"
            }
        }"##;
        let theme: ThemeFile = serde_json::from_str(json).expect("theme parses");
        assert!(theme.validate().is_ok());
    }

    #[test]
    fn parses_reference_component_theme_palette() {
        let json = r##"{
            "name":"Reference",
            "appearance":"dark",
            "palette":{
                "bg":"#141416",
                "card":"#1e1e22",
                "card_2":"#242428",
                "text_primary":"#eeeeF4",
                "text_secondary":"#c8c8d2",
                "text_body":"#dcdce4",
                "text_dim":"#9696a5",
                "text_disabled":"#767680",
                "border":"#ffffff1a",
                "border_hovered":"#ffffff33",
                "accent":"#0a84ff",
                "accent_dim":"#409cff",
                "accent_tint":"#0a84ff29",
                "danger":"#ff453a",
                "success":"#30d158"
            }
        }"##;
        let theme: ThemeFile = serde_json::from_str(json).expect("reference theme parses");
        assert!(theme.validate().is_ok());
        let visuals = theme.visuals().expect("reference theme applies");
        assert_eq!(visuals.panel_fill, parse_color("#141416").unwrap());
        assert_eq!(
            visuals.widgets.inactive.bg_fill,
            parse_color("#1e1e22").unwrap()
        );
    }

    #[test]
    fn rejects_non_ascii_color_without_panicking() {
        assert!(parse_color("éabcde").is_err());
    }
}
