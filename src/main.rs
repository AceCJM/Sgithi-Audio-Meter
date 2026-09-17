mod model;
mod pw;
mod tray;
mod ui;

fn main() -> glib::ExitCode {
    env_logger::init();
    ui::app::run()
}
