use std::{
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use wechat_send_guard_core::{
    AppSettings, ChatContext, ChatTargetKind, ConfirmationOutcome, ProtectedChat,
};
use wechat_send_guard_platform_api::{
    FakeChatContextProvider, RecordingAuditLog, RecordingInputInjector,
};
use wechat_send_guard_service::{CompletionResult, EnterHandling, GuardService, PhysicalEnter};

fn fixed_now() -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(1_787_011_200)
}

fn protected_context(title: &str) -> ChatContext {
    ChatContext {
        window_handle: 42,
        process_id: 7,
        is_trusted_weixin: true,
        is_compatibility_available: true,
        is_message_editor_focused: true,
        is_group_chat: true,
        chat_title: Some(title.to_owned()),
        ..ChatContext::default()
    }
}

fn physical_enter() -> PhysicalEnter {
    PhysicalEnter {
        is_numpad_enter: false,
        is_injected: false,
        shift_pressed: false,
        ime_composing: false,
        foreground_window: 42,
    }
}

#[test]
fn fake_platform_can_exercise_full_confirm_then_recorded_injection_flow() {
    let context = protected_context("工作群");
    let platform = Arc::new(FakeChatContextProvider::new(context));
    let injector = Arc::new(RecordingInputInjector::default());
    let audit = Arc::new(RecordingAuditLog::default());
    let service = GuardService::new(
        AppSettings {
            protected_chats: vec![ProtectedChat {
                match_title: "工作群".into(),
                target_kind: ChatTargetKind::Group,
                ..ProtectedChat::default()
            }],
            ..AppSettings::default()
        },
        platform,
        injector.clone(),
        audit.clone(),
    );

    let EnterHandling::SuppressAndConfirm(pending) =
        service.handle_physical_enter(physical_enter(), fixed_now())
    else {
        panic!("protected physical enter should be suppressed and confirmed");
    };
    assert!(matches!(
        service.complete_confirmation(
            &pending,
            ConfirmationOutcome::Confirmed,
            fixed_now() + Duration::from_secs(1),
        ),
        CompletionResult::Injected
    ));
    assert_eq!(injector.sent().len(), 1);
    assert_eq!(
        audit.entries().last().expect("audit entry").result,
        "injected"
    );
}

#[test]
fn fake_platform_rejects_changed_context_before_recording_any_input() {
    let context = protected_context("工作群");
    let platform = Arc::new(FakeChatContextProvider::new(context));
    let injector = Arc::new(RecordingInputInjector::default());
    let audit = Arc::new(RecordingAuditLog::default());
    let service = GuardService::new(
        AppSettings {
            protected_chats: vec![ProtectedChat {
                match_title: "工作群".into(),
                ..ProtectedChat::default()
            }],
            ..AppSettings::default()
        },
        platform.clone(),
        injector.clone(),
        audit,
    );

    let EnterHandling::SuppressAndConfirm(pending) =
        service.handle_physical_enter(physical_enter(), fixed_now())
    else {
        panic!("protected physical enter should be suppressed and confirmed");
    };
    platform.set_current(protected_context("另一个群"));
    assert!(matches!(
        service.complete_confirmation(
            &pending,
            ConfirmationOutcome::Confirmed,
            fixed_now() + Duration::from_secs(1),
        ),
        CompletionResult::NotInjected { .. }
    ));
    assert!(injector.sent().is_empty());
}

#[test]
fn shift_ime_and_injected_events_always_pass_through_in_safe_tests() {
    let platform = Arc::new(FakeChatContextProvider::new(protected_context("工作群")));
    let service = GuardService::new(
        AppSettings {
            protected_chats: vec![ProtectedChat {
                match_title: "工作群".into(),
                ..ProtectedChat::default()
            }],
            ..AppSettings::default()
        },
        platform,
        Arc::new(RecordingInputInjector::default()),
        Arc::new(RecordingAuditLog::default()),
    );

    assert_eq!(
        service.handle_physical_enter(
            PhysicalEnter {
                shift_pressed: true,
                ..physical_enter()
            },
            fixed_now()
        ),
        EnterHandling::PassThrough
    );
    assert_eq!(
        service.handle_physical_enter(
            PhysicalEnter {
                ime_composing: true,
                ..physical_enter()
            },
            fixed_now()
        ),
        EnterHandling::PassThrough
    );
    assert_eq!(
        service.handle_physical_enter(
            PhysicalEnter {
                is_injected: true,
                ..physical_enter()
            },
            fixed_now()
        ),
        EnterHandling::PassThrough
    );
}

#[test]
fn stale_production_snapshot_is_suppressed_without_recording_any_input() {
    let mut context = protected_context("工作群");
    context.observed_at = Some(fixed_now() - Duration::from_millis(251));
    let platform = Arc::new(FakeChatContextProvider::new(context));
    let injector = Arc::new(RecordingInputInjector::default());
    let audit = Arc::new(RecordingAuditLog::default());
    let service = GuardService::new(
        AppSettings {
            protected_chats: vec![ProtectedChat {
                match_title: "工作群".into(),
                ..ProtectedChat::default()
            }],
            ..AppSettings::default()
        },
        platform,
        injector.clone(),
        audit.clone(),
    );

    assert_eq!(
        service.handle_physical_enter(physical_enter(), fixed_now()),
        EnterHandling::SuppressBlockedUnknown
    );
    assert!(injector.sent().is_empty());
    assert_eq!(
        audit.entries().last().expect("audit entry").result,
        "stale-context"
    );
}
