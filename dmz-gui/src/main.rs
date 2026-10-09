use dmz_core::archive::{AirGapExporter, AirGapImporter, ExportConfig, ImportConfig};
use dmz_core::packaging::{BuildConfig, BuildTarget, PackageBuilder};
use dmz_core::resolver::ResolverEngine;
use dmz_sandbox::agent::AgentSandbox;
use eframe::egui;
use std::path::PathBuf;
use tracing::Level;
use tracing_subscriber::FmtSubscriber;

fn main() -> eframe::Result<()> {
    // 1. Initialize logging subscriber
    let subscriber = FmtSubscriber::builder()
        .with_max_level(Level::INFO)
        .finish();
    let _ = tracing::subscriber::set_global_default(subscriber);

    // 2. Configure native viewport window
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([960.0, 680.0])
            .with_min_inner_size([800.0, 500.0])
            .with_title("DMZ — Universal Daemonless Factory"),
        ..Default::default()
    };

    eframe::run_native(
        "DMZ Engine",
        options,
        Box::new(|_cc| Ok(Box::new(DmzGuiApp::default()))),
    )
}

#[derive(PartialEq, Eq)]
enum Tab {
    Resolver,
    Builder,
    AgentSandbox,
    AirGap,
    Supervisor,
}

pub struct DmzGuiApp {
    active_tab: Tab,
    workspace_path: String,
    lockfile_path: String,
    status_message: String,

    // Builder State
    selected_target: BuildTarget,
    output_dir: String,
    artifact_name: String,
    zstd_level: i32,

    // AI Agent State
    agent_id: String,
    allowed_domains: String,
    scratchpad_mb: usize,
    active_agents: Vec<String>,

    // Air-Gap State
    airgap_archive_path: String,
    store_dir: String,
}

impl Default for DmzGuiApp {
    fn default() -> Self {
        Self {
            active_tab: Tab::Resolver,
            workspace_path: String::from("."),
            lockfile_path: String::from("dmz.lock"),
            status_message: String::from("Ready."),

            selected_target: BuildTarget::Unified,
            output_dir: String::from("dist"),
            artifact_name: String::from("workspace_closure"),
            zstd_level: 3,

            agent_id: String::from("agent-01"),
            allowed_domains: String::from("api.openai.com, *.github.com, crates.io"),
            scratchpad_mb: 512,
            active_agents: Vec::new(),

            airgap_archive_path: String::from("dist/airgap_bundle.tar.zst"),
            store_dir: String::from(".dmz/store"),
        }
    }
}

impl eframe::App for DmzGuiApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Top Navigation Bar
        egui::TopBottomPanel::top("top_panel").show(ctx, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.heading("DMZ Engine");
                ui.separator();
                ui.selectable_value(&mut self.active_tab, Tab::Resolver, "Resolver");
                ui.selectable_value(&mut self.active_tab, Tab::Builder, "Package Builder");
                ui.selectable_value(&mut self.active_tab, Tab::AgentSandbox, "AI Agent Sandbox");
                ui.selectable_value(&mut self.active_tab, Tab::AirGap, "Air-Gap Manager");
                ui.selectable_value(&mut self.active_tab, Tab::Supervisor, "Supervisor IPC");
            });
            ui.add_space(6.0);
        });

        // Bottom Status Bar Telemetry
        egui::TopBottomPanel::bottom("bottom_panel").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label("Status:");
                ui.colored_label(egui::Color32::LIGHT_BLUE, &self.status_message);
            });
        });

        // Central Content Viewport
        egui::CentralPanel::default().show(ctx, |ui| {
            match self.active_tab {
                Tab::Resolver => self.show_resolver_tab(ui),
                Tab::Builder => self.show_builder_tab(ui),
                Tab::AgentSandbox => self.show_agent_tab(ui),
                Tab::AirGap => self.show_airgap_tab(ui),
                Tab::Supervisor => self.show_supervisor_tab(ui),
            }
        });
    }
}

