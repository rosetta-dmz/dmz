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

    info!("Starting DMZ GUI application package...");

    let viewport_builder = egui::ViewportBuilder::default()
        .with_inner_size([960.0, 720.0])
        .with_min_inner_size([800.0, 500.0])
        .with_title("DMZ")
        .with_active(true)
        .with_visible(true);

    // macOS handles app icons natively via the .app bundle (Info.plist -> AppIcon.icns).
    // Window-level icons are used primarily for Windows and Linux.
    #[cfg(not(target_os = "macos"))]
    {
        let icon_data = {
            let image = image::load_from_memory(include_bytes!("../../assets/dmz-icon.png"))
                .expect("Failed to load dmz-icon.png from workspace root assets directory")
                .into_rgba8();
            let (width, height) = image.dimensions();
            egui::IconData {
                rgba: image.into_raw(),
                width,
                height,
            }
        };
        viewport_builder = viewport_builder.with_icon(std::sync::Arc::new(icon_data));
    }

    let options = eframe::NativeOptions {
        viewport: viewport_builder,
        ..Default::default()
    };

    eframe::run_native(
        "DMZ",
        options,
        Box::new(|_cc| Ok(Box::new(DmzGuiApp::default()))),
    )
}
