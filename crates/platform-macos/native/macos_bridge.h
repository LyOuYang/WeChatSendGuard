#ifndef WECHAT_SEND_GUARD_MACOS_BRIDGE_H
#define WECHAT_SEND_GUARD_MACOS_BRIDGE_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#define WSG_MACOS_PATH_CAPACITY 4096
#define WSG_MACOS_TITLE_CAPACITY 512
#define WSG_MACOS_TEXT_CAPACITY 1024

typedef struct {
    uint64_t context_change_generation;
    int64_t window_id;
    uint32_t process_id;
    bool is_trusted_weixin;
    bool accessibility_available;
    bool compatibility_available;
    bool message_editor_focused;
    bool is_group_chat;
    bool is_contact_chat;
    bool send_button_available;
    bool send_button_enabled;
    double window_x;
    double window_y;
    double window_width;
    double window_height;
    double send_button_x;
    double send_button_y;
    double send_button_width;
    double send_button_height;
    char process_path[WSG_MACOS_PATH_CAPACITY];
    char chat_title[WSG_MACOS_TITLE_CAPACITY];
} WSGMacContextSnapshot;

typedef bool (*WSGMacKeyboardCallback)(
    uint16_t key_code,
    bool is_injected,
    bool shift_pressed,
    void *context
);
typedef bool (*WSGMacMouseCallback)(int32_t screen_x, int32_t screen_y, void *context);

bool WSGMacRequestAccessibilityAccess(void);
uint64_t WSGMacContextChangeGeneration(void);
bool WSGMacFrontmostIsWeChat(void);
bool WSGMacCopyForegroundContext(bool observe_send_button, WSGMacContextSnapshot *snapshot);
bool WSGMacRestoreEditorFocusAndCopyContext(
    int64_t expected_window_id,
    uint32_t expected_process_id,
    WSGMacContextSnapshot *snapshot
);
bool WSGMacCopyDraftPreview(
    int64_t expected_window_id,
    uint32_t expected_process_id,
    char *output,
    size_t output_capacity
);

void *WSGMacStartKeyboardTap(uint64_t marker, WSGMacKeyboardCallback callback, void *context);
void *WSGMacStartMouseTap(uint64_t marker, WSGMacMouseCallback callback, void *context);
void WSGMacStopInputTap(void *handle);
bool WSGMacPostEnter(uint16_t key_code, uint64_t marker);

bool WSGMacCopyCursorPosition(int32_t *x, int32_t *y);
void WSGMacActivateWindow(int64_t native_view);
void WSGMacShowErrorDialog(const char *message);
bool WSGMacSelectOpenJSON(char *output, size_t output_capacity);
bool WSGMacSelectSavePath(
    const char *default_name,
    const char *allowed_extension,
    char *output,
    size_t output_capacity
);
bool WSGMacCopyOperatingSystemVersion(char *output, size_t output_capacity);
bool WSGMacCopyInstalledWeChatVersion(char *output, size_t output_capacity);
bool WSGMacCopyLocalDate(uint16_t *year, uint16_t *month, uint16_t *day);

#endif
