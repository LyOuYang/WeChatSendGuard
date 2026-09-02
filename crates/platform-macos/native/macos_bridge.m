#import "macos_bridge.h"

#import <AppKit/AppKit.h>
#import <ApplicationServices/ApplicationServices.h>
#import <Security/Security.h>
#include <float.h>
#include <math.h>
#include <pthread.h>
#include <stdatomic.h>
#include <string.h>
#include <time.h>

static NSString *const WSGWeChatBundleIdentifier = @"com.tencent.xinWeChat";
static NSString *const WSGWeChatTeamIdentifier = @"5A4RE8SF68";
static const CGKeyCode WSGReturnKeyCode = 36;
static const CGKeyCode WSGNumpadEnterKeyCode = 76;
static const CGKeyCode WSGEscapeKeyCode = 53;
static _Atomic(uint64_t) WSGContextGeneration = 1;
static _Atomic(bool) WSGFrontmostWeChat = false;
static _Atomic(int32_t) WSGFrontmostPID = 0;
static BOOL WSGElementsEqual(AXUIElementRef left, AXUIElementRef right);

static void WSGNoteFrontmostApplication(NSRunningApplication *application) {
    BOOL isWeChat = [application.bundleIdentifier isEqualToString:WSGWeChatBundleIdentifier];
    int32_t pid = application == nil ? 0 : application.processIdentifier;
    BOOL previousWeChat = atomic_exchange_explicit(&WSGFrontmostWeChat, isWeChat, memory_order_acq_rel);
    int32_t previousPID = atomic_exchange_explicit(&WSGFrontmostPID, pid, memory_order_acq_rel);
    if (previousWeChat != isWeChat || previousPID != pid) {
        atomic_fetch_add_explicit(&WSGContextGeneration, 1, memory_order_acq_rel);
    }
}

@interface WSGWorkspaceActivationObserver : NSObject
- (void)applicationActivated:(NSNotification *)notification;
@end

@implementation WSGWorkspaceActivationObserver
- (void)applicationActivated:(NSNotification *)notification {
    NSRunningApplication *application = notification.userInfo[NSWorkspaceApplicationKey];
    WSGNoteFrontmostApplication(application);
}
@end

static WSGWorkspaceActivationObserver *WSGWorkspaceObserver = nil;
static AXObserverRef WSGAccessibilityObserver = NULL;
static pid_t WSGAccessibilityObserverPID = 0;
static AXUIElementRef WSGAccessibilityObservedWindow = NULL;

static void WSGAccessibilityChanged(
    AXObserverRef observer,
    AXUIElementRef element,
    CFStringRef notification,
    void *context
) {
    (void)observer;
    (void)element;
    (void)notification;
    (void)context;
    atomic_fetch_add_explicit(&WSGContextGeneration, 1, memory_order_acq_rel);
}

static void WSGInstallWorkspaceObserver(void) {
    @synchronized(WSGWorkspaceActivationObserver.class) {
        if (WSGWorkspaceObserver != nil) {
            return;
        }
        WSGWorkspaceObserver = [WSGWorkspaceActivationObserver new];
        [NSWorkspace.sharedWorkspace.notificationCenter
            addObserver:WSGWorkspaceObserver
               selector:@selector(applicationActivated:)
                   name:NSWorkspaceDidActivateApplicationNotification
                 object:nil];
        WSGNoteFrontmostApplication(NSWorkspace.sharedWorkspace.frontmostApplication);
    }
}

static void WSGConfigureAccessibilityObserver(
    pid_t pid,
    AXUIElementRef application,
    AXUIElementRef window
) {
    @synchronized(WSGWorkspaceActivationObserver.class) {
        if (WSGAccessibilityObserverPID == pid && WSGAccessibilityObserver != NULL &&
            ((WSGAccessibilityObservedWindow == NULL && window == NULL) ||
             WSGElementsEqual(WSGAccessibilityObservedWindow, window))) {
            return;
        }
        if (WSGAccessibilityObserver != NULL) {
            CFRunLoopRemoveSource(CFRunLoopGetMain(),
                                  AXObserverGetRunLoopSource(WSGAccessibilityObserver),
                                  kCFRunLoopCommonModes);
            CFRelease(WSGAccessibilityObserver);
            WSGAccessibilityObserver = NULL;
            WSGAccessibilityObserverPID = 0;
        }
        if (WSGAccessibilityObservedWindow != NULL) {
            CFRelease(WSGAccessibilityObservedWindow);
            WSGAccessibilityObservedWindow = NULL;
        }
        AXObserverRef observer = NULL;
        if (AXObserverCreate(pid, WSGAccessibilityChanged, &observer) != kAXErrorSuccess || observer == NULL) {
            return;
        }
        AXObserverAddNotification(observer, application, kAXFocusedWindowChangedNotification, NULL);
        AXObserverAddNotification(observer, application, kAXFocusedUIElementChangedNotification, NULL);
        AXObserverAddNotification(observer, application, kAXWindowCreatedNotification, NULL);
        if (window != NULL) {
            AXObserverAddNotification(observer, window, kAXTitleChangedNotification, NULL);
            AXObserverAddNotification(observer, window, kAXLayoutChangedNotification, NULL);
            AXObserverAddNotification(observer, window, kAXUIElementDestroyedNotification, NULL);
        }
        WSGAccessibilityObserver = observer;
        WSGAccessibilityObserverPID = pid;
        WSGAccessibilityObservedWindow =
            window == NULL ? NULL : (AXUIElementRef)CFRetain(window);
        CFRunLoopAddSource(CFRunLoopGetMain(),
                           AXObserverGetRunLoopSource(observer),
                           kCFRunLoopCommonModes);
    }
}

