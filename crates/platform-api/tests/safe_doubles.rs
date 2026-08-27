use wechat_send_guard_core::ChatContext;
use wechat_send_guard_platform_api::{
    ChatContextProvider, FakeChatContextProvider, InputInjector, RecordingInputInjector,
};

#[test]
fn safe_doubles_only_use_test_supplied_state() {
    let provider = FakeChatContextProvider::new(ChatContext {
        process_id: 7,
        is_trusted_weixin: true,
        ..ChatContext::default()
    });
    assert_eq!(provider.current().process_id, 7);
    assert_eq!(
        provider
            .refresh_now()
            .expect("fake refresh should work")
            .process_id,
        7
    );

    let injector = RecordingInputInjector::default();
    injector.send_enter(true).expect("recording should work");
    assert_eq!(injector.sent().len(), 1);
    assert!(injector.sent()[0].is_numpad_enter);
}
