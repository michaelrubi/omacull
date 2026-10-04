mod app;
mod commands;
mod hotkeys;
mod loupe;
mod monitor;
mod shoot;
mod state;
mod theme;

use std::path::PathBuf;

use eframe::egui_wgpu::WgpuSetup;
use eframe::wgpu::PowerPreference;

fn main() -> eframe::Result {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn,omacull=info"))
        .init();

    // `omacull <folder>`, or `omacull <raw>` to start at that frame.
    let open = std::env::args_os().nth(1).map(PathBuf::from);

    let mut options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_app_id("omacull")
            .with_title("Omacull")
            .with_inner_size([1400.0, 900.0])
            .with_min_inner_size([480.0, 320.0]),
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };
    // Showing previews is light work. Prefer the integrated GPU so opening
    // Omacull doesn't wake a laptop's discrete GPU and drain the battery.
    if let WgpuSetup::CreateNew(setup) = &mut options.wgpu_options.wgpu_setup {
        setup.power_preference = PowerPreference::LowPower;
    }

    eframe::run_native("omacull", options, Box::new(|cc| Ok(Box::new(app::App::new(cc, open)))))
}
