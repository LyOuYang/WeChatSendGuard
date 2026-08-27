#![cfg_attr(windows, deny(unsafe_op_in_unsafe_fn))]
#![cfg_attr(windows, windows_subsystem = "windows")]
//! Production composition root for the Windows application.
//!
//! This crate owns lifecycle and UI intent handling. It never makes a Weixin decision itself:
//! `guard-service` evaluates cached snapshots, and `platform-windows` can inject only after the
//! service has revalidated the original target.

#[cfg(windows)]
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use slint::{CloseRequestResponse, ComponentHandle, ModelRc, Timer, TimerMode, VecModel};
use std::{
    cell::RefCell,
    error::Error,
    fs,
    path::PathBuf,
    rc::{Rc, Weak as RcWeak},
    sync::Arc,
    thread,
    time::{Duration, Instant, SystemTime},
};
use uuid::Uuid;
use wechat_send_guard_core::{
    AppSettings, ChatContext, ChatTargetKind, ConfirmationMode, ConfirmationOutcome,
    FileSettingsStore, PendingConfirmation, ProtectedChat, RuleMode, UnknownContextBehavior,
    export_protected_chats, import_protected_chats, normalize_title,
};
use wechat_send_guard_desktop_ui::{
    AppTray, AppWindow, ChatRow, ConfirmationWindow, SlintAboutWindow,
};
use wechat_send_guard_platform_api::{ChatContextProvider, StartupRegistration};
use wechat_send_guard_platform_windows::{
    KeyboardKey, TRUSTED_WEIXIN_PATH, WindowsAuditLog, WindowsContextMonitor,
    WindowsContextProvider, WindowsInputInjector, WindowsKeyboardHook, WindowsStartupRegistration,
    activate_window, center_popup_over_window, cursor_screen_position, default_audit_log_directory,
    is_valid_weixin_executable_path, path_matches_trusted_weixin, select_protected_chat_export,
    select_protected_chat_import,
};
use wechat_send_guard_service::{CompletionResult, EnterHandling, GuardService, PhysicalEnter};

const APPLICATION_DIRECTORY: &str = "WeChatSendGuard";
const SETTINGS_FILE_NAME: &str = "settings.json";
const APPLICATION_VERSION: &str = env!("CARGO_PKG_VERSION");
const STATUS_POLL_INTERVAL: Duration = Duration::from_millis(200);
const CONFIRMATION_TICK_INTERVAL: Duration = Duration::from_millis(50);
const HOLD_TICK_INTERVAL: Duration = Duration::from_millis(20);

type ActiveConfirmationSlot = Rc<RefCell<Option<ActiveConfirmation>>>;
type ActiveConfirmationWeak = RcWeak<RefCell<Option<ActiveConfirmation>>>;
type SlintAboutSlot = Rc<RefCell<Option<SlintAboutWindow>>>;
type WindowDragOrigin = Rc<RefCell<Option<(slint::PhysicalPosition, slint::PhysicalPosition)>>>;

struct Controller {
    store: FileSettingsStore,
    settings: AppSettings,
    service: Arc<GuardService>,
    provider: Arc<WindowsContextProvider>,
    audit: Arc<WindowsAuditLog>,
    startup: Option<WindowsStartupRegistration>,
}

impl Controller {
    fn apply_settings(&mut self, settings: AppSettings) -> Result<(), String> {
        let settings = settings.sanitize();
        self.store
            .save(settings.clone())
            .map_err(|error| format!("无法保存设置：{error}"))?;

        self.provider
            .set_trusted_weixin_executable_path(settings.trusted_weixin_executable_path.as_deref());
        self.service.update_settings(settings.clone());
        self.audit.set_retention_days(settings.log_retention_days);
        self.settings = settings.clone();

        if let Some(startup) = &self.startup {
            startup
                .apply(settings.start_with_windows)
                .map_err(|error| format!("设置开机启动失败：{error}"))?;
        } else if settings.start_with_windows {
            return Err("无法确定当前程序路径，未能设置开机启动。".to_owned());
        }
        Ok(())
    }

    fn set_protection_enabled(&mut self, enabled: bool) -> Result<(), String> {
        let mut settings = self.settings.clone();
        settings.enabled = enabled;
        let settings = settings.sanitize();
        self.store
            .save(settings.clone())
            .map_err(|error| format!("无法保存发送守护状态：{error}"))?;

        self.service.update_settings(settings.clone());
        self.settings = settings;
        Ok(())
    }

    fn active_chats(&self) -> &[ProtectedChat] {
        if self.settings.rule_mode == RuleMode::ConfirmUnlessExcluded {
            &self.settings.exempted_chats
        } else {
            &self.settings.protected_chats
        }
    }
}

struct ActiveConfirmation {
    window: ConfirmationWindow,
    pending: PendingConfirmation,
    _timeout_timer: Timer,
    _hold_timer: Timer,
}

