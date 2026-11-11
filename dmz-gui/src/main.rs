use eframe::egui;
use tracing::info;
use tracing_subscriber::FmtSubscriber;

mod app;
use app::DmzGuiApp;

fn main() -> eframe::Result<()> {
    let subscriber = FmtSubscriber::builder()
        .with_max_level(tracing::Level::INFO)
        .finish();
    let _ = tracing::subscriber::set_global_default(subscriber);

    info!("Starting DMZ Desktop...");

    let viewport_builder = egui::ViewportBuilder::default()
        .with_inner_size([960.0, 720.0])
        .with_min_inner_size([800.0, 500.0])
        .with_title("DMZ")
        .with_active(true)
        .with_visible(true);

    #[cfg(not(target_os = "macos"))]
    let viewport_builder = {
        match image::load_from_memory(include_bytes!("../../assets/dmz-icon.png")) {
            Ok(loaded) => {
                let image = loaded.into_rgba8();
                let (width, height) = image.dimensions();
                let icon_data = egui::IconData {
                    rgba: image.into_raw(),
                    width,
                    height,
                };
                viewport_builder.with_icon(std::sync::Arc::new(icon_data))
            }
            Err(e) => {
                tracing::warn!("Failed to load embedded icon: {}", e);
                viewport_builder
            }
        }
    };

    let options = eframe::NativeOptions {
        viewport: viewport_builder,
        ..Default::default()
    };

    eframe::run_native(
        "DMZ",
        options,
        Box::new(|cc| Ok(Box::new(DmzGuiApp::new(cc)))),
    )
}
