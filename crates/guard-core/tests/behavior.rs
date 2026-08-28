use std::{
    fs,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use uuid::Uuid;
use wechat_send_guard_core::{
    AppSettings, CURRENT_SCHEMA_VERSION, ChatContext, ChatTargetKind, ConfirmationOutcome,
    ConfirmationSettings, FileSettingsStore, ProtectedChat, ProtectionDecision,
    ProtectionDecisionKind, RuleMode, SendGuardStateMachine, TemporaryBypassRegistry,
    UnknownContextBehavior, evaluate_protection, export_protected_chats, import_protected_chats,
    normalize_title,
};

fn fixed_now() -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(1_787_011_200)
}

fn group_editor(title: Option<&str>) -> ChatContext {
    ChatContext {
        window_handle: 42,
        process_id: 7,
        is_trusted_weixin: true,
        is_compatibility_available: true,
        is_message_editor_focused: true,
        is_group_chat: true,
        chat_title: title.map(ToOwned::to_owned),
        generation: 1,
        ..ChatContext::default()
    }
}

fn contact_editor(title: Option<&str>) -> ChatContext {
    ChatContext {
        is_group_chat: false,
        is_contact_chat: true,
        ..group_editor(title)
    }
}

#[test]
fn title_normalization_matches_legacy_behavior() {
    assert_eq!(normalize_title("  项目\t研发   群  "), "项目 研发 群");
    assert_eq!(normalize_title("\r\n工作群\n"), "工作群");
    assert_eq!(normalize_title("   "), "");
}

#[test]
fn protected_list_matches_groups_and_contacts_without_cross_matching() {
    let bypasses = TemporaryBypassRegistry::default();
    let group = ProtectedChat {
        display_name: "项目研发群".into(),
        match_title: "项目 研发群".into(),
        target_kind: ChatTargetKind::Group,
        ..ProtectedChat::default()
    };
    let contact = ProtectedChat {
        display_name: "小王".into(),
        match_title: "小王".into(),
        target_kind: ChatTargetKind::Contact,
        ..ProtectedChat::default()
    };
    let settings = AppSettings {
        protected_chats: vec![group.clone(), contact.clone()],
        ..AppSettings::default()
    };

    let decision = evaluate_protection(
        &group_editor(Some(" 项目\t研发群 ")),
        &settings,
        &bypasses,
        fixed_now(),
    );
    assert_eq!(decision.kind, ProtectionDecisionKind::ConfirmProtected);
    assert_eq!(
        decision.protected_chat.as_ref().map(|chat| chat.id),
        Some(group.id)
    );
    assert_eq!(
        evaluate_protection(
            &group_editor(Some("普通群")),
            &settings,
            &bypasses,
            fixed_now()
        )
        .kind,
        ProtectionDecisionKind::Pass
    );
    assert_eq!(
        evaluate_protection(
            &contact_editor(Some("小王")),
            &settings,
            &bypasses,
            fixed_now()
        )
        .kind,
        ProtectionDecisionKind::ConfirmProtected
    );
    assert_eq!(
        evaluate_protection(
            &contact_editor(Some("项目研发群")),
            &settings,
            &bypasses,
            fixed_now()
        )
        .kind,
        ProtectionDecisionKind::Pass
    );
}

#[test]
fn exemption_list_confirms_every_other_chat() {
    let settings = AppSettings {
        rule_mode: RuleMode::ConfirmUnlessExcluded,
        exempted_chats: vec![
            ProtectedChat {
                match_title: "家庭群".into(),
                target_kind: ChatTargetKind::Group,
                ..ProtectedChat::default()
            },
            ProtectedChat {
                match_title: "小王".into(),
                target_kind: ChatTargetKind::Contact,
                ..ProtectedChat::default()
            },
        ],
        ..AppSettings::default()
    };
    let bypasses = TemporaryBypassRegistry::default();

    assert_eq!(
        evaluate_protection(
            &group_editor(Some("家庭群")),
            &settings,
            &bypasses,
            fixed_now()
        )
        .kind,
        ProtectionDecisionKind::Pass
    );
    assert_eq!(
        evaluate_protection(
            &contact_editor(Some("小王")),
            &settings,
            &bypasses,
            fixed_now()
        )
        .kind,
        ProtectionDecisionKind::Pass
    );
    assert_eq!(
        evaluate_protection(
            &group_editor(Some("项目群")),
            &settings,
            &bypasses,
            fixed_now()
        )
        .kind,
        ProtectionDecisionKind::ConfirmUnlisted
    );
}

