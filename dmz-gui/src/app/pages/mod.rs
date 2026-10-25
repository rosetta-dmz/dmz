mod agent;
mod airgap;
mod resolver;
mod sandbox;
mod settings;
mod supervisor;
mod version_control;

use eframe::egui;

use self::{
    agent::AgentState, airgap::AirGapState, resolver::ResolverState, sandbox::SandboxState,
    settings::AppearanceState, supervisor::SupervisorState, version_control::VersionControlState,
};
use crate::app::Tab;

#[derive(Default)]
pub(super) struct Pages {
    resolver: ResolverState,
    sandbox: SandboxState,
    agent: AgentState,
    airgap: AirGapState,
    version_control: VersionControlState,
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
            Tab::Resolver => resolver::show(
                &mut self.resolver,
                ui,
                workspace_path,
                lockfile_path,
                status_message,
            ),
            Tab::WorkspaceSandbox => {
                sandbox::show(&mut self.sandbox, ui, workspace_path, status_message);
            }
            Tab::AgentSandbox => agent::show(&mut self.agent, ui, status_message),
            Tab::AirGap => airgap::show(
                &mut self.airgap,
                ui,
                workspace_path,
                lockfile_path,
                status_message,
            ),
            Tab::VersionControl => version_control::show(
                &mut self.version_control,
                ui,
                workspace_path,
                lockfile_path,
                status_message,
            ),
            Tab::Supervisor => supervisor::show(&mut self.supervisor, ui, status_message),
            Tab::Settings => self.appearance.show(ui, ctx, status_message),
        }
    }
}