static bool WSGCopyString(NSString *value, char *output, size_t capacity) {
    if (output == NULL || capacity == 0) {
        return false;
    }
    output[0] = '\0';
    if (value == nil) {
        return false;
    }
    return [value getCString:output maxLength:capacity encoding:NSUTF8StringEncoding];
}

static id WSGCopyAXAttribute(AXUIElementRef element, CFStringRef attribute) {
    if (element == NULL) {
        return nil;
    }
    CFTypeRef value = NULL;
    if (AXUIElementCopyAttributeValue(element, attribute, &value) != kAXErrorSuccess || value == NULL) {
        return nil;
    }
    return CFBridgingRelease(value);
}

static NSString *WSGCopyAXString(AXUIElementRef element, CFStringRef attribute) {
    id value = WSGCopyAXAttribute(element, attribute);
    return [value isKindOfClass:[NSString class]] ? value : nil;
}

static bool WSGCopyAXFrame(AXUIElementRef element, CGRect *frame) {
    id positionObject = WSGCopyAXAttribute(element, kAXPositionAttribute);
    id sizeObject = WSGCopyAXAttribute(element, kAXSizeAttribute);
    if (positionObject == nil || sizeObject == nil) {
        return false;
    }
    CGPoint position = CGPointZero;
    CGSize size = CGSizeZero;
    if (!AXValueGetValue((__bridge AXValueRef)positionObject, kAXValueCGPointType, &position) ||
        !AXValueGetValue((__bridge AXValueRef)sizeObject, kAXValueCGSizeType, &size) ||
        size.width <= 0 || size.height <= 0) {
        return false;
    }
    *frame = (CGRect){position, size};
    return true;
}

static BOOL WSGCopyAXBool(AXUIElementRef element, CFStringRef attribute, BOOL defaultValue) {
    id value = WSGCopyAXAttribute(element, attribute);
    return [value isKindOfClass:[NSNumber class]] ? [value boolValue] : defaultValue;
}

static BOOL WSGElementsEqual(AXUIElementRef left, AXUIElementRef right) {
    return left != NULL && right != NULL && CFEqual(left, right);
}

static BOOL WSGElementContainsFocus(AXUIElementRef element, AXUIElementRef focusedElement) {
    if (element == NULL || focusedElement == NULL) {
        return NO;
    }
    AXUIElementRef current = (AXUIElementRef)CFRetain(focusedElement);
    for (NSUInteger depth = 0; depth < 10 && current != NULL; ++depth) {
        if (WSGElementsEqual(element, current)) {
            CFRelease(current);
            return YES;
        }
        id parentObject = WSGCopyAXAttribute(current, kAXParentAttribute);
        CFRelease(current);
        current = parentObject == nil ? NULL : (AXUIElementRef)CFRetain((__bridge AXUIElementRef)parentObject);
    }
    if (current != NULL) {
        CFRelease(current);
    }
    return NO;
}

static NSString *WSGElementLabel(AXUIElementRef element) {
    for (id attribute in @[(__bridge id)kAXTitleAttribute,
                           (__bridge id)kAXDescriptionAttribute,
                           (__bridge id)kAXHelpAttribute]) {
        NSString *value = WSGCopyAXString(element, (__bridge CFStringRef)attribute);
        if (value.length > 0) {
            return [value stringByTrimmingCharactersInSet:NSCharacterSet.whitespaceAndNewlineCharacterSet];
        }
    }
    return nil;
}

static BOOL WSGIsSendLabel(NSString *label) {
    if (label.length == 0) {
        return NO;
    }
    NSString *lowercase = label.lowercaseString;
    return [label isEqualToString:@"发送"] ||
           [label containsString:@"发送消息"] ||
           [lowercase isEqualToString:@"send"] ||
           [lowercase containsString:@"send message"];
}

static BOOL WSGIsGenericHeaderLabel(NSString *label) {
    if (label.length == 0 || label.length > 128) {
        return YES;
    }
    NSSet<NSString *> *excluded = [NSSet setWithArray:@[
        @"微信", @"WeChat", @"聊天", @"通讯录", @"发现", @"我", @"搜索",
        @"发送", @"Send", @"聊天信息", @"Chat Info", @"语音聊天", @"视频聊天"
    ]];
    return [excluded containsObject:label];
}

static BOOL WSGHasGroupHint(NSString *label) {
    if (label.length == 0) {
        return NO;
    }
    NSString *lowercase = label.lowercaseString;
    if ([label containsString:@"群聊"] || [label containsString:@"群成员"] ||
        [lowercase containsString:@"group chat"] || [lowercase containsString:@"group members"]) {
        return YES;
    }
    NSRegularExpression *memberCount = [NSRegularExpression regularExpressionWithPattern:@"[（(]\\d+[）)]$"
                                                                                 options:0
                                                                                   error:nil];
    return [memberCount firstMatchInString:label options:0 range:NSMakeRange(0, label.length)] != nil;
}

@interface WSGAXCandidate : NSObject
@property(nonatomic) AXUIElementRef element;
@property(nonatomic) CGRect frame;
@property(nonatomic) BOOL focused;
@property(nonatomic) BOOL enabled;
@property(nonatomic, copy) NSString *label;
@end