fn main() {
    if let Err(error) = run() {
        show_startup_error(&error.to_string());
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let arguments = std::env::args().collect::<Vec<_>>();

    #[cfg(debug_assertions)]
    if arguments.iter().any(|argument| argument == "--ui-preview") {
        return run_ui_preview(AppSettings::default().sanitize());
    }
    #[cfg(debug_assertions)]
    if arguments.iter().any(|argument| argument == "--ui-snapshot") {
        return run_ui_snapshot(AppSettings::default().sanitize());
    }
    #[cfg(debug_assertions)]
    if arguments
        .iter()
        .any(|argument| argument == "--confirmation-snapshot")
    {
        return run_confirmation_snapshot();
    }

    let start_in_background = arguments
        .iter()
        .any(|argument| is_background_start_argument(argument));
    let local_app_data = local_app_data_directory()?;
    let settings_path = local_app_data
        .join(APPLICATION_DIRECTORY)
        .join(SETTINGS_FILE_NAME);
    let store = FileSettingsStore::new(settings_path);
    let settings = store.load()?.sanitize();

    let provider = Arc::new(
        WindowsContextProvider::new_with_trusted_weixin_executable_path(
            settings.trusted_weixin_executable_path.as_deref(),
        ),
    );
    let audit = Arc::new(WindowsAuditLog::new(
        default_audit_log_directory(&local_app_data),
        settings.log_retention_days,
    )?);
    let injector = Arc::new(WindowsInputInjector::with_random_marker());
    let service = Arc::new(GuardService::new(
        settings.clone(),
        provider.clone(),
        injector.clone(),
        audit.clone(),
    ));
    let controller = Rc::new(RefCell::new(Controller {
        store,
        settings,
        service: service.clone(),
        provider: provider.clone(),
        audit: audit.clone(),
        startup: WindowsStartupRegistration::for_current_executable().ok(),
    }));

    let main_window = AppWindow::new()?;
    let tray = AppTray::new()?;
    let active_confirmation: ActiveConfirmationSlot = Rc::new(RefCell::new(None));
    let about_slint: SlintAboutSlot = Rc::new(RefCell::new(None));
    render_settings(&main_window, &controller.borrow().settings, None);
    bind_window_callbacks(
        &main_window,
        &tray,
        Rc::clone(&controller),
        Rc::clone(&active_confirmation),
        Rc::clone(&about_slint),
    );
    bind_tray_callbacks(
        &tray,
        &main_window,
        Rc::clone(&controller),
        Rc::clone(&about_slint),
    );
    let status_timer = install_status_refresh(
        &main_window,
        &tray,
        Rc::clone(&controller),
        provider.clone(),
    );

    let mut context_monitor = WindowsContextMonitor::new(provider.clone());
    context_monitor.start()?;

    let mut keyboard_hook = WindowsKeyboardHook::new(injector.marker());
    let main_window_weak = main_window.as_weak();
    let hook_service = service.clone();
    keyboard_hook.set_key_down_handler(Arc::new(move |stroke| {
        if stroke.key == KeyboardKey::Escape {
            let Some(pending) = hook_service.current_pending_confirmation() else {
                return false;
            };
            if main_window_weak
                .upgrade_in_event_loop(|window| window.invoke_confirmation_cancel_requested())
                .is_err()
            {
                // The dialog cannot be reached while the UI is shutting down. Preserve the
                // cancellation-first guarantee without injecting anything into the target.
                let _ = hook_service.complete_confirmation(
                    &pending,
                    ConfirmationOutcome::Cancelled,
                    SystemTime::now(),
                );
            }
            return true;
        }

        let handling = hook_service.handle_physical_enter(
            PhysicalEnter {
                is_numpad_enter: stroke.is_numpad_enter,
                is_injected: stroke.is_injected,
                shift_pressed: stroke.shift_pressed,
                ime_composing: stroke.ime_composing,
                foreground_window: stroke.foreground_window,
            },
            SystemTime::now(),
        );

        match handling {
            EnterHandling::PassThrough => false,
            EnterHandling::SuppressBlockedUnknown
            | EnterHandling::SuppressWhileConfirmationActive => true,
            EnterHandling::SuppressAndConfirm(pending) => {
                let attempt_id = pending.attempt_id.to_string();
                if main_window_weak
                    .upgrade_in_event_loop(move |window| {
                        window.invoke_confirmation_requested(attempt_id.into());
                    })
                    .is_err()
                {
                    // If the UI is shutting down, the original physical key remains suppressed and
                    // the pending state is closed without any injection.
                    let _ = hook_service.complete_confirmation(
                        &pending,
                        ConfirmationOutcome::Cancelled,
                        SystemTime::now(),
                    );
                }
                true
            }
        }
    }));
    keyboard_hook.start()?;

    tray.show()?;
    if !start_in_background {
        main_window.show()?;
    }
    slint::run_event_loop()?;

    keyboard_hook.stop();
    context_monitor.stop();
    status_timer.stop();
    audit.shutdown();
    Ok(())
}

#[cfg(debug_assertions)]
fn run_ui_preview(settings: AppSettings) -> Result<(), Box<dyn Error>> {
    let window = AppWindow::new()?;
    render_settings(&window, &settings, None);
    window.set_status_text("UI 预览模式：未启动发送守护".into());
    window.set_status_healthy(false);
    window.show()?;
    slint::run_event_loop()?;
    Ok(())
}

#[cfg(debug_assertions)]
fn run_ui_snapshot(settings: AppSettings) -> Result<(), Box<dyn Error>> {
    let window = AppWindow::new()?;
    render_settings(&window, &settings, None);
    let active_page = std::env::var("WCSG_UI_SNAPSHOT_PAGE")
        .ok()
        .and_then(|value| value.parse::<i32>().ok())
        .filter(|page| (0..=2).contains(page))
        .unwrap_or(0);
    window.set_active_page(active_page);
    window.set_status_text("UI 像素校验模式：未启动发送守护".into());
    window.set_status_healthy(false);
    let snapshot_path = std::env::temp_dir().join("WeChatSendGuard-ui-preview.ppm");
    capture_component_snapshot(&window, snapshot_path)
}

#[cfg(debug_assertions)]
fn run_confirmation_snapshot() -> Result<(), Box<dyn Error>> {
    let window = ConfirmationWindow::new()?;
    window.set_target_kind("群聊".into());
    window.set_target_name("测试会话".into());
    window.set_draft_preview("这是一段仅用于界面像素校验的示例文本，不会发送。".into());
    window.set_confirmation_mode(1);
    window.set_hold_milliseconds(800);
    window.set_confirm_label("按住确认 (0.8s)".into());
    window.set_hold_progress(0.45);
    window.set_countdown_progress(72.0);
    window.set_countdown_text("7.2 秒后自动取消".into());
    let snapshot_path = std::env::temp_dir().join("WeChatSendGuard-confirmation-preview.ppm");
    capture_component_snapshot(&window, snapshot_path)
}

#[cfg(debug_assertions)]
fn capture_component_snapshot<Component>(
    component: &Component,
    snapshot_path: PathBuf,
) -> Result<(), Box<dyn Error>>
where
    Component: ComponentHandle + 'static,
{
    let completion = Rc::new(RefCell::new(None::<Result<(), String>>));
    let completion_for_timer = Rc::clone(&completion);
    let component_weak = component.as_weak();

    component.show()?;
    Timer::single_shot(Duration::from_millis(250), move || {
        let outcome = component_weak
            .upgrade()
            .ok_or_else(|| "UI snapshot window was dropped before the snapshot".to_owned())
            .and_then(|component| {
                let snapshot = component
                    .window()
                    .take_snapshot()
                    .map_err(|error| error.to_string())?;
                let result = write_snapshot_as_ppm(&snapshot_path, &snapshot);
                let _ = component.hide();
                result
            });
        *completion_for_timer.borrow_mut() = Some(outcome);
        let _ = slint::quit_event_loop();
    });
    slint::run_event_loop()?;

    completion
        .borrow_mut()
        .take()
        .ok_or_else(|| std::io::Error::other("UI snapshot timer did not run"))?
        .map_err(std::io::Error::other)?;
    Ok(())
}

#[cfg(debug_assertions)]
fn write_snapshot_as_ppm(
    path: &std::path::Path,
    snapshot: &slint::SharedPixelBuffer<slint::Rgba8Pixel>,
) -> Result<(), String> {
    let width = snapshot.width();
    let height = snapshot.height();
    let mut ppm = format!("P6\n{width} {height}\n255\n").into_bytes();
    ppm.reserve(
        (width as usize)
            .saturating_mul(height as usize)
            .saturating_mul(3),
    );
    for rgba in snapshot.as_bytes().chunks_exact(4) {
        ppm.extend_from_slice(&rgba[..3]);
    }
    fs::write(path, ppm).map_err(|error| format!("unable to write UI snapshot: {error}"))
}

#[cfg(windows)]
fn show_startup_error(message: &str) {
    use windows::{
        Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW},
        core::PCWSTR,
    };

    let title = wide_for_message_box("WeChatSendGuard 无法启动");
    let message = wide_for_message_box(message);
    // SAFETY: both UTF-16 buffers are NUL-terminated and live through the synchronous dialog.
    unsafe {
        let _ = MessageBoxW(
            None,
            PCWSTR(message.as_ptr()),
            PCWSTR(title.as_ptr()),
            MB_OK | MB_ICONERROR,
        );
    }
}

