use eframe::egui;

use super::super::components;
use dmz_sandbox::agent::AgentSandbox;

pub(super) struct AgentState {
    agent_id: String,
    allowed_domains: String,
    scratchpad_mb: usize,
    active_agents: Vec<String>,
}

impl Default for AgentState {
    fn default() -> Self {
        Self {
            agent_id: String::from("agent-01"),
            allowed_domains: String::from("api.openai.com, *.github.com, crates.io"),
            scratchpad_mb: 512,
            active_agents: Vec::new(),
        }
    }
}

pub(super) fn show(state: &mut AgentState, ui: &mut egui::Ui, status_message: &mut String) {
    components::page_header(
        ui,
        "Agent Sandbox",
        "Configure isolated scratch space and outbound domain rules for agents.",
    );

    components::section(ui, |ui| {
        components::section_header(ui, "Sandbox configuration");
        egui::Grid::new("agent_grid")
            .num_columns(2)
            .spacing([16.0, 8.0])
            .min_col_width(100.0)
            .show(ui, |ui| {
                ui.label("Agent identifier");
                ui.add_sized(
                    egui::vec2(ui.available_width(), 32.0),
                    egui::TextEdit::singleline(&mut state.agent_id),
                );
                ui.end_row();

                ui.label("Allowed domains");
                ui.add_sized(
                    egui::vec2(ui.available_width(), 32.0),
                    egui::TextEdit::singleline(&mut state.allowed_domains)
                        .hint_text("api.example.com, *.example.org"),
                );
                ui.end_row();

                ui.label("Tmpfs memory");
                ui.add(egui::Slider::new(&mut state.scratchpad_mb, 64..=4096).suffix(" MB"));
                ui.end_row();
            });
        ui.add_space(16.0);

        if components::action_button(ui, "Launch agent sandbox", true).clicked() {
            let domain_list: Vec<String> = state
                .allowed_domains
                .split(',')
                .map(|domain| domain.trim().to_string())
                .filter(|domain| !domain.is_empty())
                .collect();

            let bytes = state.scratchpad_mb * 1024 * 1024;

            match AgentSandbox::new(&state.agent_id, bytes, domain_list) {
                Ok(sandbox) => {
                    *status_message = format!(
                        "Agent '{}' spawned. Tmpfs scratchpad at {:?}",
                        state.agent_id,
                        sandbox.scratchpad_path()
                    );
                    state.active_agents.push(state.agent_id.clone());
                }
                Err(error) => {
                    *status_message = format!("Agent Launch Failed: {error}");
                }
            }
        }
    });

    ui.add_space(16.0);
    components::section(ui, |ui| {
        components::section_header(ui, "Active sandboxes");
        if state.active_agents.is_empty() {
            ui.label(
                egui::RichText::new("No agent sandboxes are currently active.")
                    .color(ui.visuals().widgets.noninteractive.fg_stroke.color),
            );
        } else {
            for id in &state.active_agents {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(id).strong());
                    ui.colored_label(
                        egui::Color32::GREEN,
                        "Protected · tmpfs and domain allowlist",
                    );
                });
            }
        }
    });
}
