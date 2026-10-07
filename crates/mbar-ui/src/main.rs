//! `mbar-ui`: management app for mbar (inspector, events, performance, system).
//!
//! The UI is an IPC client of the daemon. All IPC runs on GPUI's background executor or
//! on the dedicated `--monitor` thread; the UI thread never blocks on a socket.

mod views;

use gpui_kit::component::Theme;
use gpui_kit::*;
use mbar_ui_model::ipc::Client;
use mbar_ui_model::model::parse_cli_args;

actions!(mbar_ui, [Quit]);

fn main() {
    let args = match parse_cli_args(std::env::args().skip(1)) {
        Ok(a) => a,
        Err(msg) => {
            let help = msg.starts_with("usage");
            if help {
                println!("{msg}");
                return;
            }
            eprintln!("mbar-ui: {msg}");
            std::process::exit(2);
        }
    };
    let bar_name = args.bar_name.clone();

    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .run(move |cx| {
            gpui_kit::init(cx);

            cx.bind_keys([
                #[cfg(target_os = "macos")]
                KeyBinding::new("cmd-q", Quit, None),
                #[cfg(not(target_os = "macos"))]
                KeyBinding::new("ctrl-q", Quit, None),
            ]);
            cx.on_action(|_: &Quit, cx: &mut App| cx.quit());
            // One window; closing it ends the app.
            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();

            let options = WindowOptions {
                window_bounds: Some(WindowBounds::centered(size(px(1100.), px(720.)), cx)),
                titlebar: Some(TitlebarOptions {
                    title: Some(format!("mbar — {bar_name}").into()),
                    ..Default::default()
                }),
                ..Default::default()
            };
            let client = Client::new(bar_name.clone());
            gpui_kit::open_window(options, cx, |window, cx| {
                // Light/dark follows the system; AppView keeps it in sync afterwards.
                Theme::sync_system_appearance(Some(window), cx);
                cx.new(|cx| views::AppView::new(client, window, cx))
            })
            .expect("failed to open the mbar-ui window");
            cx.activate(true);
        });
}