#[cfg(windows)]
fn wide_for_message_box(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(not(windows))]
fn show_startup_error(message: &str) {
    eprintln!("WeChatSendGuard failed to start: {message}");
}

fn local_app_data_directory() -> Result<PathBuf, Box<dyn Error>> {
    if let Some(path) = std::env::var_os("LOCALAPPDATA") {
        return Ok(PathBuf::from(path));
    }
    let user_profile = std::env::var_os("USERPROFILE")
        .ok_or("LOCALAPPDATA and USERPROFILE are unavailable for the current user")?;
    Ok(PathBuf::from(user_profile).join("AppData").join("Local"))
}

fn is_background_start_argument(argument: &str) -> bool {
    ["--silent", "--startup", "--background"]
        .iter()
        .any(|candidate| argument.eq_ignore_ascii_case(candidate))
}

fn bind_window_callbacks(
    main_window: &AppWindow,
    tray: &AppTray,
    controller: Rc<RefCell<Controller>>,
    active_confirmation: ActiveConfirmationSlot,
    about_slint: SlintAboutSlot,
) {
    let main_window_weak = main_window.as_weak();
    let tray_weak = tray.as_weak();
    let drag_origin: WindowDragOrigin = Rc::new(RefCell::new(None));
    main_window.on_minimize_requested({
        let main_window_weak = main_window_weak.clone();
        move || {
            if let Some(window) = main_window_weak.upgrade() {
                window.window().set_minimized(true);
            }
        }
    });
    main_window.on_close_requested({
        let main_window_weak = main_window_weak.clone();
        move || {
            if let Some(window) = main_window_weak.upgrade() {
                let _ = window.hide();
            }
        }
    });
    main_window.on_title_bar_pressed({
        let drag_origin = Rc::clone(&drag_origin);
        let main_window_weak = main_window_weak.clone();
        move || {
            if let Some(window) = main_window_weak.upgrade()
                && let Some((x, y)) = cursor_screen_position()
            {
                *drag_origin.borrow_mut() = Some((
                    window.window().position(),
                    slint::PhysicalPosition::new(x, y),
                ));
            }
        }
    });
    main_window.on_title_bar_moved({
        let drag_origin = Rc::clone(&drag_origin);
        let main_window_weak = main_window_weak.clone();
        move || {
            let Some((start_position, start_cursor)) = *drag_origin.borrow() else {
                return;
            };
            let Some((x, y)) = cursor_screen_position() else {
                return;
            };
            if let Some(window) = main_window_weak.upgrade() {
                window
                    .window()
                    .set_position(window_position_for_cursor_drag(
                        start_position,
                        start_cursor,
                        slint::PhysicalPosition::new(x, y),
                    ));
            }
        }
    });
    main_window.on_settings_dirty({
        let main_window_weak = main_window_weak.clone();
        move || mark_unsaved(&main_window_weak)
    });
    main_window.on_aliases_changed({
        let main_window_weak = main_window_weak.clone();
        move |_| mark_unsaved(&main_window_weak)
    });
    main_window.on_protection_toggled({
        let controller = Rc::clone(&controller);
        let main_window_weak = main_window_weak.clone();
        let tray_weak = tray_weak.clone();
        move |enabled| {
            update_protection_enabled(enabled, &controller, &main_window_weak, &tray_weak)
        }
    });
    main_window.on_confirmation_mode_selected({
        let main_window_weak = main_window_weak.clone();
        move |_| mark_unsaved(&main_window_weak)
    });
    main_window.on_keyboard_enter_toggled({
        let main_window_weak = main_window_weak.clone();
        move |_| mark_unsaved(&main_window_weak)
    });
    main_window.on_numpad_enter_toggled({
        let main_window_weak = main_window_weak.clone();
        move |_| mark_unsaved(&main_window_weak)
    });
    main_window.on_unknown_context_selected({
        let main_window_weak = main_window_weak.clone();
        move |_| mark_unsaved(&main_window_weak)
    });
    main_window.on_startup_toggled({
        let main_window_weak = main_window_weak.clone();
        move |_| mark_unsaved(&main_window_weak)
    });
    main_window.on_reset_weixin_executable_path({
        let main_window_weak = main_window_weak.clone();
        move || {
            if let Some(window) = main_window_weak.upgrade() {
                window.set_weixin_executable_path(TRUSTED_WEIXIN_PATH.into());
                mark_unsaved(&main_window_weak);
            }
        }
    });

    main_window.on_rule_mode_selected({
        let controller = Rc::clone(&controller);
        let main_window_weak = main_window_weak.clone();
        move |mode| {
            let Some(rule_mode) = rule_mode_from_ui(mode) else {
                return;
            };
            let mut controller = controller.borrow_mut();
            let mut settings = controller.settings.clone();
            settings.rule_mode = rule_mode;
            match controller.apply_settings(settings) {
                Ok(()) => {
                    let settings = controller.settings.clone();
                    drop(controller);
                    if let Some(window) = main_window_weak.upgrade() {
                        render_chat_list(&window, &settings, None);
                        set_save_status(&window, "名单模式已即时生效", false);
                    }
                }
                Err(error) => {
                    if let Some(window) = main_window_weak.upgrade() {
                        set_save_status(&window, error, true);
                    }
                }
            }
        }
    });

    main_window.on_chat_selected({
        let controller = Rc::clone(&controller);
        let main_window_weak = main_window_weak.clone();
        move |id| {
            if let Some(window) = main_window_weak.upgrade() {
                window.set_selected_chat_id(id.clone());
                let aliases = controller
                    .borrow()
                    .active_chats()
                    .iter()
                    .find(|chat| chat.id.to_string() == id.as_str())
                    .map(|chat| chat.aliases.join(" | "))
                    .unwrap_or_default();
                window.set_alias_text(aliases.into());
            }
        }
    });

    main_window.on_add_current_chat({
        let controller = Rc::clone(&controller);
        let main_window_weak = main_window_weak.clone();
        move || {
            if let Some(window) = main_window_weak.upgrade() {
                add_current_chat(&window, &controller);
            }
        }
    });
    main_window.on_remove_selected_chat({
        let controller = Rc::clone(&controller);
        let main_window_weak = main_window_weak.clone();
        move || {
            if let Some(window) = main_window_weak.upgrade() {
                remove_selected_chat(&window, &controller);
            }
        }
    });
    main_window.on_import_list({
        let controller = Rc::clone(&controller);
        let main_window_weak = main_window_weak.clone();
        move || {
            if let Some(window) = main_window_weak.upgrade() {
                import_active_list(&window, &controller);
            }
        }
    });
    main_window.on_export_list({
        let controller = Rc::clone(&controller);
        let main_window_weak = main_window_weak.clone();
        move || {
            if let Some(window) = main_window_weak.upgrade() {
                export_active_list(&window, &controller);
            }
        }
    });
    main_window.on_save_settings({
        let controller = Rc::clone(&controller);
        let main_window_weak = main_window_weak.clone();
        move || {
            if let Some(window) = main_window_weak.upgrade() {
                save_form_settings(&window, &controller);
            }
        }
    });
    main_window.on_confirmation_requested({
        let controller = Rc::clone(&controller);
        let active_confirmation = Rc::clone(&active_confirmation);
        let main_window_weak = main_window_weak.clone();
        move |attempt_id| {
            if let Some(window) = main_window_weak.upgrade()
                && let Err(error) = show_confirmation(
                    &window,
                    &controller,
                    &active_confirmation,
                    attempt_id.as_str(),
                )
            {
                set_save_status(&window, error, true);
            }
        }
    });
    main_window.on_confirmation_cancel_requested({
        let service = controller.borrow().service.clone();
        let active_confirmation = Rc::clone(&active_confirmation);
        let main_window_weak = main_window_weak.clone();
        move || cancel_pending_confirmation(&active_confirmation, &service, &main_window_weak)
    });
    main_window.on_about_slint_requested({
        let about_slint = Rc::clone(&about_slint);
        let main_window_weak = main_window_weak.clone();
        move || {
            if let Err(error) = show_about_slint(&about_slint)
                && let Some(window) = main_window_weak.upgrade()
            {
                set_save_status(&window, error, true);
            }
        }
    });

    main_window
        .window()
        .on_close_requested(|| CloseRequestResponse::HideWindow);
}