@implementation WSGAXCandidate
- (void)dealloc {
    if (_element != NULL) {
        CFRelease(_element);
    }
}
- (void)setElement:(AXUIElementRef)element {
    if (_element == element) {
        return;
    }
    if (_element != NULL) {
        CFRelease(_element);
    }
    _element = element == NULL ? NULL : (AXUIElementRef)CFRetain(element);
}
@end

@interface WSGAXScanResult : NSObject
@property(nonatomic) AXUIElementRef window;
@property(nonatomic) AXUIElementRef editor;
@property(nonatomic) CGRect windowFrame;
@property(nonatomic) CGRect editorFrame;
@property(nonatomic) CGRect sendButtonFrame;
@property(nonatomic) BOOL editorFocused;
@property(nonatomic) BOOL sendButtonAvailable;
@property(nonatomic) BOOL sendButtonEnabled;
@property(nonatomic) BOOL groupChat;
@property(nonatomic, copy) NSString *chatTitle;
@end

@implementation WSGAXScanResult
- (void)dealloc {
    if (_window != NULL) {
        CFRelease(_window);
    }
    if (_editor != NULL) {
        CFRelease(_editor);
    }
}
- (void)setWindow:(AXUIElementRef)window {
    if (_window != NULL) {
        CFRelease(_window);
    }
    _window = window == NULL ? NULL : (AXUIElementRef)CFRetain(window);
}
- (void)setEditor:(AXUIElementRef)editor {
    if (_editor != NULL) {
        CFRelease(_editor);
    }
    _editor = editor == NULL ? NULL : (AXUIElementRef)CFRetain(editor);
}
@end

static int64_t WSGWindowIdentifier(pid_t pid, CGRect expectedFrame) {
    CFArrayRef windowInfo = CGWindowListCopyWindowInfo(kCGWindowListOptionOnScreenOnly, kCGNullWindowID);
    if (windowInfo == NULL) {
        return 0;
    }
    int64_t bestWindow = 0;
    double bestDistance = DBL_MAX;
    for (NSDictionary *entry in (__bridge NSArray *)windowInfo) {
        if ([entry[(id)kCGWindowOwnerPID] intValue] != pid || [entry[(id)kCGWindowLayer] intValue] != 0) {
            continue;
        }
        CGRect bounds = CGRectZero;
        if (!CGRectMakeWithDictionaryRepresentation((__bridge CFDictionaryRef)entry[(id)kCGWindowBounds], &bounds)) {
            continue;
        }
        double distance = fabs(bounds.origin.x - expectedFrame.origin.x) +
                          fabs(bounds.origin.y - expectedFrame.origin.y) +
                          fabs(bounds.size.width - expectedFrame.size.width) +
                          fabs(bounds.size.height - expectedFrame.size.height);
        if (distance < bestDistance) {
            bestDistance = distance;
            bestWindow = [entry[(id)kCGWindowNumber] longLongValue];
        }
    }
    CFRelease(windowInfo);
    return bestDistance <= 24.0 ? bestWindow : 0;
}

