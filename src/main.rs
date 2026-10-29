//! App shell: tray, global hotkey, hidden window, show/hide/placement.
//!
//! Speed comes from never cold-starting: the window is created once, hidden,
//! and Alt+J only shows/hides it with raw Win32 calls.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod autotag;
mod store;
mod themes;
mod trace;
mod ui;

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

use global_hotkey::{
    GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState,
    hotkey::{Code, HotKey, Modifiers},
};
use gpui_kit::*;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use tray_icon::{
    Icon, MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent,
    menu::{Menu, MenuEvent, MenuItem},
};
use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint,
};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, GetCursorPos, IsWindowVisible, SetForegroundWindow, SetWindowPos,
    ShowWindow, SW_HIDE, SW_SHOW, SWP_NOACTIVATE, SWP_NOZORDER,
};

use store::Store;

#[derive(Clone, Copy, Debug)]
enum Command {
    Toggle,
    Show,
    Quit,
}

/// GPUI's own `window_handle()` returns an `AnyWindowHandle`; call the
/// raw-window-handle trait method explicitly to get the Win32 HWND.
fn hwnd_of(window: &Window) -> HWND {
    match HasWindowHandle::window_handle(window).map(|h| h.as_raw()) {
        Ok(RawWindowHandle::Win32(h)) => HWND(h.hwnd.get() as *mut _),
        _ => HWND::default(),
    }
}

/// Center a 760x540 logical-pixel window on the monitor under the cursor,
/// corrected for that monitor's DPI. Done with raw Win32 because hidden
/// GPUI windows ignore `window_bounds` (see HACKATHON_PROMPT pitfalls).
fn place_on_cursor_monitor(hwnd: HWND) {
    unsafe {
        let mut pt = POINT::default();
        if GetCursorPos(&mut pt).is_err() {
            return;
        }
        let monitor = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if !GetMonitorInfoW(monitor, &mut info).as_bool() {
            return;
        }
        let mut dpi_x: u32 = 96;
        let mut dpi_y: u32 = 96;
        let _ = GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y);
        let dpi = if dpi_x == 0 { 96 } else { dpi_x as i32 };

        let work_w = info.rcWork.right - info.rcWork.left;
        let work_h = info.rcWork.bottom - info.rcWork.top;
        let w = ((760 * dpi) / 96).min(work_w.max(1));
        let h = ((540 * dpi) / 96).min(work_h.max(1));
        let x = info.rcWork.left + (work_w - w) / 2;
        let y = info.rcWork.top + (work_h - h) / 2;
        let _ = SetWindowPos(hwnd, None, x, y, w, h, SWP_NOZORDER | SWP_NOACTIVATE);
    }
}

/// Show the existing window and pull it to the foreground with keyboard
/// focus. Bare `SetForegroundWindow` alone is often demoted to a taskbar
/// flash when the call follows a global hotkey, so raise and activate too.
fn show_and_focus(hwnd: HWND) {
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = BringWindowToTop(hwnd);
        let _ = SetForegroundWindow(hwnd);
    }
}

/// 32x32 RGBA tray icon drawn in code (purple tile, white "L"), no assets.
fn tray_rgba() -> Vec<u8> {
    let mut rgba = vec![0u8; 32 * 32 * 4];
    for y in 0..32usize {
        for x in 0..32usize {
            let i = (y * 32 + x) * 4;
            let is_l = (10..14).contains(&x) && (8..24).contains(&y)
                || (20..24).contains(&y) && (10..22).contains(&x);
            let (r, g, b) = if is_l {
                (255, 255, 255)
            } else {
                (109, 88, 246)
            };
            rgba[i] = r;
            rgba[i + 1] = g;
            rgba[i + 2] = b;
            rgba[i + 3] = 255;
        }
    }
    rgba
}

