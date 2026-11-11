#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod cast;
mod icons;
mod live;
#[cfg(target_os = "macos")]
mod macos_open;
mod models;
mod mpv;
mod render;
mod settings;
mod shots;
mod single;
mod thumbs;

use eframe::egui;

pub const APP_NAME: &str = "OK Player";

fn main() -> eframe::Result<()> {
    let file_arg = std::env::args().nth(1);
    if settings::Settings::load().single_instance && single::forward(file_arg.as_deref()) {
        return Ok(());
    }

    #[cfg(target_os = "macos")]
    macos_open::install();

    let mut viewport = egui::ViewportBuilder::default()
        .with_title(APP_NAME)
        .with_inner_size([1280.0, 760.0])
        .with_min_inner_size([480.0, 270.0])
        .with_drag_and_drop(true);
    #[cfg(target_os = "macos")]
    {
        // Native traffic lights over our own dark title bar.
        viewport = viewport.with_fullsize_content_view(true).with_title_shown(false).with_titlebar_shown(false);
    }
    #[cfg(not(target_os = "macos"))]
    {
        viewport = viewport.with_decorations(false);
    }

    let options = eframe::NativeOptions {
        viewport,
        vsync: true,
        multisampling: 0,
        depth_buffer: 0,
        stencil_buffer: 0,
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };
    eframe::run_native(
        APP_NAME,
        options,
        Box::new(move |cc| {
            icons::install_fonts(&cc.egui_ctx);
            app::style(&cc.egui_ctx);
            Ok(Box::new(app::PlayerApp::new(cc, file_arg)))
        }),
    )
}