static WSGAXScanResult *WSGScanWindow(AXUIElementRef window, AXUIElementRef focusedElement) {
    CGRect windowFrame = CGRectZero;
    if (!WSGCopyAXFrame(window, &windowFrame)) {
        return nil;
    }

    NSMutableArray<WSGAXCandidate *> *editors = [NSMutableArray array];
    NSMutableArray<WSGAXCandidate *> *sendButtons = [NSMutableArray array];
    NSMutableArray<WSGAXCandidate *> *headers = [NSMutableArray array];
    BOOL groupHint = NO;
    CFMutableArrayRef queue = CFArrayCreateMutable(kCFAllocatorDefault, 0, &kCFTypeArrayCallBacks);
    CFArrayAppendValue(queue, window);
    CFIndex cursor = 0;
    const CFIndex maximumNodes = 1200;
    while (cursor < CFArrayGetCount(queue) && cursor < maximumNodes) {
        AXUIElementRef element = (AXUIElementRef)CFArrayGetValueAtIndex(queue, cursor++);
        NSString *role = WSGCopyAXString(element, kAXRoleAttribute);
        CGRect frame = CGRectZero;
        BOOL hasFrame = WSGCopyAXFrame(element, &frame);
        BOOL inComposer = hasFrame && CGRectGetMinY(frame) >= CGRectGetMinY(windowFrame) + windowFrame.size.height * 0.52;
        BOOL inHeader = hasFrame && CGRectGetMinY(frame) >= CGRectGetMinY(windowFrame) - 2.0 &&
                        CGRectGetMaxY(frame) <= CGRectGetMinY(windowFrame) + MIN(170.0, windowFrame.size.height * 0.28);

        if (([role isEqualToString:(__bridge NSString *)kAXTextAreaRole] ||
             [role isEqualToString:(__bridge NSString *)kAXTextFieldRole]) &&
            inComposer && frame.size.width >= MIN(220.0, windowFrame.size.width * 0.25)) {
            WSGAXCandidate *candidate = [WSGAXCandidate new];
            candidate.element = element;
            candidate.frame = frame;
            candidate.focused = WSGElementContainsFocus(element, focusedElement);
            candidate.enabled = WSGCopyAXBool(element, kAXEnabledAttribute, YES);
            [editors addObject:candidate];
        } else if ([role isEqualToString:(__bridge NSString *)kAXButtonRole] && inComposer) {
            NSString *label = WSGElementLabel(element);
            if (WSGIsSendLabel(label)) {
                WSGAXCandidate *candidate = [WSGAXCandidate new];
                candidate.element = element;
                candidate.frame = frame;
                candidate.enabled = WSGCopyAXBool(element, kAXEnabledAttribute, YES);
                candidate.label = label;
                [sendButtons addObject:candidate];
            }
        } else if (inHeader &&
                   ([role isEqualToString:(__bridge NSString *)kAXStaticTextRole] ||
                    [role isEqualToString:(__bridge NSString *)kAXHeadingRole] ||
                    [role isEqualToString:(__bridge NSString *)kAXButtonRole])) {
            NSString *label = WSGElementLabel(element);
            if (label.length == 0 && [role isEqualToString:(__bridge NSString *)kAXStaticTextRole]) {
                label = WSGCopyAXString(element, kAXValueAttribute);
            }
            label = [label stringByTrimmingCharactersInSet:NSCharacterSet.whitespaceAndNewlineCharacterSet];
            groupHint = groupHint || WSGHasGroupHint(label);
            if (!WSGIsGenericHeaderLabel(label)) {
                WSGAXCandidate *candidate = [WSGAXCandidate new];
                candidate.frame = frame;
                candidate.label = label;
                [headers addObject:candidate];
            }
        }

        id children = WSGCopyAXAttribute(element, kAXChildrenAttribute);
        if ([children isKindOfClass:[NSArray class]]) {
            for (id child in (NSArray *)children) {
                CFArrayAppendValue(queue, (__bridge CFTypeRef)child);
            }
        }
    }
    CFRelease(queue);

    WSGAXCandidate *selectedEditor = nil;
    for (WSGAXCandidate *candidate in editors) {
        if (!candidate.enabled) {
            continue;
        }
        if (selectedEditor == nil || candidate.focused ||
            candidate.frame.size.width * candidate.frame.size.height >
                selectedEditor.frame.size.width * selectedEditor.frame.size.height) {
            selectedEditor = candidate;
        }
        if (candidate.focused) {
            break;
        }
    }
    WSGAXCandidate *selectedButton = nil;
    double buttonDistance = DBL_MAX;
    for (WSGAXCandidate *candidate in sendButtons) {
        if (selectedEditor == nil) {
            break;
        }
        double verticalGap = fabs(CGRectGetMidY(candidate.frame) - CGRectGetMidY(selectedEditor.frame));
        double horizontalGap = fabs(CGRectGetMidX(candidate.frame) - CGRectGetMaxX(selectedEditor.frame));
        double distance = verticalGap * 2.0 + horizontalGap;
        if (distance < buttonDistance && verticalGap <= 180.0) {
            buttonDistance = distance;
            selectedButton = candidate;
        }
    }

    WSGAXCandidate *selectedHeader = nil;
    double headerScore = DBL_MAX;
    for (WSGAXCandidate *candidate in headers) {
        if (selectedEditor == nil ||
            CGRectGetMaxX(candidate.frame) < CGRectGetMinX(selectedEditor.frame) - 100.0) {
            continue;
        }
        double score = fabs(CGRectGetMidX(candidate.frame) - CGRectGetMidX(selectedEditor.frame)) * 0.02 +
                       (CGRectGetMinY(candidate.frame) - CGRectGetMinY(windowFrame));
        if (score < headerScore) {
            headerScore = score;
            selectedHeader = candidate;
        }
    }

    WSGAXScanResult *result = [WSGAXScanResult new];
    result.window = window;
    result.editor = selectedEditor == nil ? NULL : selectedEditor.element;
    result.windowFrame = windowFrame;
    result.editorFrame = selectedEditor == nil ? CGRectZero : selectedEditor.frame;
    result.editorFocused = selectedEditor != nil && selectedEditor.focused;
    result.sendButtonAvailable = selectedButton != nil;
    result.sendButtonEnabled = selectedButton.enabled;
    result.sendButtonFrame = selectedButton == nil ? CGRectZero : selectedButton.frame;
    result.chatTitle = selectedHeader.label;
    result.groupChat = groupHint || WSGHasGroupHint(result.chatTitle);
    return result;
}

static AXUIElementRef WSGCopyWindowForIdentifier(AXUIElementRef application, pid_t pid, int64_t expectedWindow) {
    id windows = WSGCopyAXAttribute(application, kAXWindowsAttribute);
    if (![windows isKindOfClass:[NSArray class]]) {
        return NULL;
    }
    for (id windowObject in (NSArray *)windows) {
        AXUIElementRef window = (__bridge AXUIElementRef)windowObject;
        CGRect frame = CGRectZero;
        if (WSGCopyAXFrame(window, &frame) && WSGWindowIdentifier(pid, frame) == expectedWindow) {
            return (AXUIElementRef)CFRetain(window);
        }
    }
    return NULL;
}