#[test]
fn list_sanitization_keeps_group_and_contact_entries_separate() {
    let settings = AppSettings {
        protected_chats: vec![
            ProtectedChat {
                match_title: "同名".into(),
                target_kind: ChatTargetKind::Group,
                ..ProtectedChat::default()
            },
            ProtectedChat {
                match_title: "同名".into(),
                target_kind: ChatTargetKind::Contact,
                ..ProtectedChat::default()
            },
        ],
        ..AppSettings::default()
    }
    .sanitize();

    assert_eq!(settings.protected_chats.len(), 2);
}

#[test]
fn unknown_context_obeys_configured_policy_and_never_revalidates_for_injection() {
    let bypasses = TemporaryBypassRegistry::default();
    assert_eq!(
        evaluate_protection(
            &group_editor(None),
            &AppSettings::default(),
            &bypasses,
            fixed_now()
        )
        .kind,
        ProtectionDecisionKind::ConfirmUnknown
    );
    let settings = AppSettings {
        unknown_context_behavior: UnknownContextBehavior::Block,
        ..AppSettings::default()
    };
    assert_eq!(
        evaluate_protection(&group_editor(None), &settings, &bypasses, fixed_now()).kind,
        ProtectionDecisionKind::BlockUnknown
    );

    let machine = SendGuardStateMachine::default();
    let unknown = group_editor(None);
    let pending = machine
        .try_begin(
            unknown.clone(),
            ProtectionDecision {
                kind: ProtectionDecisionKind::ConfirmUnknown,
                protected_chat: None,
            },
            false,
            Uuid::new_v4(),
            Duration::from_secs(10),
            fixed_now(),
        )
        .expect("unknown confirmation should begin");
    assert!(
        !machine
            .resolve(
                pending.attempt_id,
                ConfirmationOutcome::Confirmed,
                &unknown,
                fixed_now() + Duration::from_secs(1),
            )
            .should_inject
    );
}

#[test]
fn bypass_expires() {
    let registry = TemporaryBypassRegistry::default();
    let id = Uuid::new_v4();
    registry.grant(id, Duration::from_secs(60), fixed_now());
    assert!(registry.is_active(id, fixed_now() + Duration::from_secs(30)));
    assert!(!registry.is_active(id, fixed_now() + Duration::from_secs(120)));
    assert_eq!(
        registry.expiry(id, fixed_now() + Duration::from_secs(120)),
        None
    );
}

#[test]
fn settings_sanitization_matches_schema_v2_bounds() {
    let result = AppSettings {
        schema_version: 99,
        log_retention_days: 99,
        confirmation: ConfirmationSettings {
            hold_milliseconds: 100,
            timeout_seconds: 99,
            phrase: "   ".into(),
            ..ConfirmationSettings::default()
        },
        protected_chats: vec![
            ProtectedChat {
                match_title: " 工作群 ".into(),
                ..ProtectedChat::default()
            },
            ProtectedChat {
                match_title: "工作群".into(),
                ..ProtectedChat::default()
            },
            ProtectedChat::default(),
        ],
        shift_enter_pass_through: false,
        ..AppSettings::default()
    }
    .sanitize();

    assert_eq!(result.schema_version, CURRENT_SCHEMA_VERSION);
    assert_eq!(result.confirmation.hold_milliseconds, 500);
    assert_eq!(result.confirmation.timeout_seconds, 30);
    assert_eq!(result.confirmation.phrase, "确认发送");
    assert_eq!(result.log_retention_days, 30);
    assert!(result.shift_enter_pass_through);
    assert_eq!(result.protected_chats.len(), 1);
    assert_eq!(result.protected_chats[0].match_title, "工作群");
}