fn bind_tray_callbacks(
    tray: &AppTray,
    main_window: &AppWindow,
    controller: Rc<RefCell<Controller>>,
    about_slint: SlintAboutSlot,
) {
    let main_window_weak = main_window.as_weak();
    let tray_weak = tray.as_weak();
    tray.on_open_settings({
        let main_window_weak = main_window_weak.clone();
        move || {
            if let Some(window) = main_window_weak.upgrade() {
                show_and_restore_main_window(&window);
            }
        }
    });
    tray.on_add_current_chat({
        let controller = Rc::clone(&controller);
        let main_window_weak = main_window_weak.clone();
        move || {
            if let Some(window) = main_window_weak.upgrade() {
                show_and_restore_main_window(&window);
                add_current_chat(&window, &controller);
            }
        }
    });
    tray.on_bypass_requested({
        let controller = Rc::clone(&controller);
        let main_window_weak = main_window_weak.clone();
        move |minutes| {
            let service = controller.borrow().service.clone();
            let result = service.try_grant_current_bypass(minutes as u32, SystemTime::now());
            if let Some(window) = main_window_weak.upgrade() {
                show_and_restore_main_window(&window);
                match result {
                    Some(chat) => set_save_status(
                        &window,
                        format!("已临时放行 {} {} 分钟", chat.display_name, minutes),
                        false,
                    ),
                    None => set_save_status(
                        &window,
                        "临时放行仅适用于当前保护名单中的可识别会话。",
                        true,
                    ),
                }
            }
        }
    });
    tray.on_protection_toggled({
        let controller = Rc::clone(&controller);
        let main_window_weak = main_window_weak.clone();
        let tray_weak = tray_weak.clone();
        move |enabled| {
            update_protection_enabled(enabled, &controller, &main_window_weak, &tray_weak);
        }
    });
    tray.on_show_status({
        let controller = Rc::clone(&controller);
        let main_window_weak = main_window_weak.clone();
        move || {
            if let Some(window) = main_window_weak.upgrade() {
                show_and_restore_main_window(&window);
                let controller = controller.borrow();
                let (status, _) =
                    status_for_context(&controller.settings, &controller.provider.current());
                set_save_status(&window, status, false);
            }
        }
    });
    tray.on_about_slint_requested({
        let about_slint = Rc::clone(&about_slint);
        move || {
            let _ = show_about_slint(&about_slint);
        }
    });
    tray.on_exit_requested(|| {
        let _ = slint::quit_event_loop();
    });
}

fn show_and_restore_main_window(window: &AppWindow) {
    window.window().set_minimized(false);
    let _ = window.show();
}

fn update_protection_enabled(
    enabled: bool,
    controller: &Rc<RefCell<Controller>>,
    main_window_weak: &slint::Weak<AppWindow>,
    tray_weak: &slint::Weak<AppTray>,
) {
    let result = {
        let mut controller = controller.borrow_mut();
        controller.set_protection_enabled(enabled)
    };

    match result {
        Ok(()) => {
            if let Some(tray) = tray_weak.upgrade() {
                tray.set_protection_enabled(enabled);
            }
            if let Some(window) = main_window_weak.upgrade() {
                window.set_protection_enabled(enabled);
                let message = if window.get_has_unsaved_settings() {
                    if enabled {
                        "发送守护已启用，立即生效；其他更改尚未保存"
                    } else {
                        "发送守护已暂停，立即生效；其他更改尚未保存"
                    }
                } else if enabled {
                    "发送守护已启用，立即生效"
                } else {
                    "发送守护已暂停，立即生效"
                };
                set_save_status(&window, message, false);
            }
        }
        Err(error) => {
            let persisted_enabled = controller.borrow().settings.enabled;
            if let Some(tray) = tray_weak.upgrade() {
                tray.set_protection_enabled(persisted_enabled);
            }
            if let Some(window) = main_window_weak.upgrade() {
                window.set_protection_enabled(persisted_enabled);
                set_save_status(&window, error, true);
            }
        }
    }
}

fn cancel_pending_confirmation(
    active_slot: &ActiveConfirmationSlot,
    service: &Arc<GuardService>,
    main_window_weak: &slint::Weak<AppWindow>,
) {
    let active_window = active_slot
        .borrow()
        .as_ref()
        .map(|active| active.window.clone_strong());
    if let Some(window) = active_window {
        window.invoke_cancelled();
        return;
    }

    let Some(pending) = service.current_pending_confirmation() else {
        return;
    };
    let result =
        service.complete_confirmation(&pending, ConfirmationOutcome::Cancelled, SystemTime::now());
    show_completion_status(main_window_weak, result);
}

fn window_position_for_cursor_drag(
    start_window_position: slint::PhysicalPosition,
    start_cursor_position: slint::PhysicalPosition,
    current_cursor_position: slint::PhysicalPosition,
) -> slint::PhysicalPosition {
    slint::PhysicalPosition::new(
        start_window_position.x.saturating_add(
            current_cursor_position
                .x
                .saturating_sub(start_cursor_position.x),
        ),
        start_window_position.y.saturating_add(
            current_cursor_position
                .y
                .saturating_sub(start_cursor_position.y),
        ),
    )
}

fn show_about_slint(slot: &SlintAboutSlot) -> Result<(), String> {
    if let Some(window) = slot.borrow().as_ref() {
        return window.show().map_err(|error| error.to_string());
    }

    let window = SlintAboutWindow::new().map_err(|error| error.to_string())?;
    window
        .window()
        .on_close_requested(|| CloseRequestResponse::HideWindow);
    window.show().map_err(|error| error.to_string())?;
    *slot.borrow_mut() = Some(window);
    Ok(())
}