static BOOL WSGTrustedRunningApplication(NSRunningApplication *application) {
    if (application == nil || ![application.bundleIdentifier isEqualToString:WSGWeChatBundleIdentifier]) {
        return NO;
    }
    static pid_t cachedPID = 0;
    static NSDate *cachedLaunchDate = nil;
    static BOOL cachedTrust = NO;
    @synchronized(NSRunningApplication.class) {
        if (cachedPID == application.processIdentifier &&
            ((cachedLaunchDate == nil && application.launchDate == nil) ||
             [cachedLaunchDate isEqualToDate:application.launchDate])) {
            return cachedTrust;
        }
        NSDictionary *attributes = @{(__bridge id)kSecGuestAttributePid: @(application.processIdentifier)};
        SecCodeRef code = NULL;
        BOOL trusted = NO;
        if (SecCodeCopyGuestWithAttributes(NULL, (__bridge CFDictionaryRef)attributes, kSecCSDefaultFlags, &code) == errSecSuccess && code != NULL) {
            CFDictionaryRef signing = NULL;
            OSStatus validity = SecCodeCheckValidity(code, kSecCSStrictValidate, NULL);
            if (validity == errSecSuccess &&
                SecCodeCopySigningInformation(code, kSecCSSigningInformation, &signing) == errSecSuccess && signing != NULL) {
                NSDictionary *information = (__bridge NSDictionary *)signing;
                NSString *identifier = information[(__bridge id)kSecCodeInfoIdentifier];
                NSString *team = information[(__bridge id)kSecCodeInfoTeamIdentifier];
                trusted = [identifier isEqualToString:WSGWeChatBundleIdentifier] &&
                          [team isEqualToString:WSGWeChatTeamIdentifier];
            }
            if (signing != NULL) {
                CFRelease(signing);
            }
            CFRelease(code);
        }
        cachedPID = application.processIdentifier;
        cachedLaunchDate = application.launchDate;
        cachedTrust = trusted;
        return trusted;
    }
}

static void WSGFillSnapshot(
    NSRunningApplication *application,
    WSGAXScanResult *scan,
    BOOL trusted,
    BOOL accessibility,
    BOOL observeSendButton,
    WSGMacContextSnapshot *snapshot
) {
    memset(snapshot, 0, sizeof(*snapshot));
    snapshot->context_change_generation =
        atomic_load_explicit(&WSGContextGeneration, memory_order_acquire);
    snapshot->process_id = application == nil ? 0 : (uint32_t)application.processIdentifier;
    snapshot->is_trusted_weixin = trusted;
    snapshot->accessibility_available = accessibility;
    WSGCopyString(application.bundleURL.path, snapshot->process_path, sizeof(snapshot->process_path));
    if (scan == nil) {
        return;
    }
    snapshot->window_id = WSGWindowIdentifier(application.processIdentifier, scan.windowFrame);
    snapshot->window_x = scan.windowFrame.origin.x;
    snapshot->window_y = scan.windowFrame.origin.y;
    snapshot->window_width = scan.windowFrame.size.width;
    snapshot->window_height = scan.windowFrame.size.height;
    snapshot->message_editor_focused = scan.editorFocused;
    snapshot->is_group_chat = scan.chatTitle.length > 0 && scan.groupChat;
    snapshot->is_contact_chat = scan.chatTitle.length > 0 && !scan.groupChat;
    snapshot->compatibility_available = trusted && accessibility && snapshot->window_id != 0 &&
                                        scan.editor != NULL && scan.chatTitle.length > 0;
    snapshot->send_button_available = observeSendButton && scan.sendButtonAvailable;
    snapshot->send_button_enabled = snapshot->send_button_available && scan.sendButtonEnabled;
    if (snapshot->send_button_available) {
        snapshot->send_button_x = scan.sendButtonFrame.origin.x;
        snapshot->send_button_y = scan.sendButtonFrame.origin.y;
        snapshot->send_button_width = scan.sendButtonFrame.size.width;
        snapshot->send_button_height = scan.sendButtonFrame.size.height;
    }
    WSGCopyString(scan.chatTitle, snapshot->chat_title, sizeof(snapshot->chat_title));
}

bool WSGMacRequestAccessibilityAccess(void) {
    @autoreleasepool {
        WSGInstallWorkspaceObserver();
        NSDictionary *options = @{(__bridge id)kAXTrustedCheckOptionPrompt: @YES};
        return AXIsProcessTrustedWithOptions((__bridge CFDictionaryRef)options);
    }
}

uint64_t WSGMacContextChangeGeneration(void) {
    return atomic_load_explicit(&WSGContextGeneration, memory_order_acquire);
}

bool WSGMacFrontmostIsWeChat(void) {
    return atomic_load_explicit(&WSGFrontmostWeChat, memory_order_acquire);
}

bool WSGMacCopyForegroundContext(bool observeSendButton, WSGMacContextSnapshot *snapshot) {
    if (snapshot == NULL) {
        return false;
    }
    @autoreleasepool {
        memset(snapshot, 0, sizeof(*snapshot));
        NSRunningApplication *application = NSWorkspace.sharedWorkspace.frontmostApplication;
        WSGNoteFrontmostApplication(application);
        if (application == nil || ![application.bundleIdentifier isEqualToString:WSGWeChatBundleIdentifier]) {
            snapshot->context_change_generation =
                atomic_load_explicit(&WSGContextGeneration, memory_order_acquire);
            return true;
        }
        BOOL trusted = WSGTrustedRunningApplication(application);
        BOOL accessibility = AXIsProcessTrusted();
        if (!trusted || !accessibility) {
            WSGFillSnapshot(application, nil, trusted, accessibility, observeSendButton, snapshot);
            return true;
        }
        AXUIElementRef axApplication = AXUIElementCreateApplication(application.processIdentifier);
        id focusedWindowObject = WSGCopyAXAttribute(axApplication, kAXFocusedWindowAttribute);
        id focusedElementObject = WSGCopyAXAttribute(axApplication, kAXFocusedUIElementAttribute);
        WSGAXScanResult *scan = nil;
        if (focusedWindowObject != nil) {
            WSGConfigureAccessibilityObserver(application.processIdentifier,
                                              axApplication,
                                              (__bridge AXUIElementRef)focusedWindowObject);
            scan = WSGScanWindow((__bridge AXUIElementRef)focusedWindowObject,
                                 (__bridge AXUIElementRef)focusedElementObject);
        }
        WSGFillSnapshot(application, scan, trusted, accessibility, observeSendButton, snapshot);
        CFRelease(axApplication);
        return true;
    }
}

