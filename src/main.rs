//! App shell: tray, global hotkey, hidden window, show/hide/placement.
//!
//! Speed comes from never cold-starting: the window is created once, hidden,
//! and Alt+J only shows/hides it with raw Win32 calls.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod autotag;
mod graph;
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
    ShowWindow, SW_HIDE, SW_SHOW, SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_NOZORDER,
};

use store::Store;

#[derive(Clone, Debug)]
enum Command {
    Toggle,
    Show,
    Quit,
    Warm,
    WarmHide,
    ToggleAutostart,
    FilesChanged(Vec<std::path::PathBuf>),
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
/// Parse a settings `window` value ("x,y,w,h", physical pixels).
fn parse_window_rect(s: &str) -> Option<(i32, i32, i32, i32)> {
    let parts: Vec<i32> = s.split(',').filter_map(|p| p.trim().parse().ok()).collect();
    if parts.len() == 4 && parts[2] > 100 && parts[3] > 100 {
        Some((parts[0], parts[1], parts[2], parts[3]))
    } else {
        None
    }
}

fn window_rect_string(hwnd: HWND) -> Option<String> {
    unsafe {
        use windows::Win32::Foundation::RECT;
        use windows::Win32::UI::WindowsAndMessaging::GetWindowRect;
        let mut rc = RECT::default();
        if GetWindowRect(hwnd, &mut rc).is_ok() && rc.right > rc.left {
            return Some(format!(
                "{},{},{},{}",
                rc.left,
                rc.top,
                rc.right - rc.left,
                rc.bottom - rc.top
            ));
        }
    }
    None
}

/// Parse "alt+j" / "ctrl+shift+k" into a global hotkey.
fn parse_hotkey(s: &str) -> Option<HotKey> {
    let mut mods = Modifiers::empty();
    let mut key_code = None;
    for part in s.split('+') {
        match part.trim().to_lowercase().as_str() {
            "alt" => mods |= Modifiers::ALT,
            "ctrl" | "control" => mods |= Modifiers::CONTROL,
            "shift" => mods |= Modifiers::SHIFT,
            "super" | "win" => mods |= Modifiers::META,
            key => {
                let code = match key {
                    "a" => Code::KeyA, "b" => Code::KeyB, "c" => Code::KeyC, "d" => Code::KeyD,
                    "e" => Code::KeyE, "f" => Code::KeyF, "g" => Code::KeyG, "h" => Code::KeyH,
                    "i" => Code::KeyI, "j" => Code::KeyJ, "k" => Code::KeyK, "l" => Code::KeyL,
                    "m" => Code::KeyM, "n" => Code::KeyN, "o" => Code::KeyO, "p" => Code::KeyP,
                    "q" => Code::KeyQ, "r" => Code::KeyR, "s" => Code::KeyS, "t" => Code::KeyT,
                    "u" => Code::KeyU, "v" => Code::KeyV, "w" => Code::KeyW, "x" => Code::KeyX,
                    "y" => Code::KeyY, "z" => Code::KeyZ,
                    "0" => Code::Digit0, "1" => Code::Digit1, "2" => Code::Digit2,
                    "3" => Code::Digit3, "4" => Code::Digit4, "5" => Code::Digit5,
                    "6" => Code::Digit6, "7" => Code::Digit7, "8" => Code::Digit8,
                    "9" => Code::Digit9,
                    "f1" => Code::F1, "f2" => Code::F2, "f3" => Code::F3, "f4" => Code::F4,
                    "f5" => Code::F5, "f6" => Code::F6, "f7" => Code::F7, "f8" => Code::F8,
                    "f9" => Code::F9, "f10" => Code::F10, "f11" => Code::F11, "f12" => Code::F12,
                    _ => return None,
                };
                key_code = Some(code);
            }
        }
    }
    key_code.map(|code| HotKey::new(if mods.is_empty() { None } else { Some(mods) }, code))
}

const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Whether "Start with Windows" is currently registered.
fn autostart_enabled() -> bool {
    unsafe {
        use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RegCloseKey, RegOpenKeyExW, RegQueryValueExW, KEY_READ};
        use windows::core::PCWSTR;
        let mut key = windows::Win32::System::Registry::HKEY::default();
        if RegOpenKeyExW(HKEY_CURRENT_USER, PCWSTR(wide(RUN_KEY).as_ptr()), Some(0), KEY_READ, &mut key).is_err() {
            return false;
        }
        let name = wide("Lazykasten");
        let found = RegQueryValueExW(key, PCWSTR(name.as_ptr()), None, None, None, None).is_ok();
        let _ = RegCloseKey(key);
        found
    }
}