fn install_status_refresh(
    main_window: &AppWindow,
    tray: &AppTray,
    controller: Rc<RefCell<Controller>>,
    provider: Arc<WindowsContextProvider>,
) -> Timer {
    let main_window_weak = main_window.as_weak();
    let tray_weak = tray.as_weak();
    let timer = Timer::default();
    timer.start(TimerMode::Repeated, STATUS_POLL_INTERVAL, move || {
        let settings = controller.borrow().settings.clone();
        let context = provider.current();
        let (status, healthy) = status_for_context(&settings, &context);
        if let Some(window) = main_window_weak.upgrade() {
            window.set_status_text(status.clone().into());
            window.set_status_healthy(healthy);
        }
        if let Some(tray) = tray_weak.upgrade() {
            tray.set_status_text(status.into());
            tray.set_protection_enabled(settings.enabled);
        }
    });
    timer
}

fn mark_unsaved(main_window: &slint::Weak<AppWindow>) {
    if let Some(window) = main_window.upgrade() {
        window.set_has_unsaved_settings(true);
        set_save_status(&window, "更改尚未保存，点击保存后生效", false);
    }
}

fn render_settings(window: &AppWindow, settings: &AppSettings, selected_chat_id: Option<&str>) {
    window.set_application_version(APPLICATION_VERSION.into());
    window.set_protection_enabled(settings.enabled);
    window.set_rule_mode(rule_mode_to_ui(settings.rule_mode));
    window.set_confirmation_mode(confirmation_mode_to_ui(settings.confirmation.mode));
    window.set_hold_milliseconds(settings.confirmation.hold_milliseconds.to_string().into());
    window.set_confirmation_phrase(settings.confirmation.phrase.clone().into());
    window.set_timeout_seconds(settings.confirmation.timeout_seconds.to_string().into());
    window.set_intercept_keyboard_enter(settings.intercept_keyboard_enter);
    window.set_intercept_numpad_enter(settings.intercept_numpad_enter);
    window.set_unknown_context_behavior(unknown_behavior_to_ui(settings.unknown_context_behavior));
    window.set_start_with_windows(settings.start_with_windows);
    window.set_log_retention_days(settings.log_retention_days.to_string().into());
    window.set_weixin_executable_path(
        settings
            .trusted_weixin_executable_path
            .as_deref()
            .unwrap_or(TRUSTED_WEIXIN_PATH)
            .into(),
    );
    window.set_has_unsaved_settings(false);
    render_chat_list(window, settings, selected_chat_id);
}

fn render_chat_list(window: &AppWindow, settings: &AppSettings, selected_chat_id: Option<&str>) {
    let chats = if settings.rule_mode == RuleMode::ConfirmUnlessExcluded {
        window.set_list_title("免确认放行的白名单".into());
        window.set_list_description("名单内会话直接发送；其他可识别会话需要确认".into());
        &settings.exempted_chats
    } else {
        window.set_list_title("需要二次确认的会话".into());
        window.set_list_description("名单修改会立即生效".into());
        &settings.protected_chats
    };
    let selected_chat_id = selected_chat_id
        .filter(|id| chats.iter().any(|chat| chat.id.to_string() == *id))
        .unwrap_or_default();
    let rows = chats.iter().map(chat_row).collect::<Vec<_>>();
    window.set_active_chats(ModelRc::new(Rc::new(VecModel::from(rows))));
    window.set_selected_chat_id(selected_chat_id.into());
    let aliases = chats
        .iter()
        .find(|chat| chat.id.to_string() == selected_chat_id)
        .map(|chat| chat.aliases.join(" | "))
        .unwrap_or_default();
    window.set_alias_text(aliases.into());
}

fn chat_row(chat: &ProtectedChat) -> ChatRow {
    let detail = if chat.aliases.is_empty() {
        format!("匹配标题：{}", chat.match_title)
    } else {
        format!(
            "匹配标题：{} · 别名：{}",
            chat.match_title,
            chat.aliases.join("、")
        )
    };
    ChatRow {
        id: chat.id.to_string().into(),
        display_name: chat.display_name.clone().into(),
        detail: detail.into(),
        kind_label: target_kind_label(chat.target_kind).into(),
        enabled: chat.enabled,
    }
}

fn save_form_settings(window: &AppWindow, controller: &Rc<RefCell<Controller>>) {
    let settings = match settings_from_form(window, &controller.borrow().settings) {
        Ok(settings) => settings,
        Err(error) => {
            set_save_status(window, error, true);
            return;
        }
    };
    let mut controller = controller.borrow_mut();
    match controller.apply_settings(settings) {
        Ok(()) => {
            let settings = controller.settings.clone();
            let selected = window.get_selected_chat_id().to_string();
            drop(controller);
            render_settings(window, &settings, Some(&selected));
            set_save_status(window, "设置已保存并即时生效", false);
        }
        Err(error) => set_save_status(window, error, true),
    }
}

fn settings_from_form(window: &AppWindow, current: &AppSettings) -> Result<AppSettings, String> {
    let mut settings = current.clone();
    settings.enabled = window.get_protection_enabled();
    settings.rule_mode = rule_mode_from_ui(window.get_rule_mode()).ok_or("无效的名单模式。")?;
    settings.confirmation.mode =
        confirmation_mode_from_ui(window.get_confirmation_mode()).ok_or("无效的确认方式。")?;
    settings.confirmation.hold_milliseconds =
        parse_bounded_u32(&window.get_hold_milliseconds(), 500, 3_000, "长按时长")?;
    settings.confirmation.phrase = window.get_confirmation_phrase().trim().to_owned();
    settings.confirmation.timeout_seconds =
        parse_bounded_u32(&window.get_timeout_seconds(), 1, 30, "自动取消时间")?;
    settings.unknown_context_behavior =
        unknown_behavior_from_ui(window.get_unknown_context_behavior())
            .ok_or("无效的未知会话处理方式。")?;
    settings.intercept_keyboard_enter = window.get_intercept_keyboard_enter();
    settings.intercept_numpad_enter =
        settings.intercept_keyboard_enter && window.get_intercept_numpad_enter();
    settings.shift_enter_pass_through = true;
    settings.start_with_windows = window.get_start_with_windows();
    settings.log_retention_days =
        parse_bounded_u32(&window.get_log_retention_days(), 1, 30, "日志保留天数")?;
    settings.trusted_weixin_executable_path =
        weixin_executable_path_override_from_form(&window.get_weixin_executable_path())?;
    update_selected_aliases(
        &mut settings,
        &window.get_selected_chat_id(),
        &window.get_alias_text(),
    );
    Ok(settings.sanitize())
}