#[test]
fn schema_v2_fixture_from_the_dotnet_contract_loads_without_loss() {
    let fixture = include_str!("fixtures/settings-schema-v2.json");
    let settings: AppSettings = serde_json::from_str(fixture).expect("fixture must deserialize");
    let settings = settings.sanitize();

    assert_eq!(settings.schema_version, 2);
    assert_eq!(settings.protected_chats.len(), 1);
    assert_eq!(settings.protected_chats[0].aliases, ["项目 研发群"]);
    assert_eq!(settings.exempted_chats.len(), 1);
    assert_eq!(settings.confirmation.phrase, "确认发送");
    assert_eq!(settings.trusted_weixin_executable_path, None);
    assert!(settings.intercept_send_button);
}

#[test]
fn missing_send_button_interception_defaults_to_enabled() {
    let mut fixture: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/settings-schema-v2.json"))
            .expect("fixture should deserialize as JSON");
    fixture
        .as_object_mut()
        .expect("fixture should be a JSON object")
        .remove("interceptSendButton");

    let settings: AppSettings = serde_json::from_value(fixture)
        .expect("legacy settings should deserialize without the new preference");

    assert!(settings.intercept_send_button);
}

#[test]
fn legacy_settings_enable_update_checks_and_sanitize_an_empty_ignored_version() {
    let mut fixture: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/settings-schema-v2.json"))
            .expect("fixture should deserialize as JSON");
    let object = fixture
        .as_object_mut()
        .expect("fixture should be a JSON object");
    object.remove("autoCheckUpdates");
    object.insert(
        "ignoredUpdateVersion".into(),
        serde_json::Value::String("  ".into()),
    );

    let settings: AppSettings = serde_json::from_value(fixture)
        .expect("legacy settings should deserialize without update preferences");
    let settings = settings.sanitize();

    assert!(settings.auto_check_updates);
    assert_eq!(settings.ignored_update_version, None);
}

#[test]
fn settings_store_recovers_from_invalid_json_and_replaces_existing_file() {
    let directory = std::env::temp_dir().join(format!("WeChatSendGuard-core-{}", Uuid::new_v4()));
    let path = directory.join("settings.json");
    let store = FileSettingsStore::new(&path);

    store
        .save(AppSettings {
            protected_chats: vec![ProtectedChat {
                match_title: "工作群".into(),
                ..ProtectedChat::default()
            }],
            ..AppSettings::default()
        })
        .expect("initial settings save should work");
    store
        .save(AppSettings {
            protected_chats: vec![ProtectedChat {
                match_title: "升级后工作群".into(),
                ..ProtectedChat::default()
            }],
            ..AppSettings::default()
        })
        .expect("replacement settings save should work");
    assert_eq!(
        store
            .load()
            .expect("saved settings should load")
            .protected_chats[0]
            .match_title,
        "升级后工作群"
    );

    fs::write(&path, "{invalid json").expect("fixture overwrite should work");
    let recovered = store.load().expect("invalid settings should recover");
    assert!(recovered.enabled);
    assert!(recovered.protected_chats.is_empty());

    let _ = fs::remove_dir_all(&directory);
}

