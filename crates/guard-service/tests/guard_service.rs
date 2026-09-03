use std::{
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use wechat_send_guard_core::{
    AppSettings, ChatContext, ChatTargetKind, ConfirmationOutcome, ProtectedChat, RuleMode,
};
use wechat_send_guard_platform_api::{
    FakeChatContextProvider, RecordingAuditLog, RecordingInputInjector,
};
use wechat_send_guard_service::{
    CompletionResult, EnterHandling, GuardService, PhysicalEnter, TemporaryBypassRejection,
};

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
        modifier_pressed: false,
        foreground_window: 42,
    }
}

fn protected_button_settings() -> AppSettings {
    AppSettings {
        protected_chats: vec![ProtectedChat {
            match_title: "工作群".into(),
            target_kind: ChatTargetKind::Group,
            ..ProtectedChat::default()
        }],
        ..AppSettings::default()
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
fn confirmation_preview_is_read_after_suppression_and_traced_without_logging_content() {
    let platform = Arc::new(FakeChatContextProvider::new(protected_context("工作群")));
    platform.set_draft_preview(Some("仅测试预览".to_owned()));
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
        Arc::new(RecordingInputInjector::default()),
        audit.clone(),
    );

    let EnterHandling::SuppressAndConfirm(pending) =
        service.handle_physical_enter(physical_enter(), fixed_now())
    else {
        panic!("protected physical enter should request confirmation");
    };
    let enriched = service.enrich_pending_confirmation(&pending);

    assert_eq!(enriched.draft_preview.as_deref(), Some("仅测试预览"));
    let entry = audit
        .entries()
        .into_iter()
        .find(|entry| entry.event_type == "confirmation-preview")
        .expect("preview audit entry");
    assert_eq!(entry.result, "available");
    assert_eq!(entry.trace_id, Some(pending.trace_id));
    assert!(entry.details.is_empty());
}

#[test]
fn confirmation_accepts_a_revalidation_completed_after_the_call_timestamp() {
    let mut context = protected_context("工作群");
    context.observed_at = Some(fixed_now() + Duration::from_secs(2));
    let platform = Arc::new(FakeChatContextProvider::new(context));
    let injector = Arc::new(RecordingInputInjector::default());
    let service = GuardService::new(
        protected_button_settings(),
        platform,
        injector.clone(),
        Arc::new(RecordingAuditLog::default()),
    );

    let EnterHandling::SuppressAndConfirm(pending) =
        service.handle_physical_enter(physical_enter(), fixed_now())
    else {
        panic!("protected physical enter should request confirmation");
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
}

#[test]
fn send_button_click_uses_the_same_confirmation_and_injection_path_as_enter() {
    let context = protected_context("工作群");
    let platform = Arc::new(FakeChatContextProvider::new(context));
    let injector = Arc::new(RecordingInputInjector::default());
    let service = GuardService::new(
        protected_button_settings(),
        platform,
        injector.clone(),
        Arc::new(RecordingAuditLog::default()),
    );

    let EnterHandling::SuppressAndConfirm(pending) =
        service.handle_send_button_click(42, fixed_now())
    else {
        panic!("protected send-button click should be suppressed and confirmed");
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
}

#[test]
fn send_button_trace_is_preserved_through_confirmation_and_injection() {
    let context = protected_context("工作群");
    let platform = Arc::new(FakeChatContextProvider::new(context));
    let injector = Arc::new(RecordingInputInjector::default());
    let audit = Arc::new(RecordingAuditLog::default());
    let service = GuardService::new(
        protected_button_settings(),
        platform,
        injector,
        audit.clone(),
    );
    let trace_id = uuid::Uuid::new_v4();

    let EnterHandling::SuppressAndConfirm(pending) =
        service.handle_send_button_click_with_trace(42, trace_id, fixed_now())
    else {
        panic!("protected send-button click should request confirmation");
    };
    assert_eq!(pending.trace_id, trace_id);
    assert!(matches!(
        service.complete_confirmation(
            &pending,
            ConfirmationOutcome::Confirmed,
            fixed_now() + Duration::from_secs(1),
        ),
        CompletionResult::Injected
    ));
    assert!(
        audit
            .entries()
            .iter()
            .all(|entry| entry.trace_id == Some(trace_id))
    );
}

#[test]
fn disabled_send_button_strategy_passes_the_click_through() {
    let platform = Arc::new(FakeChatContextProvider::new(protected_context("工作群")));
    let service = GuardService::new(
        AppSettings {
            intercept_send_button: false,
            ..protected_button_settings()
        },
        platform,
        Arc::new(RecordingInputInjector::default()),
        Arc::new(RecordingAuditLog::default()),
    );

    assert_eq!(
        service.handle_send_button_click(42, fixed_now()),
        EnterHandling::PassThrough
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
fn shift_and_injected_events_always_pass_through_in_safe_tests() {
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
                modifier_pressed: true,
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
fn modified_enter_events_always_pass_through_in_protected_chats() {
    let platform = Arc::new(FakeChatContextProvider::new(protected_context("工作群")));
    let service = GuardService::new(
        protected_button_settings(),
        platform,
        Arc::new(RecordingInputInjector::default()),
        Arc::new(RecordingAuditLog::default()),
    );

    assert_eq!(
        service.handle_physical_enter(
            PhysicalEnter {
                modifier_pressed: true,
                ..physical_enter()
            },
            fixed_now()
        ),
        EnterHandling::PassThrough
    );
}

#[test]
fn temporary_pause_passes_all_send_paths_then_expires() {
    let platform = Arc::new(FakeChatContextProvider::new(protected_context("工作群")));
    let service = GuardService::new(
        protected_button_settings(),
        platform,
        Arc::new(RecordingInputInjector::default()),
        Arc::new(RecordingAuditLog::default()),
    );
    let start = fixed_now();

    assert!(service.try_pause(5, start));
    assert!(
        service
            .pause_remaining(start + Duration::from_secs(299))
            .is_some()
    );
    assert_eq!(
        service.handle_physical_enter(physical_enter(), start + Duration::from_secs(1)),
        EnterHandling::PassThrough
    );
    assert_eq!(
        service.handle_send_button_click(42, start + Duration::from_secs(1)),
        EnterHandling::PassThrough
    );
    assert!(
        service
            .pause_remaining(start + Duration::from_secs(300))
            .is_none()
    );
    assert!(matches!(
        service.handle_physical_enter(physical_enter(), start + Duration::from_secs(300)),
        EnterHandling::SuppressAndConfirm(_)
    ));
}

#[test]
fn temporary_pause_cancels_an_active_confirmation_and_rejects_late_confirmation() {
    let platform = Arc::new(FakeChatContextProvider::new(protected_context("工作群")));
    let injector = Arc::new(RecordingInputInjector::default());
    let service = GuardService::new(
        protected_button_settings(),
        platform,
        injector.clone(),
        Arc::new(RecordingAuditLog::default()),
    );
    let start = fixed_now();
    let EnterHandling::SuppressAndConfirm(pending) =
        service.handle_physical_enter(physical_enter(), start)
    else {
        panic!("protected physical enter should request confirmation");
    };

    assert!(service.try_pause(1, start));
    assert!(service.current_pending_confirmation().is_none());
    assert!(matches!(
        service.complete_confirmation(
            &pending,
            ConfirmationOutcome::Confirmed,
            start + Duration::from_secs(1),
        ),
        CompletionResult::NotInjected { .. }
    ));
    assert!(injector.sent().is_empty());
}

#[test]
fn temporary_pause_rejects_unknown_duration_without_changing_state() {
    let audit = Arc::new(RecordingAuditLog::default());
    let service = GuardService::new(
        protected_button_settings(),
        Arc::new(FakeChatContextProvider::new(protected_context("工作群"))),
        Arc::new(RecordingInputInjector::default()),
        audit.clone(),
    );

    assert!(!service.try_pause(2, fixed_now()));
    assert!(service.pause_remaining(fixed_now()).is_none());
    let entry = audit
        .entries()
        .into_iter()
        .find(|entry| entry.event_type == "protection-pause")
        .expect("invalid pause duration should be audited");
    assert_eq!(entry.result, "rejected-invalid-duration");
    assert!(entry.trace_id.is_some());
    assert_eq!(entry.details.get("requestedMinutes"), Some(&"2".to_owned()));
}

#[test]
fn temporary_pause_resume_and_expiry_are_traced() {
    let audit = Arc::new(RecordingAuditLog::default());
    let service = GuardService::new(
        protected_button_settings(),
        Arc::new(FakeChatContextProvider::new(protected_context("工作群"))),
        Arc::new(RecordingInputInjector::default()),
        audit.clone(),
    );
    let start = fixed_now();

    assert!(service.try_pause(1, start));
    assert!(service.resume_pause(start + Duration::from_secs(1)));
    let manual_resume = audit
        .entries()
        .into_iter()
        .find(|entry| entry.result == "resumed-manual")
        .expect("manual resume should be audited");
    assert!(manual_resume.trace_id.is_some());
    assert_eq!(
        manual_resume.details.get("source"),
        Some(&"tray-pause-resume".to_owned())
    );
    assert!(!manual_resume.details.contains_key("requestedMinutes"));

    assert!(service.try_pause(1, start + Duration::from_secs(2)));
    assert!(service.expire_pause(start + Duration::from_secs(62)));
    let timed_out_resume = audit
        .entries()
        .into_iter()
        .find(|entry| entry.result == "resumed-timeout")
        .expect("timeout resume should be audited");
    assert!(timed_out_resume.trace_id.is_some());
    assert_eq!(
        timed_out_resume.details.get("source"),
        Some(&"pause-expiry".to_owned())
    );
}

#[test]
fn temporary_bypass_is_unavailable_while_protection_is_paused() {
    let service = GuardService::new(
        protected_button_settings(),
        Arc::new(FakeChatContextProvider::new(protected_context("工作群"))),
        Arc::new(RecordingInputInjector::default()),
        Arc::new(RecordingAuditLog::default()),
    );
    let start = fixed_now();

    assert!(service.try_pause(1, start));
    assert_eq!(
        service.try_grant_current_bypass(1, start),
        Err(TemporaryBypassRejection::ProtectionPaused)
    );
    assert!(
        service
            .try_grant_current_bypass(1, start + Duration::from_secs(61))
            .is_ok()
    );
}

#[test]
fn temporary_bypass_allows_an_unlisted_chat_in_whitelist_mode_until_expiry() {
    let start = fixed_now();
    let context = protected_context("未加入白名单");
    let platform = Arc::new(FakeChatContextProvider::new(context.clone()));
    let audit = Arc::new(RecordingAuditLog::default());
    let service = GuardService::new(
        AppSettings {
            rule_mode: RuleMode::ConfirmUnlessExcluded,
            ..AppSettings::default()
        },
        platform,
        Arc::new(RecordingInputInjector::default()),
        audit.clone(),
    );

    let grant = service
        .try_grant_current_bypass(1, start)
        .expect("an unlisted recognized chat should be temporarily allowed");
    assert_eq!(grant.display_name, "未加入白名单");
    assert_eq!(
        service.temporary_bypass_remaining_for_context(&context, start + Duration::from_secs(1)),
        Some(Duration::from_secs(59))
    );
    assert_eq!(
        service.handle_physical_enter(physical_enter(), start + Duration::from_secs(59)),
        EnterHandling::PassThrough
    );
    assert!(matches!(
        service.handle_physical_enter(physical_enter(), start + Duration::from_secs(60)),
        EnterHandling::SuppressAndConfirm(_)
    ));
    assert_eq!(
        service.temporary_bypass_remaining_for_context(&context, start + Duration::from_secs(60)),
        None
    );
    let entry = audit
        .entries()
        .into_iter()
        .find(|entry| entry.event_type == "temporary-bypass")
        .expect("temporary bypass should be audited");
    assert_eq!(entry.result, "granted-1m");
    assert!(entry.trace_id.is_some());
    assert_eq!(
        entry.details.get("contextSource"),
        Some(&"foreground-snapshot".to_owned())
    );
    assert_eq!(
        entry.details.get("ruleMode"),
        Some(&"confirm-unless-excluded".to_owned())
    );
    assert_eq!(
        entry.details.get("decisionKind"),
        Some(&"confirm-unlisted".to_owned())
    );
}

#[test]
fn temporary_bypass_remaining_in_protect_list_mode_requires_the_current_chat() {
    let start = fixed_now();
    let context = protected_context("工作群");
    let service = GuardService::new(
        protected_button_settings(),
        Arc::new(FakeChatContextProvider::new(context.clone())),
        Arc::new(RecordingInputInjector::default()),
        Arc::new(RecordingAuditLog::default()),
    );

    service
        .try_grant_current_bypass(1, start)
        .expect("a protected chat should receive a temporary bypass");
    assert_eq!(
        service.temporary_bypass_remaining_for_context(&context, start + Duration::from_secs(1)),
        Some(Duration::from_secs(59))
    );
    assert_eq!(
        service.temporary_bypass_remaining_for_context(
            &protected_context("其他群"),
            start + Duration::from_secs(1),
        ),
        None
    );
}

#[test]
fn temporary_bypass_accepts_a_recent_tray_context_for_both_send_paths() {
    let start = fixed_now();
    let mut remembered_context = protected_context("未加入白名单");
    remembered_context.is_group_chat = false;
    remembered_context.is_contact_chat = true;
    remembered_context.observed_at = Some(start - Duration::from_millis(4_500));
    let platform = Arc::new(FakeChatContextProvider::new(remembered_context.clone()));
    let audit = Arc::new(RecordingAuditLog::default());
    let service = GuardService::new(
        AppSettings {
            rule_mode: RuleMode::ConfirmUnlessExcluded,
            ..AppSettings::default()
        },
        platform.clone(),
        Arc::new(RecordingInputInjector::default()),
        audit.clone(),
    );

    let grant = service
        .try_grant_bypass_for_context(remembered_context.clone(), 1, start)
        .expect("a recognized context within the tray action window should be allowed");
    assert_eq!(grant.display_name, "未加入白名单");

    let mut refreshed_context = remembered_context;
    refreshed_context.observed_at = Some(start);
    platform.set_current(refreshed_context);
    assert_eq!(
        service.handle_physical_enter(physical_enter(), start + Duration::from_secs(1)),
        EnterHandling::PassThrough
    );
    assert_eq!(
        service.handle_send_button_click(42, start + Duration::from_secs(1)),
        EnterHandling::PassThrough
    );

    let entry = audit
        .entries()
        .into_iter()
        .find(|entry| entry.event_type == "temporary-bypass")
        .expect("tray bypass should be audited");
    assert_eq!(entry.result, "granted-1m");
    assert_eq!(
        entry.details.get("contextAgeMilliseconds"),
        Some(&"4500".to_owned())
    );
    assert_eq!(
        entry.details.get("contextMaximumAgeMilliseconds"),
        Some(&"5000".to_owned())
    );
}

#[test]
fn temporary_bypass_rejects_a_context_after_the_tray_action_window() {
    let start = fixed_now();
    let mut context = protected_context("未加入白名单");
    context.observed_at = Some(start - Duration::from_millis(5_001));
    let audit = Arc::new(RecordingAuditLog::default());
    let service = GuardService::new(
        AppSettings {
            rule_mode: RuleMode::ConfirmUnlessExcluded,
            ..AppSettings::default()
        },
        Arc::new(FakeChatContextProvider::new(context.clone())),
        Arc::new(RecordingInputInjector::default()),
        audit.clone(),
    );

    assert_eq!(
        service.try_grant_bypass_for_context(context, 1, start),
        Err(TemporaryBypassRejection::ContextStale)
    );
    let entry = audit
        .entries()
        .into_iter()
        .find(|entry| entry.event_type == "temporary-bypass")
        .expect("rejected tray bypass should be audited");
    assert_eq!(entry.result, "rejected-context-stale");
    assert!(entry.trace_id.is_some());
    assert_eq!(
        entry.details.get("contextAgeMilliseconds"),
        Some(&"5001".to_owned())
    );
}

#[test]
fn temporary_bypass_uses_the_supplied_recognized_context_after_focus_changes() {
    let start = fixed_now();
    let remembered_context = protected_context("未加入白名单");
    let platform = Arc::new(FakeChatContextProvider::new(remembered_context.clone()));
    platform.set_current(ChatContext {
        is_message_editor_focused: false,
        ..remembered_context.clone()
    });
    let audit = Arc::new(RecordingAuditLog::default());
    let service = GuardService::new(
        AppSettings {
            rule_mode: RuleMode::ConfirmUnlessExcluded,
            ..AppSettings::default()
        },
        platform.clone(),
        Arc::new(RecordingInputInjector::default()),
        audit.clone(),
    );

    assert!(
        service
            .try_grant_bypass_for_context(remembered_context.clone(), 1, start)
            .is_ok()
    );
    platform.set_current(remembered_context);
    assert_eq!(
        service.handle_physical_enter(physical_enter(), start + Duration::from_secs(59)),
        EnterHandling::PassThrough
    );
    assert!(matches!(
        service.handle_physical_enter(physical_enter(), start + Duration::from_secs(60)),
        EnterHandling::SuppressAndConfirm(_)
    ));
    let entry = audit
        .entries()
        .into_iter()
        .find(|entry| entry.event_type == "temporary-bypass")
        .expect("supplied-context bypass should be audited");
    assert_eq!(
        entry.details.get("contextSource"),
        Some(&"provided-context".to_owned())
    );
}

#[test]
fn temporary_bypass_and_pause_are_reset_when_the_service_is_recreated() {
    let start = fixed_now();
    let settings = protected_button_settings();
    let platform = Arc::new(FakeChatContextProvider::new(protected_context("工作群")));

    let bypassed = GuardService::new(
        settings.clone(),
        platform.clone(),
        Arc::new(RecordingInputInjector::default()),
        Arc::new(RecordingAuditLog::default()),
    );
    assert!(bypassed.try_grant_current_bypass(5, start).is_ok());
    assert_eq!(
        bypassed.handle_physical_enter(physical_enter(), start + Duration::from_secs(1)),
        EnterHandling::PassThrough
    );

    let restarted_after_bypass = GuardService::new(
        settings.clone(),
        platform.clone(),
        Arc::new(RecordingInputInjector::default()),
        Arc::new(RecordingAuditLog::default()),
    );
    assert!(matches!(
        restarted_after_bypass
            .handle_physical_enter(physical_enter(), start + Duration::from_secs(1)),
        EnterHandling::SuppressAndConfirm(_)
    ));

    let paused = GuardService::new(
        settings.clone(),
        platform.clone(),
        Arc::new(RecordingInputInjector::default()),
        Arc::new(RecordingAuditLog::default()),
    );
    assert!(paused.try_pause(5, start));
    assert!(paused.pause_remaining(start).is_some());

    let restarted_after_pause = GuardService::new(
        settings,
        platform,
        Arc::new(RecordingInputInjector::default()),
        Arc::new(RecordingAuditLog::default()),
    );
    assert!(restarted_after_pause.pause_remaining(start).is_none());
    assert!(matches!(
        restarted_after_pause
            .handle_physical_enter(physical_enter(), start + Duration::from_secs(1)),
        EnterHandling::SuppressAndConfirm(_)
    ));
}

#[test]
fn unrelated_application_enters_are_not_written_to_the_diagnostic_log() {
    let platform = Arc::new(FakeChatContextProvider::new(ChatContext::inactive()));
    let audit = Arc::new(RecordingAuditLog::default());
    let service = GuardService::new(
        protected_button_settings(),
        platform,
        Arc::new(RecordingInputInjector::default()),
        audit.clone(),
    );

    assert_eq!(
        service.handle_physical_enter(physical_enter(), fixed_now()),
        EnterHandling::PassThrough
    );
    assert!(audit.entries().is_empty());
}

#[test]
fn stale_production_snapshot_requests_confirmation_then_rejects_stale_revalidation() {
    let mut context = protected_context("工作群");
    context.observed_at = Some(fixed_now() - Duration::from_millis(2_501));
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

    let EnterHandling::SuppressAndConfirm(pending) =
        service.handle_physical_enter(physical_enter(), fixed_now())
    else {
        panic!("a stale known protected chat should still surface confirmation");
    };
    assert!(matches!(
        service.complete_confirmation(
            &pending,
            ConfirmationOutcome::Confirmed,
            fixed_now() + Duration::from_secs(1),
        ),
        CompletionResult::NotInjected { .. }
    ));
    assert!(injector.sent().is_empty());
    assert_eq!(
        audit.entries().last().expect("audit entry").result,
        "cancelled-stale-revalidation"
    );
}

#[test]
fn stale_snapshot_cannot_pass_through_an_old_unprotected_chat() {
    let mut context = protected_context("普通群");
    context.observed_at = Some(fixed_now() - Duration::from_millis(2_501));
    let audit = Arc::new(RecordingAuditLog::default());
    let service = GuardService::new(
        protected_button_settings(),
        Arc::new(FakeChatContextProvider::new(context)),
        Arc::new(RecordingInputInjector::default()),
        audit.clone(),
    );

    assert_eq!(
        service.handle_physical_enter(physical_enter(), fixed_now()),
        EnterHandling::SuppressBlockedUnknown
    );
    assert_eq!(
        audit.entries().last().expect("audit entry").result,
        "stale-context"
    );
}

#[test]
fn trusted_but_incomplete_context_consumes_enter_until_recognition_recovers() {
    let context = ChatContext {
        window_handle: 42,
        is_trusted_weixin: true,
        is_compatibility_available: false,
        ..ChatContext::default()
    };
    let service = GuardService::new(
        protected_button_settings(),
        Arc::new(FakeChatContextProvider::new(context)),
        Arc::new(RecordingInputInjector::default()),
        Arc::new(RecordingAuditLog::default()),
    );

    assert_eq!(
        service.handle_physical_enter(physical_enter(), fixed_now()),
        EnterHandling::SuppressBlockedUnknown
    );
}

#[test]
fn send_button_candidate_with_incomplete_context_is_consumed() {
    let context = ChatContext {
        window_handle: 42,
        is_trusted_weixin: true,
        is_compatibility_available: false,
        ..ChatContext::default()
    };
    let service = GuardService::new(
        protected_button_settings(),
        Arc::new(FakeChatContextProvider::new(context)),
        Arc::new(RecordingInputInjector::default()),
        Arc::new(RecordingAuditLog::default()),
    );

    assert_eq!(
        service.handle_send_button_click(42, fixed_now()),
        EnterHandling::SuppressBlockedUnknown
    );
}