bool WSGMacRestoreEditorFocusAndCopyContext(
    int64_t expectedWindowId,
    uint32_t expectedProcessId,
    WSGMacContextSnapshot *snapshot
) {
    if (snapshot == NULL || expectedWindowId == 0 || expectedProcessId == 0) {
        return false;
    }
    @autoreleasepool {
        NSRunningApplication *application = [NSRunningApplication runningApplicationWithProcessIdentifier:(pid_t)expectedProcessId];
        if (!WSGTrustedRunningApplication(application) || !AXIsProcessTrusted()) {
            memset(snapshot, 0, sizeof(*snapshot));
            return false;
        }
        AXUIElementRef axApplication = AXUIElementCreateApplication((pid_t)expectedProcessId);
        AXUIElementRef window = WSGCopyWindowForIdentifier(axApplication, (pid_t)expectedProcessId, expectedWindowId);
        if (window == NULL) {
            CFRelease(axApplication);
            memset(snapshot, 0, sizeof(*snapshot));
            return false;
        }
        WSGAXScanResult *scan = WSGScanWindow(window, NULL);
        if (scan == nil || scan.editor == NULL) {
            CFRelease(window);
            CFRelease(axApplication);
            memset(snapshot, 0, sizeof(*snapshot));
            return false;
        }
        [application activateWithOptions:NSApplicationActivateIgnoringOtherApps];
        AXUIElementPerformAction(window, kAXRaiseAction);
        AXUIElementSetAttributeValue(axApplication, kAXFocusedWindowAttribute, window);
        AXUIElementSetAttributeValue(scan.editor, kAXFocusedAttribute, kCFBooleanTrue);
        AXUIElementSetAttributeValue(axApplication, kAXFocusedUIElementAttribute, scan.editor);
        [NSThread sleepForTimeInterval:0.04];
        id focusedElementObject = WSGCopyAXAttribute(axApplication, kAXFocusedUIElementAttribute);
        scan = WSGScanWindow(window, (__bridge AXUIElementRef)focusedElementObject);
        WSGFillSnapshot(application, scan, YES, YES, YES, snapshot);
        CFRelease(window);
        CFRelease(axApplication);
        return snapshot->message_editor_focused && snapshot->window_id == expectedWindowId;
    }
}

bool WSGMacCopyDraftPreview(
    int64_t expectedWindowId,
    uint32_t expectedProcessId,
    char *output,
    size_t outputCapacity
) {
    if (output == NULL || outputCapacity == 0 || expectedWindowId == 0 || expectedProcessId == 0) {
        return false;
    }
    output[0] = '\0';
    @autoreleasepool {
        NSRunningApplication *application = [NSRunningApplication runningApplicationWithProcessIdentifier:(pid_t)expectedProcessId];
        if (!WSGTrustedRunningApplication(application) || !AXIsProcessTrusted()) {
            return false;
        }
        AXUIElementRef axApplication = AXUIElementCreateApplication((pid_t)expectedProcessId);
        AXUIElementRef window = WSGCopyWindowForIdentifier(axApplication, (pid_t)expectedProcessId, expectedWindowId);
        if (window == NULL) {
            CFRelease(axApplication);
            return false;
        }
        WSGAXScanResult *scan = WSGScanWindow(window, NULL);
        NSString *draft = scan.editor == NULL ? nil : WSGCopyAXString(scan.editor, kAXValueAttribute);
        if (draft.length > 240) {
            draft = [[draft substringToIndex:240] stringByAppendingString:@"…"];
        }
        BOOL copied = draft == nil || WSGCopyString(draft, output, outputCapacity);
        CFRelease(window);
        CFRelease(axApplication);
        return copied;
    }
}

typedef NS_ENUM(NSUInteger, WSGInputTapKind) {
    WSGInputTapKindKeyboard,
    WSGInputTapKindMouse,
};

@interface WSGInputTap : NSObject {
@public
    _Atomic(bool) ready;
    _Atomic(bool) failed;
    _Atomic(bool) stopping;
}
@property(nonatomic) WSGInputTapKind kind;
@property(nonatomic) uint64_t marker;
@property(nonatomic) WSGMacKeyboardCallback keyboardCallback;
@property(nonatomic) WSGMacMouseCallback mouseCallback;
@property(nonatomic) void *callbackContext;
@property(nonatomic) CFMachPortRef eventTap;
@property(nonatomic) CFRunLoopRef runLoop;
@property(nonatomic) pthread_t thread;
@property(nonatomic) BOOL suppressingRelease;
@property(nonatomic) int64_t suppressedCode;
@end

@implementation WSGInputTap
- (void)dealloc {
    if (_eventTap != NULL) {
        CFRelease(_eventTap);
    }
    if (_runLoop != NULL) {
        CFRelease(_runLoop);
    }
}
@end