fn add_current_chat(window: &AppWindow, controller: &Rc<RefCell<Controller>>) {
    let mut controller = controller.borrow_mut();
    let context = match controller.provider.refresh_now() {
        Ok(context) => context,
        Err(error) => {
            set_save_status(window, format!("无法读取当前会话：{error}"), true);
            return;
        }
    };
    let Some(target_kind) = context.target_kind() else {
        set_save_status(
            window,
            "请先在微信中打开可识别的群聊或联系人，并将光标放入消息输入框。",
            true,
        );
        return;
    };
    let title = context.normalized_chat_title();
    if !context.is_trusted_weixin
        || !context.is_compatibility_available
        || !context.is_message_editor_focused
        || title.is_empty()
    {
        set_save_status(
            window,
            "请先在微信中打开可识别的群聊或联系人，并将光标放入消息输入框。",
            true,
        );
        return;
    }
    if controller
        .active_chats()
        .iter()
        .any(|chat| chat.target_kind == target_kind && chat_matches_title(chat, &title))
    {
        set_save_status(window, "该会话已经在当前名单中。", false);
        return;
    }

    let mut settings = controller.settings.clone();
    let chat = ProtectedChat {
        display_name: title.clone(),
        match_title: title.clone(),
        target_kind,
        ..ProtectedChat::default()
    };
    if settings.rule_mode == RuleMode::ConfirmUnlessExcluded {
        settings.exempted_chats.push(chat.clone());
    } else {
        settings.protected_chats.push(chat.clone());
    }
    match controller.apply_settings(settings) {
        Ok(()) => {
            let settings = controller.settings.clone();
            drop(controller);
            render_chat_list(window, &settings, Some(&chat.id.to_string()));
            set_save_status(window, "会话名单已立即生效", false);
        }
        Err(error) => set_save_status(window, error, true),
    }
}

fn remove_selected_chat(window: &AppWindow, controller: &Rc<RefCell<Controller>>) {
    let Ok(id) = Uuid::parse_str(window.get_selected_chat_id().as_str()) else {
        return;
    };
    let mut controller = controller.borrow_mut();
    let mut settings = controller.settings.clone();
    if settings.rule_mode == RuleMode::ConfirmUnlessExcluded {
        settings.exempted_chats.retain(|chat| chat.id != id);
    } else {
        settings.protected_chats.retain(|chat| chat.id != id);
    }
    match controller.apply_settings(settings) {
        Ok(()) => {
            let settings = controller.settings.clone();
            drop(controller);
            render_chat_list(window, &settings, None);
            set_save_status(window, "会话名单已立即生效", false);
        }
        Err(error) => set_save_status(window, error, true),
    }
}

fn import_active_list(window: &AppWindow, controller: &Rc<RefCell<Controller>>) {
    let path = match select_protected_chat_import() {
        Ok(Some(path)) => path,
        Ok(None) => return,
        Err(error) => {
            set_save_status(window, format!("打开导入窗口失败：{error}"), true);
            return;
        }
    };
    let contents = match fs::read_to_string(&path) {
        Ok(contents) => contents,
        Err(error) => {
            set_save_status(window, format!("读取导入文件失败：{error}"), true);
            return;
        }
    };
    let chats = match import_protected_chats(&contents) {
        Ok(chats) => chats,
        Err(error) => {
            set_save_status(window, format!("导入文件无效：{error}"), true);
            return;
        }
    };
    let mut controller = controller.borrow_mut();
    let mut settings = controller.settings.clone();
    if settings.rule_mode == RuleMode::ConfirmUnlessExcluded {
        settings.exempted_chats = chats;
    } else {
        settings.protected_chats = chats;
    }
    match controller.apply_settings(settings) {
        Ok(()) => {
            let settings = controller.settings.clone();
            drop(controller);
            render_chat_list(window, &settings, None);
            set_save_status(window, "会话名单已立即导入并生效", false);
        }
        Err(error) => set_save_status(window, error, true),
    }
}

fn export_active_list(window: &AppWindow, controller: &Rc<RefCell<Controller>>) {
    let (default_name, payload) = {
        let controller = controller.borrow();
        let default_name = if controller.settings.rule_mode == RuleMode::ConfirmUnlessExcluded {
            "exempted-chats.json"
        } else {
            "protected-chats.json"
        };
        let payload = match export_protected_chats(controller.active_chats().iter().cloned()) {
            Ok(payload) => payload,
            Err(error) => {
                set_save_status(window, format!("无法导出名单：{error}"), true);
                return;
            }
        };
        (default_name, payload)
    };
    let path = match select_protected_chat_export(default_name) {
        Ok(Some(path)) => path,
        Ok(None) => return,
        Err(error) => {
            set_save_status(window, format!("打开导出窗口失败：{error}"), true);
            return;
        }
    };
    match fs::write(path, payload) {
        Ok(()) => set_save_status(window, "会话名单已导出", false),
        Err(error) => set_save_status(window, format!("写入导出文件失败：{error}"), true),
    }
}

fn show_confirmation(
    main_window: &AppWindow,
    controller: &Rc<RefCell<Controller>>,
    active_slot: &ActiveConfirmationSlot,
    attempt_id: &str,
) -> Result<(), String> {
    if active_slot.borrow().is_some() {
        return Ok(());
    }
    let service = controller.borrow().service.clone();
    let Some(pending) = service.current_pending_confirmation() else {
        return Ok(());
    };
    if pending.attempt_id.to_string() != attempt_id {
        return Ok(());
    }
    let confirmation_settings = service.settings().confirmation;
    let confirmation = ConfirmationWindow::new().map_err(|error| error.to_string())?;
    configure_confirmation_window(&confirmation, &pending, &confirmation_settings);

    let active_weak = Rc::downgrade(active_slot);
    let main_window_weak = main_window.as_weak();
    confirmation.on_cancelled({
        let active_weak = active_weak.clone();
        let service = service.clone();
        let main_window_weak = main_window_weak.clone();
        move || {
            finish_confirmation(
                &active_weak,
                service.clone(),
                main_window_weak.clone(),
                ConfirmationOutcome::Cancelled,
            )
        }
    });
    confirmation.on_confirmed({
        let active_weak = active_weak.clone();
        let service = service.clone();
        let main_window_weak = main_window_weak.clone();
        move || {
            finish_confirmation(
                &active_weak,
                service.clone(),
                main_window_weak.clone(),
                ConfirmationOutcome::Confirmed,
            )
        }
    });
    let close_weak = confirmation.as_weak();
    confirmation.window().on_close_requested(move || {
        if let Some(confirmation) = close_weak.upgrade() {
            confirmation.invoke_cancelled();
        }
        CloseRequestResponse::HideWindow
    });

    let timeout_timer = start_timeout_timer(
        &confirmation,
        &pending,
        Rc::downgrade(active_slot),
        service.clone(),
        main_window_weak.clone(),
    );
    let hold_timer = start_hold_timer(&confirmation, confirmation_settings.hold_milliseconds);
    *active_slot.borrow_mut() = Some(ActiveConfirmation {
        window: confirmation.clone_strong(),
        pending: pending.clone(),
        _timeout_timer: timeout_timer,
        _hold_timer: hold_timer,
    });

    confirmation.show().map_err(|error| error.to_string())?;
    position_confirmation_over_target(&confirmation, &pending);
    confirmation.invoke_focus_default();
    schedule_confirmation_focus(&confirmation);
    enrich_confirmation_preview(confirmation.as_weak(), service, pending);
    Ok(())
}

