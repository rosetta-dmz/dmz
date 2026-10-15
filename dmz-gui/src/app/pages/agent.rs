use eframe::egui;

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
    ui.heading("Autonomous Agent Sub-Sandbox");
    ui.label("Configure volatile in-memory scratchpads and egress domain filter rules for agents.");
    ui.add_space(10.0);

    egui::Grid::new("agent_grid")
        .num_columns(2)
        .spacing([10.0, 10.0])
        .show(ui, |ui| {
            ui.label("Agent Identifier:");
            ui.text_edit_singleline(&mut state.agent_id);
            ui.end_row();

            ui.label("Allowed Domains (comma separated):");
            ui.text_edit_singleline(&mut state.allowed_domains);
            ui.end_row();

            ui.label("Tmpfs Memory Limit (MB):");
            ui.add(egui::Slider::new(&mut state.scratchpad_mb, 64..=4096));
            ui.end_row();
        });

    ui.add_space(15.0);

    if ui.button("Launch Agent Sub-Sandbox").clicked() {
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

    ui.add_space(20.0);
    ui.separator();
    ui.label("Active Agent Sub-Sandboxes:");

    if state.active_agents.is_empty() {
        ui.label("(No agents currently running)");
    } else {
        for id in &state.active_agents {
            ui.horizontal(|ui| {
                ui.label(format!("• {id}"));
                ui.colored_label(
                    egui::Color32::GREEN,
                    "[PROTECTED: tmpfs + domain-whitelisted]",
                );
            });
        }
    }
}
