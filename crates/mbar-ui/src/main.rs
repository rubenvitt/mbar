//! `mbar-ui`: management app for mbar (inspector, events, performance, system).
//!
//! The UI is an IPC client of the daemon. All IPC runs on GPUI's background executor or
//! on the dedicated `--monitor` thread; the UI thread never blocks on a socket.

#[cfg(target_os = "macos")]
mod mac;
mod views;

use gpui_kit::component::Theme;
use gpui_kit::*;
use mbar_ui_model::ipc::Client;
use mbar_ui_model::model::parse_cli_args;

actions!(mbar_ui, [Quit, CheckForUpdates]);

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

            #[cfg(target_os = "macos")]
            {
                let update_mode = args.update;
                if update_mode {
                    // Only Sparkle's dialog: no Dock icon, no main window.
                    mac::app::set_accessory(true);
                }
                let updater = std::rc::Rc::new(mac::sparkle::Updater::start(move || {
                    if update_mode {
                        mac::app::terminate();
                    }
                }));
                if let Some(u) = updater.as_ref() {
                    // The daemon found an update while this window is open.
                    let shared = updater.clone();
                    mac::app::on_distributed(mbar_app::UPDATE_NOTIFICATION, move || {
                        if let Some(u) = shared.as_ref() {
                            u.check_in_background();
                        }
                    });
                    if update_mode {
                        u.check_in_background();
                    }
                }
                if update_mode {
                    if updater.is_none() {
                        std::process::exit(0);
                    }
                    cx.set_global(mac::UpdaterGlobal(updater));
                    return;
                }
                let shared = updater.clone();
                cx.on_action(move |_: &CheckForUpdates, _cx: &mut App| {
                    if let Some(u) = shared.as_ref() {
                        u.check_now();
                    }
                });
                cx.set_menus([Menu::new("mbar").items([
                    MenuItem::action("Check for Updates…", CheckForUpdates),
                    MenuItem::separator(),
                    MenuItem::action("Quit mbar", Quit),
                ])]);
                cx.set_global(mac::UpdaterGlobal(updater));
            }

            let options = WindowOptions {
                window_bounds: Some(WindowBounds::centered(size(px(1100.), px(720.)), cx)),
                titlebar: Some(TitlebarOptions {
                    title: Some(format!("mbar — {bar_name}").into()),
                    ..Default::default()
                }),
                ..Default::default()
            };
            // Daemon version sync: inside mbar.app, restart a running daemon whose version
            // differs from the bundle's. A no-op outside a bundle (dev builds, Linux).
            // Opening the app also brings the bar up: when the login item is set up but
            // the default bar does not answer, launchd is asked to start it.
            if let Some(bundle) = mbar_ui_model::system::current_bundle() {
                let client = Client::new(bar_name.clone());
                cx.background_executor()
                    .spawn(async move {
                        #[cfg(target_os = "macos")]
                        if client.bar_name() == mbar_ipc::DEFAULT_BAR_NAME
                            && client.status() == mbar_ui_model::ipc::DaemonStatus::NotRunning
                            && mac::login_item::status() == mac::login_item::LoginItem::Enabled
                        {
                            if let Err(e) = views::onboarding::start_login_item() {
                                eprintln!("mbar-ui: {e}");
                            }
                            return;
                        }
                        mbar_ui_model::system::sync_daemon_version(&client, &bundle.short_version);
                    })
                    .detach();
            }

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