fn configure_confirmation_window(
    window: &ConfirmationWindow,
    pending: &PendingConfirmation,
    settings: &wechat_send_guard_core::ConfirmationSettings,
) {
    let target_name = match pending.decision.kind {
        wechat_send_guard_core::ProtectionDecisionKind::ConfirmProtected => pending
            .decision
            .protected_chat
            .as_ref()
            .map(|chat| chat.display_name.clone())
            .or_else(|| pending.original_context.chat_title.clone())
            .unwrap_or_else(|| "当前会话".to_owned()),
        wechat_send_guard_core::ProtectionDecisionKind::ConfirmUnlisted => pending
            .original_context
            .chat_title
            .clone()
            .unwrap_or_else(|| "当前会话".to_owned()),
        _ => "无法验证当前会话".to_owned(),
    };
    let target_kind = pending
        .original_context
        .target_kind()
        .map(target_kind_label)
        .unwrap_or("未知会话");
    let confirm_label = match settings.mode {
        ConfirmationMode::Click | ConfirmationMode::Phrase => "确认发送".to_owned(),
        ConfirmationMode::Hold => {
            format!(
                "按住确认 ({:.1}s)",
                settings.hold_milliseconds as f64 / 1_000.0
            )
        }
    };
    window.set_target_kind(target_kind.into());
    window.set_target_name(target_name.into());
    window.set_confirmation_mode(confirmation_mode_to_ui(settings.mode));
    window.set_hold_milliseconds(settings.hold_milliseconds as i32);
    window.set_required_phrase(settings.phrase.clone().into());
    window.set_phrase_value("".into());
    window.set_confirm_label(confirm_label.into());
    window.set_hold_progress(0.0);
    window.set_countdown_progress(100.0);
}

fn position_confirmation_over_target(
    confirmation: &ConfirmationWindow,
    pending: &PendingConfirmation,
) {
    let popup_size = confirmation.window().size();
    if let Some((x, y)) = center_popup_over_window(
        pending.original_context.window_handle,
        popup_size.width,
        popup_size.height,
    ) {
        confirmation
            .window()
            .set_position(slint::PhysicalPosition::new(x, y));
    }
}

fn schedule_confirmation_focus(confirmation: &ConfirmationWindow) {
    let confirmation_weak = confirmation.as_weak();
    Timer::single_shot(Duration::ZERO, move || {
        let Some(confirmation) = confirmation_weak.upgrade() else {
            return;
        };
        #[cfg(windows)]
        if let Some(window_handle) = native_window_handle(confirmation.window()) {
            activate_window(window_handle);
        }
        confirmation.invoke_focus_default();
    });
}

#[cfg(windows)]
fn native_window_handle(window: &slint::Window) -> Option<isize> {
    let handle = window.window_handle();
    match handle.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(handle) => Some(handle.hwnd.get()),
        _ => None,
    }
}

fn start_timeout_timer(
    confirmation: &ConfirmationWindow,
    pending: &PendingConfirmation,
    active_weak: ActiveConfirmationWeak,
    service: Arc<GuardService>,
    main_window_weak: slint::Weak<AppWindow>,
) -> Timer {
    let timer = Timer::default();
    let confirmation_weak = confirmation.as_weak();
    let expires_at = pending.expires_at;
    let total = pending
        .expires_at
        .duration_since(pending.created_at)
        .unwrap_or(Duration::from_secs(1));
    timer.start(TimerMode::Repeated, CONFIRMATION_TICK_INTERVAL, move || {
        let Some(confirmation) = confirmation_weak.upgrade() else {
            return;
        };
        let remaining = expires_at
            .duration_since(SystemTime::now())
            .unwrap_or(Duration::ZERO);
        let percent = (remaining.as_secs_f32() / total.as_secs_f32() * 100.0).clamp(0.0, 100.0);
        confirmation.set_countdown_progress(percent);
        confirmation
            .set_countdown_text(format!("{:.1} 秒后自动取消", remaining.as_secs_f32()).into());
        if remaining.is_zero() {
            finish_confirmation(
                &active_weak,
                service.clone(),
                main_window_weak.clone(),
                ConfirmationOutcome::TimedOut,
            );
        }
    });
    timer
}

fn start_hold_timer(confirmation: &ConfirmationWindow, hold_milliseconds: u32) -> Timer {
    let timer = Timer::default();
    let confirmation_weak = confirmation.as_weak();
    let holding_since = Rc::new(RefCell::new(None::<Instant>));
    timer.start(TimerMode::Repeated, HOLD_TICK_INTERVAL, move || {
        let Some(confirmation) = confirmation_weak.upgrade() else {
            return;
        };
        if !confirmation.get_hold_pressed() {
            *holding_since.borrow_mut() = None;
            confirmation.set_hold_progress(0.0);
            return;
        }
        let now = Instant::now();
        let started_at = *holding_since.borrow_mut().get_or_insert(now);
        let elapsed = now.saturating_duration_since(started_at);
        let progress =
            (elapsed.as_secs_f32() * 1_000.0 / hold_milliseconds.max(1) as f32).clamp(0.0, 1.0);
        confirmation.set_hold_progress(progress);
        if progress >= 1.0 {
            confirmation.invoke_confirmed();
        }
    });
    timer
}

fn enrich_confirmation_preview(
    confirmation_weak: slint::Weak<ConfirmationWindow>,
    service: Arc<GuardService>,
    pending: PendingConfirmation,
) {
    thread::spawn(move || {
        let pending = service.enrich_pending_confirmation(&pending);
        let preview = pending.draft_preview.unwrap_or_default();
        let _ = confirmation_weak.upgrade_in_event_loop(move |confirmation| {
            confirmation.set_draft_preview(preview.into());
        });
    });
}

fn finish_confirmation(
    active_weak: &ActiveConfirmationWeak,
    service: Arc<GuardService>,
    main_window_weak: slint::Weak<AppWindow>,
    outcome: ConfirmationOutcome,
) {
    let Some(active_slot) = active_weak.upgrade() else {
        return;
    };
    let Some(active) = active_slot.borrow_mut().take() else {
        return;
    };
    let _ = active.window.hide();
    let pending = active.pending;
    if outcome != ConfirmationOutcome::Confirmed {
        let result = service.complete_confirmation(&pending, outcome, SystemTime::now());
        show_completion_status(&main_window_weak, result);
        return;
    }

    thread::spawn(move || {
        let result = service.complete_confirmation(&pending, outcome, SystemTime::now());
        let _ = main_window_weak.upgrade_in_event_loop(move |window| {
            show_completion_status_for_window(&window, result);
        });
    });
}

fn show_completion_status(main_window_weak: &slint::Weak<AppWindow>, result: CompletionResult) {
    if let Some(window) = main_window_weak.upgrade() {
        show_completion_status_for_window(&window, result);
    }
}

fn show_completion_status_for_window(window: &AppWindow, result: CompletionResult) {
    match result {
        CompletionResult::Injected => set_save_status(window, "已确认并发送", false),
        CompletionResult::NotInjected { reason }
            if reason == "Cancelled" || reason == "TimedOut" =>
        {
            set_save_status(window, "发送已取消", false)
        }
        CompletionResult::NotInjected { reason } => {
            set_save_status(window, format!("未发送：{reason}"), true)
        }
    }
}

