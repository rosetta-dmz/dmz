mod agent;
mod airgap;
mod resolver;
mod sandbox;
mod settings;
mod supervisor;
mod version_control;

use eframe::egui;

use std::path::{Path, PathBuf};

use self::{
    agent::AgentState, airgap::AirGapState, resolver::ResolverState, sandbox::SandboxState,
    settings::AppearanceState, supervisor::SupervisorState, version_control::VersionControlState,
};
use crate::app::Tab;

pub(super) struct WorkspaceSessionState {
    pub(super) is_dual_repo: bool,
    pub(super) sidecar_path: String,
    pub(super) sidecar_id: String,
    pub(super) sidecar_role: String,
    pub(super) include_dev: bool,
    pub(super) target_platform: String,
}

impl Default for WorkspaceSessionState {
    fn default() -> Self {
        Self {
            is_dual_repo: false,
            sidecar_path: String::new(),
            sidecar_id: String::from("sidecar"),
            sidecar_role: String::from("sidecar"),
            include_dev: true,
            target_platform: String::new(),
        }
    }
}

/// Builds a normalized [`dmz_core::workspace::WorkspaceProjectConfig`] for a dual-repo session.
pub(super) fn build_dual_repo_project_config(
    ws_canon: &Path,
    sidecar_path: &str,
    sidecar_id: &str,
    sidecar_role: &str,
) -> dmz_core::workspace::WorkspaceProjectConfig {
    let sc_path = PathBuf::from(sidecar_path);
    let sc_canon = std::fs::canonicalize(&sc_path).unwrap_or(sc_path);
    let primary_name = ws_canon
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("primary");
    let mut primary_m = dmz_core::workspace::WorkspaceMember::new(
        primary_name,
        Path::new("."),
        dmz_core::workspace::MemberRole::Primary,
    );
    primary_m.populate_detected_defaults(ws_canon, ws_canon);

    let sc_role = if sidecar_role.trim().eq_ignore_ascii_case("service") {
        dmz_core::workspace::MemberRole::Service
    } else if sidecar_role.trim().eq_ignore_ascii_case("sidecar") {
        dmz_core::workspace::MemberRole::Sidecar
    } else {
        dmz_core::workspace::MemberRole::Member
    };
    let normalized_sc_path = dmz_core::workspace::normalize_member_path(ws_canon, &sc_canon);
    let effective_id = if sidecar_id.trim().is_empty() {
        "sidecar"
    } else {
        sidecar_id.trim()
    };
    let mut sidecar_m =
        dmz_core::workspace::WorkspaceMember::new(effective_id, normalized_sc_path, sc_role);
    sidecar_m.populate_detected_defaults(&sc_canon, ws_canon);

    dmz_core::workspace::WorkspaceProjectConfig::new(primary_name, vec![primary_m, sidecar_m])
}

#[derive(Default)]
pub(super) struct Pages {
    session: WorkspaceSessionState,
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

    pub(super) fn is_busy(&self) -> bool {
        self.airgap.is_busy()
            || self.resolver.is_busy()
            || self.version_control.is_busy()
            || self.sandbox.is_busy()
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
                &mut self.session,
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
                &mut self.session,
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
            Tab::Supervisor => {
                supervisor::show(&mut self.supervisor, ui, status_message, workspace_path)
            }
            Tab::Settings => self.appearance.show(ui, ctx, status_message),
        }
    }
}