static CGEventRef WSGInputTapCallback(
    CGEventTapProxy proxy,
    CGEventType type,
    CGEventRef event,
    void *reference
) {
    (void)proxy;
    WSGInputTap *tap = (__bridge WSGInputTap *)reference;
    if (type == kCGEventTapDisabledByTimeout || type == kCGEventTapDisabledByUserInput) {
        if (tap.eventTap != NULL && !atomic_load_explicit(&tap->stopping, memory_order_acquire)) {
            CGEventTapEnable(tap.eventTap, true);
        }
        return event;
    }
    if (tap.kind == WSGInputTapKindKeyboard) {
        CGKeyCode keyCode = (CGKeyCode)CGEventGetIntegerValueField(event, kCGKeyboardEventKeycode);
        if (type == kCGEventKeyUp && tap.suppressingRelease && keyCode == tap.suppressedCode) {
            tap.suppressingRelease = NO;
            tap.suppressedCode = 0;
            return NULL;
        }
        if (type != kCGEventKeyDown ||
            (keyCode != WSGReturnKeyCode && keyCode != WSGNumpadEnterKeyCode && keyCode != WSGEscapeKeyCode)) {
            return event;
        }
        uint64_t eventMarker = (uint64_t)CGEventGetIntegerValueField(event, kCGEventSourceUserData);
        BOOL injected = eventMarker == tap.marker;
        CGEventFlags flags = CGEventGetFlags(event);
        BOOL shift = (flags & kCGEventFlagMaskShift) != 0;
        BOOL modifier = (flags & (kCGEventFlagMaskShift |
                                  kCGEventFlagMaskControl |
                                  kCGEventFlagMaskAlternate |
                                  kCGEventFlagMaskCommand)) != 0;
        BOOL suppress = tap.keyboardCallback != NULL &&
                        tap.keyboardCallback(keyCode, injected, shift, modifier, tap.callbackContext);
        if (suppress) {
            tap.suppressingRelease = YES;
            tap.suppressedCode = keyCode;
            return NULL;
        }
        return event;
    }

    if (type == kCGEventLeftMouseUp && tap.suppressingRelease) {
        tap.suppressingRelease = NO;
        return NULL;
    }
    if (type != kCGEventLeftMouseDown) {
        return event;
    }
    uint64_t eventMarker = (uint64_t)CGEventGetIntegerValueField(event, kCGEventSourceUserData);
    if (eventMarker == tap.marker) {
        return event;
    }
    CGPoint point = CGEventGetLocation(event);
    BOOL suppress = tap.mouseCallback != NULL &&
                    tap.mouseCallback((int32_t)llround(point.x), (int32_t)llround(point.y), tap.callbackContext);
    if (suppress) {
        tap.suppressingRelease = YES;
        return NULL;
    }
    return event;
}

static void *WSGInputTapThread(void *reference) {
    @autoreleasepool {
        WSGInputTap *tap = (__bridge WSGInputTap *)reference;
        CGEventMask mask = tap.kind == WSGInputTapKindKeyboard
            ? CGEventMaskBit(kCGEventKeyDown) | CGEventMaskBit(kCGEventKeyUp)
            : CGEventMaskBit(kCGEventLeftMouseDown) | CGEventMaskBit(kCGEventLeftMouseUp);
        tap.eventTap = CGEventTapCreate(kCGSessionEventTap,
                                        kCGHeadInsertEventTap,
                                        kCGEventTapOptionDefault,
                                        mask,
                                        WSGInputTapCallback,
                                        reference);
        if (tap.eventTap == NULL) {
            atomic_store_explicit(&tap->failed, true, memory_order_release);
            atomic_store_explicit(&tap->ready, true, memory_order_release);
            return NULL;
        }
        CFRunLoopSourceRef source = CFMachPortCreateRunLoopSource(kCFAllocatorDefault, tap.eventTap, 0);
        tap.runLoop = (CFRunLoopRef)CFRetain(CFRunLoopGetCurrent());
        CFRunLoopAddSource(tap.runLoop, source, kCFRunLoopCommonModes);
        CGEventTapEnable(tap.eventTap, true);
        atomic_store_explicit(&tap->ready, true, memory_order_release);
        CFRunLoopRun();
        CFRunLoopRemoveSource(tap.runLoop, source, kCFRunLoopCommonModes);
        CFRelease(source);
        return NULL;
    }
}

static void *WSGStartInputTap(
    WSGInputTapKind kind,
    uint64_t marker,
    WSGMacKeyboardCallback keyboardCallback,
    WSGMacMouseCallback mouseCallback,
    void *context
) {
    if (!CGPreflightListenEventAccess()) {
        CGRequestListenEventAccess();
        return NULL;
    }
    WSGInputTap *tap = [WSGInputTap new];
    tap.kind = kind;
    tap.marker = marker;
    tap.keyboardCallback = keyboardCallback;
    tap.mouseCallback = mouseCallback;
    tap.callbackContext = context;
    atomic_init(&tap->ready, false);
    atomic_init(&tap->failed, false);
    atomic_init(&tap->stopping, false);
    void *handle = (void *)CFBridgingRetain(tap);
    pthread_t thread;
    if (pthread_create(&thread, NULL, WSGInputTapThread, handle) != 0) {
        CFBridgingRelease(handle);
        return NULL;
    }
    tap.thread = thread;
    for (NSUInteger attempt = 0; attempt < 400 && !atomic_load_explicit(&tap->ready, memory_order_acquire); ++attempt) {
        [NSThread sleepForTimeInterval:0.005];
    }
    if (!atomic_load_explicit(&tap->ready, memory_order_acquire) ||
        atomic_load_explicit(&tap->failed, memory_order_acquire)) {
        atomic_store_explicit(&tap->stopping, true, memory_order_release);
        if (tap.runLoop != NULL) {
            CFRunLoopStop(tap.runLoop);
        }
        pthread_join(tap.thread, NULL);
        CFBridgingRelease(handle);
        return NULL;
    }
    return handle;
}

void *WSGMacStartKeyboardTap(uint64_t marker, WSGMacKeyboardCallback callback, void *context) {
    return WSGStartInputTap(WSGInputTapKindKeyboard, marker, callback, NULL, context);
}