/// Write or remove the Run entry for "Start with Windows".
fn set_autostart(enabled: bool) {
    unsafe {
        use windows::Win32::System::Registry::{
            HKEY_CURRENT_USER, KEY_SET_VALUE, REG_SZ, RegCloseKey, RegDeleteValueW, RegOpenKeyExW,
            RegSetValueExW,
        };
        use windows::core::PCWSTR;
        let mut key = windows::Win32::System::Registry::HKEY::default();
        if RegOpenKeyExW(HKEY_CURRENT_USER, PCWSTR(wide(RUN_KEY).as_ptr()), Some(0), KEY_SET_VALUE, &mut key).is_err() {
            return;
        }
        let name = wide("Lazykasten");
        if enabled {
            if let Ok(exe) = std::env::current_exe() {
                let value = wide(&format!("\"{}\" --hidden", exe.display()));
                let bytes: &[u8] = std::slice::from_raw_parts(value.as_ptr() as *const u8, value.len() * 2);
                let _ = RegSetValueExW(key, PCWSTR(name.as_ptr()), Some(0), REG_SZ, Some(bytes));
            }
        } else {
            let _ = RegDeleteValueW(key, PCWSTR(name.as_ptr()));
        }
        let _ = RegCloseKey(key);
    }
}

fn place_on_cursor_monitor(hwnd: HWND, saved: Option<(i32, i32, i32, i32)>) {
    unsafe {
        if let Some((x, y, w, h)) = saved {
            let _ = SetWindowPos(hwnd, None, x, y, w, h, SWP_NOZORDER | SWP_NOACTIVATE);
            return;
        }
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

    // The settings file in the default vault may point at another vault.
    let mut root = store::default_root();
    let mut settings = themes::read_settings(&root.join(".lazykasten"));
    if let Some(v) = settings.get("vault") {
        if !v.trim().is_empty() {
            root = std::path::PathBuf::from(v);
            settings = themes::read_settings(&root.join(".lazykasten"));
        }
    }
    let hotkey_setting = settings.get("hotkey").cloned();
    let saved_window_setting = settings.get("window").cloned();
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
        // Hotkey from settings (`hotkey = "alt+j"`), falling back to
        // Alt+J when the configured one cannot be registered.
        let mut hotkey_status: Option<String> = None;
        let configured = hotkey_setting
            .as_deref()
            .and_then(parse_hotkey)
            .unwrap_or_else(|| HotKey::new(Some(Modifiers::ALT), Code::KeyJ));
        if manager.register(configured).is_err() {
            let fallback = HotKey::new(Some(Modifiers::ALT), Code::KeyJ);
            if configured != fallback && manager.register(fallback).is_ok() {
                hotkey_status = Some(format!(
                    "Could not register {}; using Alt+J",
                    hotkey_setting.as_deref().unwrap_or("hotkey")
                ));
            } else {
                eprintln!("lazykasten: could not register the global hotkey (another instance?)");
                cx.quit();
                return;
            }
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
        let autostart_item = tray_icon::menu::CheckMenuItem::new(
            "Start with Windows",
            true,
            autostart_enabled(),
            None,
        );
        let quit_item = MenuItem::new("Quit Lazykasten", true, None);
        let open_id = open_item.id().clone();
        let autostart_id = autostart_item.id().clone();
        let quit_id = quit_item.id().clone();
        let menu = Menu::new();
        {
            if menu.append_items(&[&open_item, &autostart_item, &quit_item]).is_ok() {
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
            } else if e.id == autostart_id {
                let _ = tx_menu.try_send(Command::ToggleAutostart);
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
        if let Some(msg) = hotkey_status.take() {
            let _ = view.update(cx, |v, cx| v.set_initial_status(msg, cx));
        }
        trace::log("startup_ready", app_started.elapsed());

        // Warm first-frame costs while still hidden: shape the strings
        // the editor screen shows (font/atlas warm-up) and request one
        // frame so the swapchain exists before the first Alt+J. This
        // never calls `activate_window()`.
        let warm_t0 = std::time::Instant::now();
        let _ = any_window.update(cx, |_, window, _cx| {
            // `text_style()` needs a rendering view; use the default
            // font directly (same family the theme resolves to).
            let font = Font::default();
            for s in [
                "Capture a thought\u{2026}  (Tag: a, b  or  #hashtags)",
                "Alt+J save & hide   Ctrl+O browse   Ctrl+T suggest tags",
                "#inbox  0 backlinks",
            ] {
                let _ = window.text_system().shape_line(
                    s.to_string().into(),
                    px(14.0),
                    &[TextRun {
                        len: s.len(),
                        font: font.clone(),
                        color: gpui_kit::black(),
                        background_color: None,
                        underline: None,
                        strikethrough: None,
                    }],
                    None,
                );
            }
        });
        // One invalidated frame while hidden (renders the editor once;
        // `request_animation_frame` outside a view render panics on
        // `current_view`, so this goes through the view instead).
        let warm_view = view.clone();
        let _ = warm_view.update(cx, |_, cx| cx.notify());
        trace::log("warmup", warm_t0.elapsed());

        // Swapchain warm-up: a hidden window never presents, so the
        // first visible frame would pay GPU setup (~17ms). Show the
        // placed window once with SW_SHOWNOACTIVATE (never focused,
        // never `activate_window()`) and hide it after a short delay;
        // an Alt+J inside that window adopts the warm show instead.
        let warming = Rc::new(Cell::new(start_hidden));
        if start_hidden {
            // Sent to the command loop: the show must run outside any
            // App borrow (a SetWindowPos/ShowWindow inside one
            // re-enters GPUI's handlers and panics).
            let _ = tx.try_send(Command::Warm);
        }

        if !start_hidden {
            let _ = tx.try_send(Command::Show);
        }

        // Command loop: hotkey/tray callbacks only send into the channel;
        // all window work happens here on the UI thread. Raw Win32 calls
        // (placement, ShowWindow) run *outside* `window.update`: a
        // cross-monitor SetWindowPos re-enters GPUI's move handler, which
        // panics on the already-borrowed App if called inside the update.
        let placed = Rc::new(Cell::new(start_hidden));
        let saved_rect = saved_window_setting.as_deref().and_then(parse_window_rect);
        let settings_dir = theme_data_dir.clone();
        let quit_store = store.clone();
        // Live file watching: notify on notes/, debounced 300ms in a
        // blocking thread (0% idle CPU), re-indexed in the background.
        let _watcher = {
            use notify::Watcher as _;
            let (wtx, wrx) = std::sync::mpsc::channel::<Vec<std::path::PathBuf>>();
            let tx_files = tx.clone();
            std::thread::spawn(move || {
                while let Ok(first) = wrx.recv() {
                    let mut paths = first;
                    loop {
                        match wrx.recv_timeout(std::time::Duration::from_millis(300)) {
                            Ok(mut more) => paths.append(&mut more),
                            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => break,
                            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
                        }
                    }
                    paths.sort();
                    paths.dedup();
                    let _ = tx_files.try_send(Command::FilesChanged(paths));
                }
            });
            match notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
                if let Ok(ev) = res {
                    let _ = wtx.send(ev.paths);
                }
            }) {
                Ok(mut w) => {
                    let _ = w.watch(&store.notes_dir(), notify::RecursiveMode::NonRecursive);
                    Some(w)
                }
                Err(e) => {
                    eprintln!("lazykasten: file watcher failed: {e}");
                    None
                }
            }
        };

        let tx_loop = tx.clone();
        cx.spawn(async move |cx| {
            // Keep the watcher alive for the app's whole life: locals
            // of the startup closure drop when it returns.
            let _watcher_keep = _watcher;
            let autostart_item = autostart_item;
            while let Ok(cmd) = rx.recv().await {
                let view = view.clone();
                let placed = placed.clone();
                let warming = warming.clone();
                let hwnd = hwnd_cell.get();
                let visible = unsafe { IsWindowVisible(hwnd).as_bool() };
                match cmd {
                    Command::ToggleAutostart => {
                        let enable = !autostart_enabled();
                        set_autostart(enable);
                        autostart_item.set_checked(enable);
                    }
                    Command::FilesChanged(paths) => {
                        let _ = any_window.update(cx, |_, window, cx| {
                            view.update(cx, |v, cx| v.on_external_files(paths, window, cx));
                        });
                    }
                    Command::Warm => {
                        place_on_cursor_monitor(hwnd, saved_rect);
                        unsafe {
                            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
                        }
                        let tx_hide = tx_loop.clone();
                        std::thread::spawn(move || {
                            std::thread::sleep(std::time::Duration::from_millis(250));
                            let _ = tx_hide.try_send(Command::WarmHide);
                        });
                    }
                    Command::WarmHide => {
                        if warming.get() {
                            warming.set(false);
                            unsafe {
                                let _ = ShowWindow(hwnd, SW_HIDE);
                            }
                        }
                    }
                    Command::Toggle if visible && !warming.get() => {
                        let _ = any_window.update(cx, |_, window, cx| {
                            view.update(cx, |v, cx| v.commit(window, cx));
                        });
                        // Remember the window's size and position.
                        if let Some(rect) = window_rect_string(hwnd) {
                            let dir = settings_dir.clone();
                            std::thread::spawn(move || {
                                let mut map = themes::read_settings(&dir);
                                map.insert("window".to_string(), rect);
                                themes::write_settings(&dir, &map);
                            });
                        }
                        unsafe {
                            let _ = ShowWindow(hwnd, SW_HIDE);
                        }
                    }
                    Command::Toggle | Command::Show => {
                        // A show during warm-up adopts the already
                        // visible (but unfocused) warm window.
                        let adopt = warming.get();
                        if adopt {
                            warming.set(false);
                        }
                        if !visible || adopt {
                            let t0 = std::time::Instant::now();
                            let first = adopt || !placed.get();
                            if first {
                                place_on_cursor_monitor(hwnd, saved_rect);
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
