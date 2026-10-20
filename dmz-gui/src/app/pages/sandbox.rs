use eframe::egui;
use std::path::PathBuf;

use super::super::components;
use dmz_core::diagnostics::DiagnosticError;
use dmz_sandbox::platform::{run_sandboxed, SandboxConfig};

pub(super) struct SandboxState {
    closure_path: String,
    pub(super) sandboxed: bool,
    pub(super) allow_network: bool,
    pub(super) diagnose: bool,
    command: String,
    console_output: String,
}

impl Default for SandboxState {
    fn default() -> Self {
        Self {
            closure_path: String::from(".dmz/store"),
            sandboxed: true,
            allow_network: false,
            diagnose: true,
            command: String::from("cargo check --offline"),
            console_output: String::from(
                "DMZ Workspace Sandbox ready.\nConfigure isolation toggles, workspace directory, and execute commands below.\n",
            ),
        }
    }
}

pub(super) fn show(
    state: &mut SandboxState,
    ui: &mut egui::Ui,
    workspace_path: &mut String,
    status_message: &mut String,
) {
    components::page_header(
        ui,
        "Workspace Sandbox",
        "Execute build, test, and shell commands in a daemonless process sandbox with zero host leakage.",
    );

    components::section(ui, |ui| {
        components::section_header(ui, "Sandbox Environment");
        egui::Grid::new("sandbox_env_grid")
            .num_columns(2)
            .spacing([16.0, 8.0])
            .min_col_width(120.0)
            .show(ui, |ui| {
                ui.label("Workspace directory");
                ui.horizontal(|ui| {
                    let browse_width = 86.0;
                    let field_width = (ui.available_width() - browse_width - 8.0).max(80.0);
                    ui.add_sized(
                        egui::vec2(field_width, 32.0),
                        egui::TextEdit::singleline(workspace_path)
                            .vertical_align(egui::Align::Center)
                            .margin(egui::Margin::symmetric(8, 0)),
                    );
                    if ui.button("Browse...").clicked() {
                        if let Some(path) = rfd::FileDialog::new().pick_folder() {
                            *workspace_path = path.display().to_string();
                        }
                    }
                });
                ui.end_row();

                ui.label("Closure store directory");
                ui.horizontal(|ui| {
                    let browse_width = 86.0;
                    let field_width = (ui.available_width() - browse_width - 8.0).max(80.0);
                    ui.add_sized(
                        egui::vec2(field_width, 32.0),
                        egui::TextEdit::singleline(&mut state.closure_path)
                            .vertical_align(egui::Align::Center)
                            .margin(egui::Margin::symmetric(8, 0)),
                    );
                    if ui.button("Browse...").clicked() {
                        if let Some(path) = rfd::FileDialog::new().pick_folder() {
                            state.closure_path = path.display().to_string();
                        }
                    }
                });
                ui.end_row();
            });

        ui.add_space(16.0);
        components::section_header(ui, "Isolation & Security Toggles");

        ui.horizontal(|ui| {
            components::toggle(ui, &mut state.sandboxed);
            ui.add_space(8.0);
            ui.vertical(|ui| {
                ui.label(egui::RichText::new("Sandbox Process Isolation").strong());
                ui.label(
                    egui::RichText::new(
                        "Enforces kernel-level process isolation (macOS Seatbelt, Linux Namespaces, Windows Job Objects)",
                    )
                    .color(ui.visuals().widgets.noninteractive.fg_stroke.color)
                    .size(12.0),
                );
            });
        });

        ui.add_space(12.0);
        ui.horizontal(|ui| {
            components::toggle(ui, &mut state.allow_network);
            ui.add_space(8.0);
            ui.vertical(|ui| {
                ui.label(egui::RichText::new("Outbound Network Access").strong());
                ui.label(
                    egui::RichText::new(
                        "Permits outbound network traffic (disabled by default for air-gap hermeticity)",
                    )
                    .color(ui.visuals().widgets.noninteractive.fg_stroke.color)
                    .size(12.0),
                );
            });
        });

        ui.add_space(12.0);
        ui.horizontal(|ui| {
            components::toggle(ui, &mut state.diagnose);
            ui.add_space(8.0);
            ui.vertical(|ui| {
                ui.label(egui::RichText::new("Automated Error Diagnostics").strong());
                ui.label(
                    egui::RichText::new(
                        "Classifies sandbox failures as Platform Fault vs User Code Fault with remediation advice",
                    )
                    .color(ui.visuals().widgets.noninteractive.fg_stroke.color)
                    .size(12.0),
                );
            });
        });
    });

    ui.add_space(16.0);
    components::section(ui, |ui| {
        components::section_header(ui, "Command Runner");
        egui::Grid::new("sandbox_cmd_grid")
            .num_columns(2)
            .spacing([16.0, 8.0])
            .min_col_width(120.0)
            .show(ui, |ui| {
                ui.label("Command to execute");
                ui.add_sized(
                    egui::vec2(ui.available_width(), 32.0),
                    egui::TextEdit::singleline(&mut state.command)
                        .vertical_align(egui::Align::Center)
                        .margin(egui::Margin::symmetric(8, 0))
                        .hint_text("cargo test --offline, npm run build, etc."),
                );
                ui.end_row();
            });

        ui.add_space(16.0);
        ui.horizontal_wrapped(|ui| {
            if components::action_button(ui, "Execute command in sandbox", true).clicked() {
                let workspace = PathBuf::from(&*workspace_path);
                let abs_workspace =
                    std::fs::canonicalize(&workspace).unwrap_or_else(|_| workspace.clone());
                let store = PathBuf::from(&state.closure_path);

                let start_time = std::time::Instant::now();
                state.console_output.push_str(&format!(
                    "\n>>> [{}] Executing: {} (sandboxed: {}, network: {})\n",
                    chrono_timestamp(),
                    state.command,
                    state.sandboxed,
                    state.allow_network
                ));

                let mut envs: std::collections::HashMap<String, String> =
                    std::env::vars().collect();
                envs.insert(
                    "DMZ_WORKSPACE".to_string(),
                    abs_workspace.display().to_string(),
                );
                envs.insert("DMZ_STORE".to_string(), store.display().to_string());
                envs.insert(
                    "CARGO_HOME".to_string(),
                    abs_workspace.join(".cargo").display().to_string(),
                );
                envs.insert(
                    "NODE_PATH".to_string(),
                    abs_workspace.join("node_modules").display().to_string(),
                );
                envs.insert(
                    "DMZ_SANDBOX".to_string(),
                    if state.sandboxed {
                        "1".to_string()
                    } else {
                        "0".to_string()
                    },
                );

                let env_vec: Vec<(String, String)> = envs.into_iter().collect();

                if state.sandboxed {
                    let config = SandboxConfig {
                        workspace_path: workspace,
                        closure_path: store,
                        command: "/bin/sh".to_string(),
                        args: vec!["-c".to_string(), state.command.clone()],
                        envs: env_vec,
                        allow_network: state.allow_network,
                    };

                    match run_sandboxed(&config) {
                        Ok(status) => {
                            let elapsed = start_time.elapsed();
                            let code = status.code().unwrap_or(-1);
                            if status.success() {
                                state.console_output.push_str(&format!(
                                    "✓ [SUCCESS] Process finished in {:.2?} with exit code {}\n",
                                    elapsed, code
                                ));
                                *status_message =
                                    format!("Sandbox process finished (exit code {})", code);
                            } else {
                                state.console_output.push_str(&format!(
                                    "✗ [PROCESS FAILED] Finished in {:.2?} with exit code {}\n",
                                    elapsed, code
                                ));
                                *status_message =
                                    format!("Sandbox process exited with code {}", code);
                            }
                        }
                        Err(e) => {
                            let elapsed = start_time.elapsed();
                            state.console_output.push_str(&format!(
                                "✗ [PLATFORM ERROR] Failed after {:.2?}: {}\n",
                                elapsed, e
                            ));
                            if state.diagnose {
                                let diag = DiagnosticError::classify(&e);
                                state.console_output.push_str(&format!(
                                    "\n[DIAGNOSTIC CLASSIFICATION]: {:?}\nMessage: {}\nRemediation: {}\n",
                                    diag.category, diag.message, diag.remediation
                                ));
                            }
                            *status_message = format!("Sandbox Execution Error: {}", e);
                        }
                    }
                } else {
                    let mut cmd = std::process::Command::new("/bin/sh");
                    cmd.args(["-c", &state.command])
                        .current_dir(&workspace)
                        .envs(env_vec);

                    match cmd.status() {
                        Ok(status) => {
                            let elapsed = start_time.elapsed();
                            let code = status.code().unwrap_or(-1);
                            state.console_output.push_str(&format!(
                                "✓ [HOST UNCONFINED] Finished in {:.2?} with exit code {}\n",
                                elapsed, code
                            ));
                            *status_message =
                                format!("Host command completed (exit code {})", code);
                        }
                        Err(e) => {
                            state
                                .console_output
                                .push_str(&format!("✗ [ERROR] Host execution failed: {}\n", e));
                            *status_message = format!("Command Execution Error: {}", e);
                        }
                    }
                }
            }

            if components::action_button(ui, "Launch interactive shell", false).clicked() {
                launch_external_terminal(workspace_path, state.sandboxed, state.allow_network);
                *status_message =
                    String::from("Launched interactive terminal shell session for workspace.");
            }

            if components::action_button(ui, "Clear console", false).clicked() {
                state.console_output.clear();
            }
        });
    });

    ui.add_space(16.0);
    components::section(ui, |ui| {
        components::section_header(ui, "Execution Console & Output");
        egui::ScrollArea::vertical()
            .max_height(200.0)
            .min_scrolled_height(100.0)
            .stick_to_bottom(true)
            .show(ui, |ui| {
                ui.add(
                    egui::TextEdit::multiline(&mut state.console_output)
                        .font(egui::TextStyle::Monospace)
                        .desired_width(f32::INFINITY)
                        .desired_rows(8)
                        .lock_focus(true),
                );
            });
    });
}

