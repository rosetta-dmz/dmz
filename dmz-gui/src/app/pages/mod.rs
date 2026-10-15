mod agent;
mod airgap;
mod builder;
mod resolver;
mod settings;
mod supervisor;

use eframe::egui;

use self::{
    agent::AgentState, airgap::AirGapState, builder::BuilderState, settings::AppearanceState,
    supervisor::SupervisorState,
};
use crate::app::Tab;

#[derive(Default)]
pub(super) struct Pages {
    builder: BuilderState,
    agent: AgentState,
    airgap: AirGapState,
    supervisor: SupervisorState,
    appearance: AppearanceState,
}

impl Pages {
    pub(super) fn load_appearance(
        &mut self,
        storage: Option<&dyn eframe::Storage>,
        status_message: &mut String,
    ) {
        self.appearance = AppearanceState::load(storage, status_message);
    }

    pub(super) fn apply_appearance(&self, ctx: &egui::Context) -> Result<egui::Visuals, String> {
        self.appearance.apply(ctx)
    }

    pub(super) fn show_status_bar(&self) -> bool {
        self.appearance.show_status_bar()
    }

    pub(super) fn save(&self, storage: &mut dyn eframe::Storage) {
        self.appearance.save(storage);
    }

    pub(super) fn show_tab(
        &mut self,
        tab: Tab,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        workspace_path: &mut String,
        lockfile_path: &mut String,
        status_message: &mut String,
    ) {
        match tab {
            Tab::Resolver => resolver::show(ui, workspace_path, lockfile_path, status_message),
            Tab::Builder => builder::show(
                &mut self.builder,
                ui,
                workspace_path,
                lockfile_path,
                status_message,
            ),
            Tab::AgentSandbox => agent::show(&mut self.agent, ui, status_message),
            Tab::AirGap => airgap::show(
                &mut self.airgap,
                ui,
                workspace_path,
                lockfile_path,
                self.builder.zstd_level(),
                status_message,
            ),
            Tab::Supervisor => supervisor::show(&mut self.supervisor, ui, status_message),
            Tab::Settings => self.appearance.show(ui, ctx, status_message),
        }
    }
}