void *WSGMacStartMouseTap(uint64_t marker, WSGMacMouseCallback callback, void *context) {
    return WSGStartInputTap(WSGInputTapKindMouse, marker, NULL, callback, context);
}

void WSGMacStopInputTap(void *handle) {
    if (handle == NULL) {
        return;
    }
    WSGInputTap *tap = (__bridge WSGInputTap *)handle;
    atomic_store_explicit(&tap->stopping, true, memory_order_release);
    if (tap.runLoop != NULL) {
        CFRunLoopStop(tap.runLoop);
        CFRunLoopWakeUp(tap.runLoop);
    }
    pthread_join(tap.thread, NULL);
    CFBridgingRelease(handle);
}

bool WSGMacPostEnter(uint16_t keyCode, uint64_t marker) {
    if (!CGPreflightPostEventAccess()) {
        CGRequestPostEventAccess();
        return false;
    }
    CGEventRef down = CGEventCreateKeyboardEvent(NULL, (CGKeyCode)keyCode, true);
    CGEventRef up = CGEventCreateKeyboardEvent(NULL, (CGKeyCode)keyCode, false);
    if (down == NULL || up == NULL) {
        if (down != NULL) CFRelease(down);
        if (up != NULL) CFRelease(up);
        return false;
    }
    CGEventSetIntegerValueField(down, kCGEventSourceUserData, (int64_t)marker);
    CGEventSetIntegerValueField(up, kCGEventSourceUserData, (int64_t)marker);
    CGEventPost(kCGHIDEventTap, down);
    CGEventPost(kCGHIDEventTap, up);
    CFRelease(down);
    CFRelease(up);
    return true;
}

bool WSGMacCopyCursorPosition(int32_t *x, int32_t *y) {
    if (x == NULL || y == NULL) {
        return false;
    }
    CGEventRef event = CGEventCreate(NULL);
    if (event == NULL) {
        return false;
    }
    CGPoint point = CGEventGetLocation(event);
    CFRelease(event);
    *x = (int32_t)llround(point.x);
    *y = (int32_t)llround(point.y);
    return true;
}

void WSGMacActivateWindow(int64_t nativeView) {
    if (nativeView == 0) {
        return;
    }
    NSView *view = (__bridge NSView *)(void *)(uintptr_t)nativeView;
    [NSApp activateIgnoringOtherApps:YES];
    [view.window makeKeyAndOrderFront:nil];
}

void WSGMacShowErrorDialog(const char *message) {
    @autoreleasepool {
        NSAlert *alert = [NSAlert new];
        alert.alertStyle = NSAlertStyleCritical;
        alert.messageText = @"WeChatSendGuard 无法启动";
        alert.informativeText = message == NULL ? @"未知错误" : [NSString stringWithUTF8String:message];
        [alert addButtonWithTitle:@"确定"];
        [alert runModal];
    }
}

bool WSGMacSelectOpenJSON(char *output, size_t outputCapacity) {
    @autoreleasepool {
        NSOpenPanel *panel = [NSOpenPanel openPanel];
        panel.canChooseFiles = YES;
        panel.canChooseDirectories = NO;
        panel.allowsMultipleSelection = NO;
        panel.allowedFileTypes = @[@"json"];
        if ([panel runModal] != NSModalResponseOK) {
            return false;
        }
        return WSGCopyString(panel.URL.path, output, outputCapacity);
    }
}

bool WSGMacSelectSavePath(
    const char *defaultName,
    const char *allowedExtension,
    char *output,
    size_t outputCapacity
) {
    @autoreleasepool {
        NSSavePanel *panel = [NSSavePanel savePanel];
        if (defaultName != NULL) {
            panel.nameFieldStringValue = [NSString stringWithUTF8String:defaultName];
        }
        if (allowedExtension != NULL && strlen(allowedExtension) > 0) {
            panel.allowedFileTypes = @[[NSString stringWithUTF8String:allowedExtension]];
        }
        if ([panel runModal] != NSModalResponseOK) {
            return false;
        }
        return WSGCopyString(panel.URL.path, output, outputCapacity);
    }
}

bool WSGMacCopyOperatingSystemVersion(char *output, size_t outputCapacity) {
    @autoreleasepool {
        NSString *version = [NSString stringWithFormat:@"macOS %@", NSProcessInfo.processInfo.operatingSystemVersionString];
        return WSGCopyString(version, output, outputCapacity);
    }
}

bool WSGMacCopyInstalledWeChatVersion(char *output, size_t outputCapacity) {
    @autoreleasepool {
        NSURL *url = [NSWorkspace.sharedWorkspace URLForApplicationWithBundleIdentifier:WSGWeChatBundleIdentifier];
        NSBundle *bundle = url == nil ? nil : [NSBundle bundleWithURL:url];
        NSString *version = [bundle objectForInfoDictionaryKey:@"CFBundleShortVersionString"];
        return WSGCopyString(version, output, outputCapacity);
    }
}

bool WSGMacCopyLocalDate(uint16_t *year, uint16_t *month, uint16_t *day) {
    if (year == NULL || month == NULL || day == NULL) {
        return false;
    }
    time_t current = time(NULL);
    struct tm local = {0};
    if (localtime_r(&current, &local) == NULL) {
        return false;
    }
    *year = (uint16_t)(local.tm_year + 1900);
    *month = (uint16_t)(local.tm_mon + 1);
    *day = (uint16_t)local.tm_mday;
    return true;
}