fn chrono_timestamp() -> String {
    let now = std::time::SystemTime::now();
    let duration = now
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = duration.as_secs() % 86400;
    let hours = secs / 3600;
    let mins = (secs % 3600) / 60;
    let s = secs % 60;
    format!("{:02}:{:02}:{:02}", hours, mins, s)
}

fn launch_external_terminal(workspace_path: &str, sandboxed: bool, allow_network: bool) {
    let abs_workspace =
        std::fs::canonicalize(workspace_path).unwrap_or_else(|_| PathBuf::from(workspace_path));

    #[cfg(target_os = "macos")]
    {
        let extra_flags = if !sandboxed {
            "--no-sandbox"
        } else if allow_network {
            "--allow-network"
        } else {
            ""
        };
        let shell_cmd = format!(
            "cd '{}' && cargo run -p dmz-cli -- shell {}",
            abs_workspace.display(),
            extra_flags
        );
        let _ = std::process::Command::new("osascript")
            .arg("-e")
            .arg(format!(
                "tell application \"Terminal\" to do script \"{}\"",
                shell_cmd
            ))
            .spawn();
    }

    #[cfg(target_os = "linux")]
    {
        let extra_flags = if !sandboxed {
            "--no-sandbox"
        } else if allow_network {
            "--allow-network"
        } else {
            ""
        };
        let _ = std::process::Command::new("x-terminal-emulator")
            .arg("-e")
            .arg(format!(
                "cargo run -p dmz-cli -- shell -w '{}' {}",
                abs_workspace.display(),
                extra_flags
            ))
            .spawn();
    }

    #[cfg(target_os = "windows")]
    {
        let _ = std::process::Command::new("cmd.exe")
            .arg("/C")
            .arg(format!(
                "start cmd /K cargo run -p dmz-cli -- shell -w \"{}\"",
                abs_workspace.display()
            ))
            .spawn();
    }
}