fn main() {
    let app_started = std::time::Instant::now();
    // Single instance: a second launch exits quietly instead of failing on
    // the Alt+J hotkey registration. The mutex handle stays open for the
    // whole run because `application().run` blocks until quit.
    let _instance_mutex = unsafe {
        use windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError};
        use windows::Win32::System::Threading::CreateMutexW;
        use windows::core::PCWSTR;
        let name: Vec<u16> = "Local\\LazykastenSingleInstance\0".encode_utf16().collect();
        match CreateMutexW(None, true, PCWSTR(name.as_ptr())) {
            Ok(handle) => {
                if GetLastError() == ERROR_ALREADY_EXISTS {
                    eprintln!("lazykasten: already running");
                    return;
                }
                Some(handle)
            }
            Err(_) => None,
        }
    };

    let root = store::default_root();
    trace::init(&root);
    let store = match Store::open(&root) {
        Ok(s) => Arc::new(s),
        Err(e) => {
            eprintln!("lazykasten: failed to open vault at {}: {e:#}", root.display());
            std::process::exit(1);
        }
    };
    // `--eval-tags`: leave-one-out auto-tagging accuracy, no window.
    if std::env::args().any(|a| a == "--eval-tags") {
        let _ = store.sync_with_disk();
        match autotag::eval_tags(&store) {
            Ok(r) => {
                println!("notes evaluated: {}", r.notes);
                println!("precision: {:.3} ({} of {} applied tags correct)", r.precision, r.correct, r.applied);
                println!("recall: {:.3} ({} of {} real tags found)", r.recall, r.correct, r.real_total);
                println!("notes with >=1 correct tag: {:.3}", r.hit_rate);
                println!("time per note: {:.2} ms", r.ms_per_note);
                println!("thresholds: apply >= {}, suggest >= {}, name boost +{} (precision first: a wrong tag annoys more than a missing one)", autotag::AUTO_THRESHOLD, autotag::SUGGEST_THRESHOLD, autotag::NAME_BOOST);
            }
            Err(e) => {
                eprintln!("lazykasten: --eval-tags failed: {e:#}");
                std::process::exit(1);
            }
        }
        return;
    }
    let start_hidden = std::env::args().any(|a| a == "--hidden");

    // The saved theme name is read before the app starts so startup parses
    // only that one theme (the rest load when the picker first opens).
    let theme_data_dir = store.data_dir();
    gpui_kit::application().run(move |cx: &mut App| {
        gpui_kit::init(cx);
        themes::apply_saved(cx, &theme_data_dir);
        ui::bind_keys(cx);
        // A hidden window must never quit the app.
        cx.set_quit_mode(QuitMode::Explicit);

        // Startup re-check runs in the background, never on the UI thread.
        let sync_store = store.clone();
        cx.background_spawn(async move {
            let _ = sync_store.sync_with_disk();
        })
        .detach();

        let (tx, rx) = async_channel::unbounded::<Command>();

        // Global hotkey: Alt+J toggles. If registration fails, another
        // instance is probably already running.
        let manager = match GlobalHotKeyManager::new() {
            Ok(m) => m,
            Err(e) => {
                eprintln!("lazykasten: hotkey manager failed: {e}");
                cx.quit();
                return;
            }
        };
        if let Err(e) = manager.register(HotKey::new(Some(Modifiers::ALT), Code::KeyJ)) {
            eprintln!("lazykasten: could not register Alt+J (another instance?): {e}");
            cx.quit();
            return;
        }
        let tx_hotkey = tx.clone();
        GlobalHotKeyEvent::set_event_handler(Some(move |e: GlobalHotKeyEvent| {
            if e.state == HotKeyState::Pressed {
                let _ = tx_hotkey.try_send(Command::Toggle);
            }
        }));
        Box::leak(Box::new(manager));

        // Tray icon and menu.
        let open_item = MenuItem::new("Open note\tAlt+J", true, None);
        let quit_item = MenuItem::new("Quit Lazykasten", true, None);
        let open_id = open_item.id().clone();
        let quit_id = quit_item.id().clone();
        let menu = Menu::new();
        {
            if menu.append_items(&[&open_item, &quit_item]).is_ok() {
                if let Ok(icon) = Icon::from_rgba(tray_rgba(), 32, 32) {
                    let built = TrayIconBuilder::new()
                        .with_icon(icon)
                        .with_tooltip("Lazykasten (Alt+J)")
                        .with_menu(Box::new(menu))
                        .with_menu_on_left_click(false)
                        .build();
                    match built {
                        Ok(tray) => {
                            Box::leak(Box::new(tray));
                        }
                        Err(e) => eprintln!("lazykasten: tray icon failed: {e}"),
                    }
                }
            }
        }
        let tx_menu = tx.clone();
        MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
            if e.id == open_id {
                let _ = tx_menu.try_send(Command::Show);
            } else if e.id == quit_id {
                let _ = tx_menu.try_send(Command::Quit);
            }
        }));
        let tx_tray = tx.clone();
        TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = e
            {
                let _ = tx_tray.try_send(Command::Toggle);
            }
        }));

        // The one and only window, created hidden. No `window_bounds`:
        // hidden windows ignore it (see `place_on_cursor_monitor`).
        let view_store = store.clone();
        let options = WindowOptions {
            titlebar: Some(TitlebarOptions {
                title: Some("Lazykasten".into()),
                ..Default::default()
            }),
            show: false,
            focus: false,
            ..Default::default()
        };
        let hwnd_cell = Rc::new(Cell::new(HWND::default()));
        let hwnd_cell_for_window = hwnd_cell.clone();
        let opened = gpui_kit::open_window(options, cx, |window, cx| {
            hwnd_cell_for_window.set(hwnd_of(window));
            let view = cx.new(|cx| ui::LazyKasten::new(view_store, window, cx));
            // The close button hides; it never quits.
            let view_for_close = view.clone();
            window.on_window_should_close(cx, move |window, cx| {
                let hwnd = hwnd_of(window);
                view_for_close.update(cx, |v, cx| v.commit(window, cx));
                unsafe {
                    let _ = ShowWindow(hwnd, SW_HIDE);
                }
                false
            });
            view
        });
        let (any_window, view) = match opened {
            Ok(pair) => pair,
            Err(e) => {
                eprintln!("lazykasten: could not open window: {e}");
                cx.quit();
                return;
            }
        };
        trace::log("startup_ready", app_started.elapsed());

        if !start_hidden {
            let _ = tx.try_send(Command::Show);
        }

        // Command loop: hotkey/tray callbacks only send into the channel;
        // all window work happens here on the UI thread. Raw Win32 calls
        // (placement, ShowWindow) run *outside* `window.update`: a
        // cross-monitor SetWindowPos re-enters GPUI's move handler, which
        // panics on the already-borrowed App if called inside the update.
        let placed = Rc::new(Cell::new(false));
        let quit_store = store.clone();
        cx.spawn(async move |cx| {
            while let Ok(cmd) = rx.recv().await {
                let view = view.clone();
                let placed = placed.clone();
                let hwnd = hwnd_cell.get();
                let visible = unsafe { IsWindowVisible(hwnd).as_bool() };
                match cmd {
                    Command::Toggle if visible => {
                        let _ = any_window.update(cx, |_, window, cx| {
                            view.update(cx, |v, cx| v.commit(window, cx));
                        });
                        unsafe {
                            let _ = ShowWindow(hwnd, SW_HIDE);
                        }
                    }
                    Command::Toggle | Command::Show => {
                        if !visible {
                            let t0 = std::time::Instant::now();
                            let first = !placed.get();
                            if first {
                                place_on_cursor_monitor(hwnd);
                                placed.set(true);
                            }
                            show_and_focus(hwnd);
                            let _ = any_window.update(cx, |_, window, cx| {
                                view.update(cx, |v, cx| v.on_show(window, cx));
                                if trace::enabled() {
                                    window.on_next_frame(move |_, _| {
                                        trace::log(if first { "hotkey_first_frame" } else { "hotkey_frame" }, t0.elapsed());
                                    });
                                }
                            });
                            trace::log(if first { "hotkey_show_first" } else { "hotkey_show" }, t0.elapsed());
                        }
                    }
                    Command::Quit => {
                        let quit_store = quit_store.clone();
                        let _ = any_window.update(cx, |_, window, cx| {
                            view.update(cx, |v, cx| v.commit(window, cx));
                            quit_store.optimize();
                            cx.quit();
                        });
                    }
                }
            }
        })
        .detach();
    });
}
