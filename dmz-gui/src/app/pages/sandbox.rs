use eframe::egui;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};
use std::thread;

use super::super::components;
use dmz_core::diagnostics::DiagnosticError;
use dmz_sandbox::platform::{run_sandboxed, SandboxConfig};

pub(super) struct SandboxTaskOutcome {
    output_line: String,
    status_message: String,
}

pub(super) struct SandboxState {
    closure_path: String,
    pub(super) sandboxed: bool,
    pub(super) allow_network: bool,
    pub(super) diagnose: bool,
    command: String,
    console_output: String,
    pub(super) active_task: Option<(String, Receiver<SandboxTaskOutcome>)>,
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
            active_task: None,
        }
    }
}

impl SandboxState {
    pub(super) fn is_busy(&self) -> bool {
        self.active_task.is_some()
    }
}

pub(super) fn show(
    state: &mut SandboxState,
    ui: &mut egui::Ui,
    workspace_path: &mut String,
    status_message: &mut String,
) {
    if let Some((ref label, ref rx)) = state.active_task {
        match rx.try_recv() {
            Ok(outcome) => {
                state.console_output.push_str(&outcome.output_line);
                *status_message = outcome.status_message;
                state.active_task = None;
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                ui.ctx().request_repaint();
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                *status_message = format!("Sandbox task '{label}' disconnected unexpectedly.");
                state.active_task = None;
            }
        }
    }

    components::page_header(ui, "Workspace Sandbox");

    components::section(ui, |ui| {
        components::section_header(ui, "Sandbox Environment");
        egui::Grid::new("sandbox_env_grid")
            .num_columns(2)
            .spacing([16.0, 8.0])
            .min_col_width(120.0)
            .show(ui, |ui| {
                ui.label("Workspace Directory");
                if components::path_browse_row(ui, workspace_path, None) {
                    if let Some(path) = rfd::FileDialog::new().pick_folder() {
                        *workspace_path = path.display().to_string();
                    }
                }
                ui.end_row();

                ui.label("Closure Store Directory");
                if components::path_browse_row(ui, &mut state.closure_path, None) {
                    if let Some(path) = rfd::FileDialog::new().pick_folder() {
                        state.closure_path = path.display().to_string();
                    }
                }
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
                ui.label("Command to Execute");
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
        let is_busy = state.is_busy();
        ui.horizontal_wrapped(|ui| {
            ui.add_enabled_ui(!is_busy, |ui| {
                if components::action_button(ui, "Execute Command in Sandbox", true).clicked() {
                    let workspace = PathBuf::from(&*workspace_path);
                    let abs_workspace =
                        std::fs::canonicalize(&workspace).unwrap_or_else(|_| workspace.clone());
                    let store = PathBuf::from(&state.closure_path);

                    let sandboxed = state.sandboxed;
                    let allow_network = state.allow_network;
                    let diagnose = state.diagnose;
                    let command = state.command.clone();

                    state.console_output.push_str(&format!(
                        "\n>>> [{}] Executing: {} (sandboxed: {}, network: {})\n",
                        chrono_timestamp(),
                        command,
                        sandboxed,
                        allow_network
                    ));

                    let mut envs =
                        dmz_core::workspace::WorkspaceProjectConfig::default().build_member_env(
                            &abs_workspace,
                            &abs_workspace,
                            Path::new(""),
                            allow_network,
                        );
                    envs.insert("DMZ_STORE".to_string(), store.display().to_string());
                    envs.insert(
                        "DMZ_SANDBOX".to_string(),
                        if sandboxed {
                            "1".to_string()
                        } else {
                            "0".to_string()
                        },
                    );

                    let env_vec: Vec<(String, String)> = envs.into_iter().collect();

                    let (shell_cmd, shell_arg) = if cfg!(windows) {
                        ("cmd.exe", "/C")
                    } else {
                        ("/bin/sh", "-c")
                    };

                    let (tx, rx) = channel();
                    let ctx = ui.ctx().clone();
                    state.active_task = Some(("Executing command...".to_string(), rx));

                    thread::spawn(move || {
                        let start_time = std::time::Instant::now();
                        if sandboxed {
                            let config = SandboxConfig {
                                workspace_path: workspace,
                                closure_path: store,
                                command: shell_cmd.to_string(),
                                args: vec![shell_arg.to_string(), command],
                                envs: env_vec,
                                allow_network,
                                ipc_socket_path: None,
                            };

                            let outcome = match run_sandboxed(&config) {
                                Ok(status) => {
                                    let elapsed = start_time.elapsed();
                                    let code = status.code().unwrap_or(-1);
                                    if status.success() {
                                        SandboxTaskOutcome {
                                            output_line: format!(
                                                "✓ [SUCCESS] Process finished in {:.2?} with exit code {}\n",
                                                elapsed, code
                                            ),
                                            status_message: format!(
                                                "Sandbox process finished (exit code {})",
                                                code
                                            ),
                                        }
                                    } else {
                                        SandboxTaskOutcome {
                                            output_line: format!(
                                                "✗ [PROCESS FAILED] Finished in {:.2?} with exit code {}\n",
                                                elapsed, code
                                            ),
                                            status_message: format!(
                                                "Sandbox process exited with code {}",
                                                code
                                            ),
                                        }
                                    }
                                }
                                Err(e) => {
                                    let elapsed = start_time.elapsed();
                                    let mut out = format!(
                                        "✗ [PLATFORM ERROR] Failed after {:.2?}: {}\n",
                                        elapsed, e
                                    );
                                    if diagnose {
                                        let diag = DiagnosticError::classify(&e);
                                        out.push_str(&format!(
                                            "\n[DIAGNOSTIC CLASSIFICATION]: {:?}\nMessage: {}\nRemediation: {}\n",
                                            diag.category, diag.message, diag.remediation
                                        ));
                                    }
                                    SandboxTaskOutcome {
                                        output_line: out,
                                        status_message: format!("Sandbox Execution Error: {}", e),
                                    }
                                }
                            };
                            let _ = tx.send(outcome);
                        } else {
                            let mut cmd = std::process::Command::new(shell_cmd);
                            cmd.args([shell_arg, &command])
                                .current_dir(&workspace)
                                .envs(env_vec);

                            let outcome = match cmd.status() {
                                Ok(status) => {
                                    let elapsed = start_time.elapsed();
                                    let code = status.code().unwrap_or(-1);
                                    SandboxTaskOutcome {
                                        output_line: format!(
                                            "✓ [HOST UNCONFINED] Finished in {:.2?} with exit code {}\n",
                                            elapsed, code
                                        ),
                                        status_message: format!(
                                            "Host command completed (exit code {})",
                                            code
                                        ),
                                    }
                                }
                                Err(e) => SandboxTaskOutcome {
                                    output_line: format!("✗ [ERROR] Host execution failed: {}\n", e),
                                    status_message: format!("Command Execution Error: {}", e),
                                },
                            };
                            let _ = tx.send(outcome);
                        }
                        ctx.request_repaint();
                    });
                }

                if components::action_button(ui, "Launch Interactive Shell", false).clicked() {
                    launch_external_terminal(workspace_path, state.sandboxed, state.allow_network);
                    *status_message =
                        String::from("Launched interactive terminal shell session for workspace.");
                }

                if components::action_button(ui, "Clear Console", false).clicked() {
                    state.console_output.clear();
                }
            });

            if let Some((ref label, _)) = state.active_task {
                ui.add_space(8.0);
                components::inline_spinner_status(ui, label);
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

fn resolve_dmz_cli_cmd(abs_workspace: &Path, extra_flags: &str) -> String {
    let current_exe = std::env::current_exe().ok();
    let dmz_bin_name = if cfg!(windows) { "dmz.exe" } else { "dmz" };
    let adjacent_dmz = current_exe
        .as_ref()
        .and_then(|p| p.parent())
        .map(|p| p.join(dmz_bin_name));

    let flags_str = if extra_flags.is_empty() {
        String::new()
    } else {
        format!(" {}", extra_flags)
    };

    if cfg!(windows) {
        if let Some(bin) = adjacent_dmz.filter(|p| p.exists()) {
            format!(
                "cd /d \"{}\" && \"{}\" shell{}",
                abs_workspace.display(),
                bin.display(),
                flags_str
            )
        } else if let Ok(path) = which::which("dmz") {
            format!(
                "cd /d \"{}\" && \"{}\" shell{}",
                abs_workspace.display(),
                path.display(),
                flags_str
            )
        } else if which::which("cargo").is_ok() && std::path::Path::new("Cargo.toml").exists() {
            format!(
                "cd /d \"{}\" && cargo run -p dmz-cli -- shell{}",
                abs_workspace.display(),
                flags_str
            )
        } else {
            format!("cd /d \"{}\"", abs_workspace.display())
        }
    } else if let Some(bin) = adjacent_dmz.filter(|p| p.exists()) {
        format!(
            "cd '{}' && '{}' shell{}",
            abs_workspace.display(),
            bin.display(),
            flags_str
        )
    } else if let Ok(path) = which::which("dmz") {
        format!(
            "cd '{}' && '{}' shell{}",
            abs_workspace.display(),
            path.display(),
            flags_str
        )
    } else if which::which("cargo").is_ok() && std::path::Path::new("Cargo.toml").exists() {
        format!(
            "cd '{}' && cargo run -p dmz-cli -- shell{}",
            abs_workspace.display(),
            flags_str
        )
    } else {
        format!("cd '{}' && exec $SHELL", abs_workspace.display())
    }
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
        let shell_cmd = resolve_dmz_cli_cmd(&abs_workspace, extra_flags);
        let escaped_cmd = shell_cmd.replace('\\', "\\\\").replace('"', "\\\"");
        let _ = std::process::Command::new("osascript")
            .arg("-e")
            .arg(format!(
                "tell application \"Terminal\" to do script \"{}\"",
                escaped_cmd
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
        let shell_cmd = resolve_dmz_cli_cmd(&abs_workspace, extra_flags);
        let terminals = [
            std::env::var("TERMINAL").ok(),
            Some("x-terminal-emulator".to_string()),
            Some("gnome-terminal".to_string()),
            Some("konsole".to_string()),
            Some("alacritty".to_string()),
            Some("kitty".to_string()),
            Some("xterm".to_string()),
        ];

        for term_opt in terminals.into_iter().flatten() {
            if which::which(&term_opt).is_ok() {
                let spawned = if term_opt == "gnome-terminal" {
                    std::process::Command::new(&term_opt)
                        .arg("--")
                        .arg("sh")
                        .arg("-c")
                        .arg(&shell_cmd)
                        .spawn()
                } else {
                    std::process::Command::new(&term_opt)
                        .arg("-e")
                        .arg(&shell_cmd)
                        .spawn()
                };
                if spawned.is_ok() {
                    break;
                }
            }
        }
    }

    #[cfg(target_os = "windows")]
    {
        let extra_flags = if !sandboxed {
            "--no-sandbox"
        } else if allow_network {
            "--allow-network"
        } else {
            ""
        };
        let shell_cmd = resolve_dmz_cli_cmd(&abs_workspace, extra_flags);
        let _ = std::process::Command::new("cmd.exe")
            .arg("/C")
            .arg(format!("start cmd /K \"{}\"", shell_cmd))
            .spawn();
    }
}