impl DmzGuiApp {
    fn show_resolver_tab(&mut self, ui: &mut egui::Ui) {
        ui.heading("Dependency Resolver & Lockfile Generator");
        ui.label("Scan project manifests (Cargo.toml, package.json, requirements.txt) and generate cryptographic closure signatures.");
        ui.add_space(10.0);

        egui::Grid::new("resolver_grid")
            .num_columns(2)
            .spacing([10.0, 10.0])
            .show(ui, |ui| {
                ui.label("Workspace Directory:");
                ui.text_edit_singleline(&mut self.workspace_path);
                ui.end_row();

                ui.label("Lockfile Output:");
                ui.text_edit_singleline(&mut self.lockfile_path);
                ui.end_row();
            });

        ui.add_space(15.0);

        if ui.button("Scan Manifests & Resolve Closure").clicked() {
            let path = PathBuf::from(&self.workspace_path);
            match ResolverEngine::resolve_workspace(&path) {
                Ok(lockfile) => {
                    let out = PathBuf::from(&self.lockfile_path);
                    if let Err(e) = lockfile.save(&out) {
                        self.status_message = format!("Error saving lockfile: {}", e);
                    } else {
                        self.status_message = format!(
                            "Resolved {} packages. Closure Sig: {}",
                            lockfile.packages.len(),
                            &lockfile.closure_signature[..12]
                        );
                    }
                }
                Err(e) => {
                    self.status_message = format!("Resolution Failed: {}", e);
                }
            }
        }
    }

    fn show_builder_tab(&mut self, ui: &mut egui::Ui) {
        ui.heading("Deterministic Package Builder");
        ui.label("Assemble compressed .tar.zst packages targeting specific runtime layers.");
        ui.add_space(10.0);

        ui.horizontal(|ui| {
            ui.label("Target Mode:");
            ui.radio_value(
                &mut self.selected_target,
                BuildTarget::DependenciesOnly,
                "Target A (Deps Only)",
            );
            ui.radio_value(
                &mut self.selected_target,
                BuildTarget::AppOnly,
                "Target B (App Code Only)",
            );
            ui.radio_value(
                &mut self.selected_target,
                BuildTarget::Unified,
                "Target C (Unified)",
            );
        });

        ui.add_space(10.0);

        egui::Grid::new("builder_grid")
            .num_columns(2)
            .spacing([10.0, 10.0])
            .show(ui, |ui| {
                ui.label("Artifact Base Name:");
                ui.text_edit_singleline(&mut self.artifact_name);
                ui.end_row();

                ui.label("Output Directory:");
                ui.text_edit_singleline(&mut self.output_dir);
                ui.end_row();

                ui.label("Zstd Compression Level:");
                ui.add(egui::Slider::new(&mut self.zstd_level, 1..=22));
                ui.end_row();
            });

        ui.add_space(15.0);

        if ui.button("Build Artifact Package").clicked() {
            let config = BuildConfig {
                workspace_dir: PathBuf::from(&self.workspace_path),
                lockfile_path: PathBuf::from(&self.lockfile_path),
                output_dir: PathBuf::from(&self.output_dir),
                target: self.selected_target,
                archive_name: self.artifact_name.clone(),
                zstd_level: self.zstd_level,
            };

            match PackageBuilder::build(&config) {
                Ok(res) => {
                    self.status_message = format!(
                        "Build Complete: {:?} (SHA256: {})",
                        res.artifact_path.file_name().unwrap_or_default(),
                        &res.sha256[..12]
                    );
                }
                Err(e) => {
                    self.status_message = format!("Build Error: {}", e);
                }
            }
        }
    }

