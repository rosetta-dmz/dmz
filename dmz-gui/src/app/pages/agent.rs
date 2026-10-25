use eframe::egui;

use super::super::components;
use dmz_sandbox::agent::AgentSandbox;

pub(super) struct AgentState {
    agent_id: String,
    allowed_domains: String,
    scratchpad_mb: usize,
    active_agents: Vec<AgentSandbox>,
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
    components::page_header(ui, "Agent Sandbox");

    components::section(ui, |ui| {
        components::section_header(ui, "Sandbox Configuration");
        egui::Grid::new("agent_grid")
            .num_columns(2)
            .spacing([16.0, 8.0])
            .min_col_width(100.0)
            .show(ui, |ui| {
                ui.label("Agent Identifier");
                ui.add_sized(
                    egui::vec2(ui.available_width(), 32.0),
                    egui::TextEdit::singleline(&mut state.agent_id)
                        .vertical_align(egui::Align::Center)
                        .margin(egui::Margin::symmetric(8, 0)),
                );
                ui.end_row();

                ui.label("Allowed Domains");
                ui.add_sized(
                    egui::vec2(ui.available_width(), 32.0),
                    egui::TextEdit::singleline(&mut state.allowed_domains)
                        .vertical_align(egui::Align::Center)
                        .margin(egui::Margin::symmetric(8, 0))
                        .hint_text("api.example.com, *.example.org"),
                );
                ui.end_row();

                ui.label("Tmpfs Memory");
                ui.add(egui::Slider::new(&mut state.scratchpad_mb, 64..=4096).suffix(" MB"));
                ui.end_row();
            });
        ui.add_space(16.0);

        if components::action_button(ui, "Launch Agent Sandbox", true).clicked() {
            let id = state.agent_id.trim();
            if id.is_empty() {
                *status_message = String::from("Agent Identifier cannot be empty.");
            } else if state.active_agents.iter().any(|a| a.id == id) {
                *status_message = format!("Agent sandbox '{id}' is already running.");
            } else {
                let domain_list: Vec<String> = state
                    .allowed_domains
                    .split(',')
                    .map(|domain| domain.trim().to_string())
                    .filter(|domain| !domain.is_empty())
                    .collect();

                let bytes = state.scratchpad_mb * 1024 * 1024;

                match AgentSandbox::new(id, bytes, domain_list) {
                    Ok(sandbox) => {
                        *status_message = format!(
                            "Agent '{}' spawned. Tmpfs scratchpad mounted at {:?}",
                            id,
                            sandbox.scratchpad_path()
                        );
                        state.active_agents.push(sandbox);
                    }
                    Err(error) => {
                        *status_message = format!("Agent Launch Failed: {error}");
                    }
                }
            }
        }
    });

    ui.add_space(16.0);
    components::section(ui, |ui| {
        components::section_header(ui, "Active Sandboxes");
        if state.active_agents.is_empty() {
            ui.label(
                egui::RichText::new("No agent sandboxes are currently active.")
                    .color(ui.visuals().widgets.noninteractive.fg_stroke.color),
            );
        } else {
            let mut to_stop = None;

            for (idx, sandbox) in state.active_agents.iter().enumerate() {
                components::card(ui, false, |ui| {
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new(&sandbox.id).strong());
                                ui.colored_label(
                                    egui::Color32::GREEN,
                                    "Protected · tmpfs and domain allowlist",
                                );
                            });
                            ui.add_space(4.0);
                            ui.label(
                                egui::RichText::new(format!(
                                    "Scratchpad: {} ({} MB)",
                                    sandbox.scratchpad_path().display(),
                                    sandbox.scratchpad.size_limit_bytes / (1024 * 1024)
                                ))
                                .size(11.0)
                                .color(ui.visuals().widgets.noninteractive.fg_stroke.color),
                            );
                        });

                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if components::action_button(ui, "Stop sandbox", false).clicked() {
                                to_stop = Some(idx);
                            }
                        });
                    });
                });
                ui.add_space(8.0);
            }

            if let Some(idx) = to_stop {
                let stopped = state.active_agents.remove(idx);
                *status_message = format!(
                    "Agent '{}' stopped. Scratchpad unmounted and wiped.",
                    stopped.id
                );
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_agent_sandbox_stop_lifecycle_cleans_scratchpad() {
        let mut state = AgentState::default();
        let agent_id = "test-agent-e2e";
        let bytes = 64 * 1024 * 1024;
        let domains = vec!["api.openai.com".to_string()];

        let sandbox =
            AgentSandbox::new(agent_id, bytes, domains).expect("failed to spawn test sandbox");
        let scratchpad_path = sandbox.scratchpad_path().to_path_buf();

        assert!(
            scratchpad_path.exists(),
            "scratchpad directory should exist while sandbox is active"
        );
        state.active_agents.push(sandbox);
        assert_eq!(state.active_agents.len(), 1);

        // Stop agent (remove from active_agents and drop)
        let stopped = state.active_agents.remove(0);
        assert_eq!(stopped.id, agent_id);
        drop(stopped);

        assert!(
            !scratchpad_path.exists(),
            "scratchpad directory should be wiped on drop"
        );
        assert!(state.active_agents.is_empty());
    }
}