#[test]
fn protected_chat_export_is_compatible_with_v1_and_v2_imports() {
    let original = ProtectedChat {
        display_name: "项目研发群".into(),
        match_title: "项目研发群".into(),
        aliases: vec!["项目 研发群".into()],
        ..ProtectedChat::default()
    };
    let json = export_protected_chats(vec![original.clone()]).expect("export should serialize");
    let imported = import_protected_chats(&json).expect("v2 export should import");
    assert_eq!(imported.len(), 1);
    assert_eq!(imported[0].match_title, original.match_title);
    assert_eq!(imported[0].aliases, original.aliases);

    let legacy_v1 = include_str!("fixtures/protected-chats-schema-v1.json");
    assert_eq!(
        import_protected_chats(legacy_v1)
            .expect("v1 export should import")
            .len(),
        1
    );
}

#[test]
fn confirmed_unchanged_chat_injects_once_but_changed_or_expired_chat_does_not() {
    let machine = SendGuardStateMachine::default();
    let context = group_editor(Some("工作群"));
    let decision = ProtectionDecision {
        kind: ProtectionDecisionKind::ConfirmProtected,
        protected_chat: None,
    };
    let pending = machine
        .try_begin(
            context.clone(),
            decision.clone(),
            false,
            Uuid::new_v4(),
            Duration::from_secs(10),
            fixed_now(),
        )
        .expect("confirmation should begin");
    assert!(
        machine
            .resolve(
                pending.attempt_id,
                ConfirmationOutcome::Confirmed,
                &context,
                fixed_now() + Duration::from_secs(1),
            )
            .should_inject
    );
    assert!(machine.current().is_none());

    let changed = machine
        .try_begin(
            context.clone(),
            decision.clone(),
            false,
            Uuid::new_v4(),
            Duration::from_secs(10),
            fixed_now(),
        )
        .expect("second confirmation should begin");
    assert!(
        !machine
            .resolve(
                changed.attempt_id,
                ConfirmationOutcome::Confirmed,
                &group_editor(Some("另一个群")),
                fixed_now() + Duration::from_secs(1),
            )
            .should_inject
    );

    let expired = machine
        .try_begin(
            context.clone(),
            decision,
            false,
            Uuid::new_v4(),
            Duration::from_secs(5),
            fixed_now(),
        )
        .expect("third confirmation should begin");
    assert!(
        !machine
            .resolve(
                expired.attempt_id,
                ConfirmationOutcome::Confirmed,
                &context,
                fixed_now() + Duration::from_secs(6),
            )
            .should_inject
    );
}

#[test]
fn holding_confirmation_extends_deadline_and_prevents_timeout() {
    let machine = SendGuardStateMachine::default();
    let context = group_editor(Some("测试群"));
    let decision = ProtectionDecision {
        kind: ProtectionDecisionKind::ConfirmProtected,
        protected_chat: None,
    };
    let pending = machine
        .try_begin(
            context.clone(),
            decision,
            false,
            Uuid::new_v4(),
            Duration::from_secs(3),
            fixed_now(),
        )
        .expect("confirmation should begin");

    // 模拟按住暂停顺延 2 秒
    machine.extend_deadline(pending.attempt_id, Duration::from_secs(2));

    // 在 3.5 秒时确认（若未顺延则已超时失败，顺延后成功注入）
    let resolution = machine.resolve(
        pending.attempt_id,
        ConfirmationOutcome::Confirmed,
        &context,
        fixed_now() + Duration::from_millis(3500),
    );
    assert!(resolution.should_inject);
    assert_eq!(resolution.reason, "Confirmed");
}

#[test]
fn only_one_confirmation_can_be_pending() {
    let machine = SendGuardStateMachine::default();
    let decision = ProtectionDecision {
        kind: ProtectionDecisionKind::ConfirmProtected,
        protected_chat: None,
    };
    assert!(
        machine
            .try_begin(
                group_editor(Some("工作群")),
                decision.clone(),
                false,
                Uuid::new_v4(),
                Duration::from_secs(10),
                fixed_now(),
            )
            .is_some()
    );
    assert!(
        machine
            .try_begin(
                group_editor(Some("工作群")),
                decision,
                false,
                Uuid::new_v4(),
                Duration::from_secs(10),
                fixed_now(),
            )
            .is_none()
    );
}