fn update_selected_aliases(settings: &mut AppSettings, selected_id: &str, aliases: &str) {
    let Ok(selected_id) = Uuid::parse_str(selected_id) else {
        return;
    };
    let aliases = aliases
        .split(['|', ',', '\n', '\r'])
        .map(normalize_title)
        .filter(|alias| !alias.is_empty())
        .collect::<Vec<_>>();
    let chats = if settings.rule_mode == RuleMode::ConfirmUnlessExcluded {
        &mut settings.exempted_chats
    } else {
        &mut settings.protected_chats
    };
    if let Some(chat) = chats.iter_mut().find(|chat| chat.id == selected_id) {
        chat.aliases = aliases;
    }
}

fn weixin_executable_path_override_from_form(value: &str) -> Result<Option<String>, String> {
    let value = value.trim().trim_matches('"').trim();
    if value.is_empty() {
        return Err("微信客户端路径不能为空；如需使用默认安装位置，请点击“恢复默认”。".to_owned());
    }

    if !is_valid_weixin_executable_path(value) {
        return Err("微信客户端路径必须是绝对路径，且必须指向 Weixin.exe。".to_owned());
    }

    if path_matches_trusted_weixin(value) {
        Ok(None)
    } else {
        Ok(Some(value.to_owned()))
    }
}

fn parse_bounded_u32(value: &str, minimum: u32, maximum: u32, label: &str) -> Result<u32, String> {
    let parsed = value
        .trim()
        .parse::<u32>()
        .map_err(|_| format!("{label}必须是 {minimum} 到 {maximum} 之间的整数。"))?;
    if !(minimum..=maximum).contains(&parsed) {
        return Err(format!("{label}必须是 {minimum} 到 {maximum} 之间的整数。"));
    }
    Ok(parsed)
}

fn set_save_status(window: &AppWindow, message: impl Into<slint::SharedString>, is_error: bool) {
    window.set_save_status(message.into());
    window.set_save_status_error(is_error);
}

fn status_for_context(settings: &AppSettings, context: &ChatContext) -> (String, bool) {
    if !settings.enabled {
        return ("状态：守护已暂停".to_owned(), false);
    }
    if context.requires_elevation {
        return (
            "状态：微信以管理员权限运行，无法读取其窗口".to_owned(),
            false,
        );
    }
    if !context.is_trusted_weixin {
        return ("状态：等待微信成为前台窗口".to_owned(), false);
    }
    if !context.is_compatibility_available {
        return ("状态：当前微信界面不可识别".to_owned(), false);
    }
    let title = context.normalized_chat_title();
    if context.is_message_editor_focused {
        let suffix = if title.is_empty() {
            "已就绪".to_owned()
        } else {
            title
        };
        return (format!("状态：守护中（{suffix}）"), true);
    }
    let suffix = if title.is_empty() {
        String::new()
    } else {
        format!("（{title}）")
    };
    (format!("状态：等待消息输入框焦点{suffix}"), false)
}

fn chat_matches_title(chat: &ProtectedChat, title: &str) -> bool {
    chat.match_title == title || chat.aliases.iter().any(|alias| alias == title)
}

fn target_kind_label(target_kind: ChatTargetKind) -> &'static str {
    match target_kind {
        ChatTargetKind::Group => "群聊",
        ChatTargetKind::Contact => "联系人",
    }
}

const fn confirmation_mode_to_ui(mode: ConfirmationMode) -> i32 {
    match mode {
        ConfirmationMode::Click => 0,
        ConfirmationMode::Hold => 1,
        ConfirmationMode::Phrase => 2,
    }
}

const fn confirmation_mode_from_ui(mode: i32) -> Option<ConfirmationMode> {
    match mode {
        0 => Some(ConfirmationMode::Click),
        1 => Some(ConfirmationMode::Hold),
        2 => Some(ConfirmationMode::Phrase),
        _ => None,
    }
}

const fn rule_mode_to_ui(mode: RuleMode) -> i32 {
    match mode {
        RuleMode::ProtectListed => 0,
        RuleMode::ConfirmUnlessExcluded => 1,
    }
}

const fn rule_mode_from_ui(mode: i32) -> Option<RuleMode> {
    match mode {
        0 => Some(RuleMode::ProtectListed),
        1 => Some(RuleMode::ConfirmUnlessExcluded),
        _ => None,
    }
}

const fn unknown_behavior_to_ui(behavior: UnknownContextBehavior) -> i32 {
    match behavior {
        UnknownContextBehavior::Confirm => 0,
        UnknownContextBehavior::Block => 1,
    }
}

const fn unknown_behavior_from_ui(behavior: i32) -> Option<UnknownContextBehavior> {
    match behavior {
        0 => Some(UnknownContextBehavior::Confirm),
        1 => Some(UnknownContextBehavior::Block),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        is_background_start_argument, parse_bounded_u32, status_for_context,
        weixin_executable_path_override_from_form, window_position_for_cursor_drag,
    };
    use slint::PhysicalPosition;
    use wechat_send_guard_core::{AppSettings, ChatContext};

    #[test]
    fn bounded_integer_parser_rejects_invalid_settings_before_persistence() {
        assert_eq!(parse_bounded_u32("800", 500, 3_000, "长按时长"), Ok(800));
        assert!(parse_bounded_u32("499", 500, 3_000, "长按时长").is_err());
        assert!(parse_bounded_u32("eight", 500, 3_000, "长按时长").is_err());
    }

    #[test]
    fn weixin_executable_path_uses_default_only_when_it_matches_the_supported_path() {
        assert_eq!(
            weixin_executable_path_override_from_form(
                r"c:/PROGRAM FILES/Tencent/Weixin/Weixin.exe"
            ),
            Ok(None)
        );
        assert_eq!(
            weixin_executable_path_override_from_form(r"D:\Apps\Weixin\Weixin.exe"),
            Ok(Some(r"D:\Apps\Weixin\Weixin.exe".to_owned()))
        );
        assert!(weixin_executable_path_override_from_form(r"Weixin.exe").is_err());
        assert!(weixin_executable_path_override_from_form(r"D:\Apps\Weixin\other.exe").is_err());
    }

    #[test]
    fn status_is_not_healthy_when_context_is_untrusted() {
        let (status, healthy) =
            status_for_context(&AppSettings::default(), &ChatContext::default());
        assert!(!healthy);
        assert!(status.contains("等待微信"));
    }

    #[test]
    fn legacy_background_start_aliases_remain_supported() {
        for argument in ["--silent", "--startup", "--background", "--SILENT"] {
            assert!(is_background_start_argument(argument));
        }
        assert!(!is_background_start_argument("--foreground"));
    }

    #[test]
    fn caption_drag_uses_screen_coordinates_instead_of_moving_local_coordinates() {
        assert_eq!(
            window_position_for_cursor_drag(
                PhysicalPosition::new(300, 120),
                PhysicalPosition::new(600, 400),
                PhysicalPosition::new(648, 365),
            ),
            PhysicalPosition::new(348, 85),
        );
    }
}