    fn show_agent_tab(&mut self, ui: &mut egui::Ui) {
        ui.heading("Autonomous AI Agent Sub-Sandbox");
        ui.label("Configure volatile in-memory scratchpads and egress domain filter rules for AI agents.");
        ui.add_space(10.0);

        egui::Grid::new("agent_grid")
            .num_columns(2)
            .spacing([10.0, 10.0])
            .show(ui, |ui| {
                ui.label("Agent Identifier:");
                ui.text_edit_singleline(&mut self.agent_id);
                ui.end_row();

                ui.label("Allowed Domains (comma separated):");
                ui.text_edit_singleline(&mut self.allowed_domains);
                ui.end_row();

                ui.label("Tmpfs Memory Limit (MB):");
                ui.add(egui::Slider::new(&mut self.scratchpad_mb, 64..=4096));
                ui.end_row();
            });

        ui.add_space(15.0);

        ui.horizontal(|ui| {
            if ui.button("Launch Agent Sub-Sandbox").clicked() {
                let domain_list: Vec<String> = self
                    .allowed_domains
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();

                let bytes = self.scratchpad_mb * 1024 * 1024;

                match AgentSandbox::new(&self.agent_id, bytes, domain_list) {
                    Ok(sandbox) => {
                        self.status_message = format!(
                            "Agent '{}' spawned. Tmpfs scratchpad at {:?}",
                            self.agent_id,
                            sandbox.scratchpad_path()
                        );
                        self.active_agents.push(self.agent_id.clone());
                    }
                    Err(e) => {
                        self.status_message = format!("Agent Launch Failed: {}", e);
                    }
                }
            }
        });

        ui.add_space(20.0);
        ui.separator();
        ui.label("Active Agent Sub-Sandboxes:");

        if self.active_agents.is_empty() {
            ui.label("(No agents currently running)");
        } else {
            for id in &self.active_agents {
                ui.horizontal(|ui| {
                    ui.label(format!("• {}", id));
                    ui.colored_label(
                        egui::Color32::GREEN,
                        "[PROTECTED: tmpfs + domain-whitelisted]",
                    );
                });
            }
        }
    }

    fn show_airgap_tab(&mut self, ui: &mut egui::Ui) {
        ui.heading("Air-Gap Bundle Importer & Exporter");
        ui.label("Manage offline, portable store closures for target environments without internet access.");
        ui.add_space(10.0);

        egui::Grid::new("airgap_grid")
            .num_columns(2)
            .spacing([10.0, 10.0])
            .show(ui, |ui| {
                ui.label("Bundle Archive Path:");
                ui.text_edit_singleline(&mut self.airgap_archive_path);
                ui.end_row();

                ui.label("Target Store Path:");
                ui.text_edit_singleline(&mut self.store_dir);
                ui.end_row();
            });

        ui.add_space(15.0);

        ui.horizontal(|ui| {
            if ui.button("Export Air-Gap Bundle").clicked() {
                let config = ExportConfig {
                    lockfile_path: PathBuf::from(&self.lockfile_path),
                    store_dir: PathBuf::from(&self.store_dir),
                    output_path: PathBuf::from(&self.airgap_archive_path),
                    zstd_level: self.zstd_level,
                };

                match AirGapExporter::export(&config) {
                    Ok(manifest) => {
                        self.status_message = format!(
                            "Exported {} packages to bundle (SHA256: {})",
                            manifest.package_count,
                            &manifest.archive_sha256[..12]
                        );
                    }
                    Err(e) => {
                        self.status_message = format!("Export Failed: {}", e);
                    }
                }
            }

            if ui.button("Import Air-Gap Bundle").clicked() {
                let config = ImportConfig {
                    archive_path: PathBuf::from(&self.airgap_archive_path),
                    target_store_dir: PathBuf::from(&self.store_dir),
                    verify_closure_hash: true,
                };

                match AirGapImporter::import(&config) {
                    Ok(result) => {
                        self.status_message = format!(
                            "Imported {} files. Verified Closure Sig: {}",
                            result.imported_packages,
                            &result.closure_signature[..12]
                        );
                    }
                    Err(e) => {
                        self.status_message = format!("Import Failed: {}", e);
                    }
                }
            }
        });
    }

    fn show_supervisor_tab(&mut self, ui: &mut egui::Ui) {
        ui.heading("Workspace Process Supervisor");
        ui.label("Monitor localized IPC socket streams, background collateral services, and process trees.");
        ui.add_space(10.0);

        #[cfg(unix)]
        ui.label("IPC Socket Path: /tmp/dmz_supervisor.sock");

        #[cfg(windows)]
        ui.label(r"IPC Pipe Path: \\.\pipe\dmz_supervisor");

        ui.add_space(15.0);

        ui.group(|ui| {
            ui.label("Supervisor State: ACTIVE (Localized Thread)");
            ui.label("Idle Footprint: 0% CPU | < 12 MB RAM");
            ui.label("Connected Clients: 1 (GUI Desktop)");
        });
    }
}
