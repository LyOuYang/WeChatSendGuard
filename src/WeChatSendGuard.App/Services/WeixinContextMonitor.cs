using System.Diagnostics;
using System.Windows.Automation;
using System.Windows.Threading;
using WeChatSendGuard.App.Interop;
using WeChatSendGuard.Core.Configuration;
using WeChatSendGuard.Core.Guard;

namespace WeChatSendGuard.App.Services;

internal sealed class WeixinContextMonitor : IChatContextProvider, IDisposable
{
    private const string InputAutomationId = "chat_input_field";
    private const string ChatNameAutomationId = "current_chat_name_label";
    private const string GroupTitleClassName = "mmui::ChatTitleBarChatRoomView";
    private const string GroupTitleClassNameSuffix = "ChatTitleBarChatRoomView";
    private const int DraftPreviewLimit = 600;

    private readonly Dispatcher _dispatcher;
    private readonly ForegroundWindowMonitor _foregroundMonitor = new();
    private readonly DispatcherTimer _debounceTimer;
    private readonly AutomationFocusChangedEventHandler _focusChangedHandler;
    private readonly AutomationPropertyChangedEventHandler _propertyChangedHandler;
    private readonly object _sync = new();
    private AutomationElement? _watchedTitle;
    private AutomationElement? _watchedRoot;
    private ChatContext _current = ChatContext.Inactive;
    private ChatContext _lastRecognizedWeixin = ChatContext.Inactive;
    private bool _focusHandlerInstalled;
    private bool _started;
    private bool _disposed;

    public WeixinContextMonitor(Dispatcher dispatcher)
    {
        _dispatcher = dispatcher;
        _debounceTimer = new DispatcherTimer(DispatcherPriority.Background, dispatcher)
        {
            Interval = TimeSpan.FromMilliseconds(250),
        };
        _debounceTimer.Tick += DebounceTimer_Tick;
        _foregroundMonitor.ForegroundWindowChanged += ForegroundMonitor_ForegroundWindowChanged;
        _focusChangedHandler = (_, _) => QueueRefresh();
        _propertyChangedHandler = (_, _) => QueueRefresh();
    }

    public event EventHandler<ChatContext>? ContextChanged;

    public ChatContext Current => Volatile.Read(ref _current);

    public ChatContext LastRecognizedWeixin => Volatile.Read(ref _lastRecognizedWeixin);

    public bool CanHandleProtectCurrentShortcut
    {
        get
        {
            var context = Current;
            return context.IsTrustedWeixin
                && context.IsCompatibilityAvailable
                && context.IsMessageEditorFocused
                && context.IsKnownChat
                && context.WindowHandle != nint.Zero
                && NativeMethods.GetForegroundWindow() == context.WindowHandle;
        }
    }

    public void Start()
    {
        ObjectDisposedException.ThrowIf(_disposed, this);
        if (_started)
        {
            return;
        }

        _started = true;
        _foregroundMonitor.Start();
        QueueRefresh();
    }

    public ChatContext RefreshNow()
    {
        ObjectDisposedException.ThrowIf(_disposed, this);
        var foregroundWindow = NativeMethods.GetForegroundWindow();
        var processId = 0;
        var processPath = string.Empty;
        var requiresElevation = false;
        if (foregroundWindow == nint.Zero || !WeixinProcessTrust.IsTrusted(foregroundWindow, out processId, out processPath, out requiresElevation))
        {
            UnsubscribeAutomationEvents();
            return PublishInactive(foregroundWindow, processId, processPath, requiresElevation);
        }

        try
        {
            var root = AutomationElement.FromHandle(foregroundWindow);
            var editor = FindByAutomationIdSuffix(root, InputAutomationId);
            var groupTitle = FindByClassNameSuffix(root, GroupTitleClassNameSuffix);
            var titleElement = groupTitle is null
                ? FindByAutomationIdSuffix(root, ChatNameAutomationId)
                : FindByAutomationIdSuffix(groupTitle, ChatNameAutomationId);
            var title = ReadName(titleElement);
            var focused = IsEditorFocused(editor, root);

            SubscribeAutomationEvents(root, titleElement);
            var context = new ChatContext
            {
                WindowHandle = foregroundWindow,
                ProcessId = processId,
                ProcessPath = processPath,
                IsTrustedWeixin = true,
                IsCompatibilityAvailable = editor is not null,
                IsMessageEditorFocused = focused,
                IsGroupChat = groupTitle is not null,
                IsContactChat = groupTitle is null && titleElement is not null && !string.IsNullOrWhiteSpace(title),
                ChatTitle = title,
                Generation = Current.Generation,
                ObservedAt = DateTimeOffset.UtcNow,
            };
            return Publish(context);
        }
        catch (Exception ex) when (ex is not OutOfMemoryException and not StackOverflowException and not AccessViolationException)
        {
            UnsubscribeAutomationEvents();
            return Publish(new ChatContext
            {
                WindowHandle = foregroundWindow,
                ProcessId = processId,
                ProcessPath = processPath,
                IsTrustedWeixin = true,
                IsCompatibilityAvailable = false,
                ObservedAt = DateTimeOffset.UtcNow,
                Generation = Current.Generation,
            });
        }
    }

    public Task<ChatContext> RefreshNowAsync()
    {
        if (_dispatcher.CheckAccess())
        {
            return Task.FromResult(RefreshNow());
        }

        return _dispatcher.InvokeAsync(RefreshNow).Task;
    }

    public Task<bool> TryRestoreMessageEditorFocusAsync(ChatContext expected)
    {
        if (_dispatcher.CheckAccess())
        {
            return Task.FromResult(TryRestoreMessageEditorFocus(expected));
        }

        return _dispatcher.InvokeAsync(() => TryRestoreMessageEditorFocus(expected)).Task;
    }

    public Task<string?> TryReadDraftPreviewAsync(ChatContext expected)
    {
        if (_dispatcher.CheckAccess())
        {
            return Task.FromResult(TryReadDraftPreview(expected));
        }

        return _dispatcher.InvokeAsync(() => TryReadDraftPreview(expected)).Task;
    }

    public void Dispose()
    {
        if (_disposed)
        {
            return;
        }

        _disposed = true;
        _debounceTimer.Stop();
        _foregroundMonitor.Dispose();
        UnsubscribeAutomationEvents();
        _debounceTimer.Tick -= DebounceTimer_Tick;
        _foregroundMonitor.ForegroundWindowChanged -= ForegroundMonitor_ForegroundWindowChanged;
    }

    private void ForegroundMonitor_ForegroundWindowChanged(object? sender, nint windowHandle)
    {
        QueueRefresh();
    }

    private void QueueRefresh()
    {
        if (_disposed)
        {
            return;
        }

        if (!_dispatcher.CheckAccess())
        {
            _dispatcher.BeginInvoke(QueueRefresh);
            return;
        }

        _debounceTimer.Stop();
        _debounceTimer.Start();
    }

    private void DebounceTimer_Tick(object? sender, EventArgs e)
    {
        _debounceTimer.Stop();
        if (!_disposed)
        {
            RefreshNow();
        }
    }

    private ChatContext PublishInactive(nint foregroundWindow, int processId, string processPath, bool requiresElevation)
    {
        return Publish(new ChatContext
        {
            WindowHandle = foregroundWindow,
            ProcessId = processId,
            ProcessPath = processPath,
            IsTrustedWeixin = false,
            RequiresElevation = requiresElevation,
            IsCompatibilityAvailable = false,
            ObservedAt = DateTimeOffset.UtcNow,
            Generation = Current.Generation,
        });
    }

    private bool TryRestoreMessageEditorFocus(ChatContext expected)
    {
        if (!TryGetExpectedEditor(expected, out var editor) || editor is null)
        {
            return false;
        }

        try
        {
            editor.SetFocus();
            return true;
        }
        catch (Exception ex) when (ex is ElementNotAvailableException or InvalidOperationException or COMException)
        {
            return false;
        }
    }

    private string? TryReadDraftPreview(ChatContext expected)
    {
        if (!TryGetExpectedEditor(expected, out var editor) || editor is null)
        {
            return null;
        }

        try
        {
            string? text = null;
            if (editor.TryGetCurrentPattern(ValuePattern.Pattern, out var valuePattern))
            {
                text = ((ValuePattern)valuePattern).Current.Value;
            }
            else if (editor.TryGetCurrentPattern(TextPattern.Pattern, out var textPattern))
            {
                text = ((TextPattern)textPattern).DocumentRange.GetText(DraftPreviewLimit + 1);
            }

            return NormalizeDraftPreview(text);
        }
        catch (Exception ex) when (ex is ElementNotAvailableException or InvalidOperationException or COMException)
        {
            return null;
        }
    }

    private static bool TryGetExpectedEditor(ChatContext expected, out AutomationElement? editor)
    {
        editor = null;
        if (!expected.IsTrustedWeixin
            || !expected.IsKnownChat
            || string.IsNullOrEmpty(expected.NormalizedChatTitle)
            || expected.WindowHandle == nint.Zero
            || NativeMethods.GetForegroundWindow() != expected.WindowHandle)
        {
            return false;
        }

        try
        {
            if (!WeixinProcessTrust.IsTrusted(expected.WindowHandle, out var processId, out _, out _)
                || processId != expected.ProcessId)
            {
                return false;
            }

            var root = AutomationElement.FromHandle(expected.WindowHandle);
            var candidateEditor = FindByAutomationIdSuffix(root, InputAutomationId);
            var groupTitle = FindByClassNameSuffix(root, GroupTitleClassNameSuffix);
            var titleElement = groupTitle is null
                ? FindByAutomationIdSuffix(root, ChatNameAutomationId)
                : FindByAutomationIdSuffix(groupTitle, ChatNameAutomationId);
            var title = ReadName(titleElement);
            ChatTargetKind? actualTargetKind = groupTitle is not null
                ? ChatTargetKind.Group
                : titleElement is not null && !string.IsNullOrWhiteSpace(title)
                    ? ChatTargetKind.Contact
                    : null;
            if (candidateEditor is null
                || actualTargetKind != expected.TargetKind
                || !string.Equals(expected.NormalizedChatTitle, ChatTitleNormalizer.Normalize(title), StringComparison.Ordinal))
            {
                return false;
            }

            editor = candidateEditor;
            return true;
        }
        catch (Exception ex) when (ex is ElementNotAvailableException or InvalidOperationException or COMException)
        {
            return false;
        }
    }

    private static string? NormalizeDraftPreview(string? text)
    {
        if (string.IsNullOrWhiteSpace(text))
        {
            return null;
        }

        var preview = text.Replace("\0", string.Empty).Trim();
        return preview.Length <= DraftPreviewLimit
            ? preview
            : preview[..DraftPreviewLimit] + "...";
    }

    private ChatContext Publish(ChatContext candidate)
    {
        var previous = Current;
        if (!SameObservation(previous, candidate))
        {
            candidate = candidate with { Generation = previous.Generation + 1 };
        }
        else
        {
            candidate = candidate with { Generation = previous.Generation };
        }

        Interlocked.Exchange(ref _current, candidate);
        if (candidate.IsTrustedWeixin && candidate.IsCompatibilityAvailable)
        {
            Interlocked.Exchange(ref _lastRecognizedWeixin, candidate);
        }
        if (!SameObservation(previous, candidate))
        {
            ContextChanged?.Invoke(this, candidate);
        }

        return candidate;
    }

    private void SubscribeAutomationEvents(AutomationElement root, AutomationElement? titleElement)
    {
        if (!_focusHandlerInstalled)
        {
            Automation.AddAutomationFocusChangedEventHandler(_focusChangedHandler);
            _focusHandlerInstalled = true;
        }

        if (_watchedRoot is not null && !Automation.Compare(_watchedRoot, root))
        {
            UnsubscribePropertyEvents();
        }

        if (_watchedTitle is not null && (titleElement is null || !Automation.Compare(_watchedTitle, titleElement)))
        {
            UnsubscribePropertyEvents();
        }

        _watchedRoot = root;
        if (titleElement is not null && _watchedTitle is null)
        {
            _watchedTitle = titleElement;
            Automation.AddAutomationPropertyChangedEventHandler(
                _watchedTitle,
                TreeScope.Element,
                _propertyChangedHandler,
                AutomationElement.NameProperty);
        }
    }

    private void UnsubscribeAutomationEvents()
    {
        UnsubscribePropertyEvents();
        if (_focusHandlerInstalled)
        {
            Automation.RemoveAutomationFocusChangedEventHandler(_focusChangedHandler);
            _focusHandlerInstalled = false;
        }

        _watchedRoot = null;
    }

    private void UnsubscribePropertyEvents()
    {
        if (_watchedTitle is not null)
        {
            Automation.RemoveAutomationPropertyChangedEventHandler(_watchedTitle, _propertyChangedHandler);
            _watchedTitle = null;
        }
    }

    private static AutomationElement? FindByAutomationIdSuffix(AutomationElement root, string suffix)
    {
        var elements = root.FindAll(
            TreeScope.Descendants,
            new PropertyCondition(AutomationElement.AutomationIdProperty, suffix));
        if (elements.Count > 0)
        {
            return elements[0];
        }

        // Some Weixin builds expose the full view path as the AutomationId.
        var all = root.FindAll(TreeScope.Descendants, System.Windows.Automation.Condition.TrueCondition);
        foreach (AutomationElement element in all)
        {
            try
            {
                if (element.Current.AutomationId.EndsWith(suffix, StringComparison.Ordinal))
                {
                    return element;
                }
            }
            catch (ElementNotAvailableException)
            {
                // The view was replaced while the tree was being read.
            }
        }

        return null;
    }

    private static AutomationElement? FindByClassNameSuffix(AutomationElement root, string suffix)
    {
        var exact = root.FindFirst(
            TreeScope.Descendants,
            new OrCondition(
                new PropertyCondition(AutomationElement.ClassNameProperty, GroupTitleClassName),
                new PropertyCondition(AutomationElement.ClassNameProperty, suffix)));
        if (exact is not null)
        {
            return exact;
        }

        var all = root.FindAll(TreeScope.Descendants, System.Windows.Automation.Condition.TrueCondition);
        foreach (AutomationElement element in all)
        {
            try
            {
                if (element.Current.ClassName.EndsWith(suffix, StringComparison.Ordinal))
                {
                    return element;
                }
            }
            catch (ElementNotAvailableException)
            {
                // The view was replaced while the tree was being read.
            }
        }

        return null;
    }

    private static string? ReadName(AutomationElement? element)
    {
        if (element is null)
        {
            return null;
        }

        try
        {
            var name = element.Current.Name;
            return string.IsNullOrWhiteSpace(name) ? null : name;
        }
        catch (ElementNotAvailableException)
        {
            return null;
        }
    }

    private static bool IsEditorFocused(AutomationElement? editor, AutomationElement root)
    {
        if (editor is null)
        {
            return false;
        }

        try
        {
            var focused = AutomationElement.FocusedElement;
            for (var current = focused; current is not null;)
            {
                if (Automation.Compare(current, editor))
                {
                    return true;
                }

                if (Automation.Compare(current, root))
                {
                    break;
                }

                current = TreeWalker.ControlViewWalker.GetParent(current);
            }
        }
        catch (ElementNotAvailableException)
        {
            // Focus changed while the tree was being read.
        }

        return false;
    }

    private static bool SameObservation(ChatContext left, ChatContext right)
    {
        return left.WindowHandle == right.WindowHandle
            && left.ProcessId == right.ProcessId
            && left.IsTrustedWeixin == right.IsTrustedWeixin
            && left.RequiresElevation == right.RequiresElevation
            && left.IsCompatibilityAvailable == right.IsCompatibilityAvailable
            && left.IsMessageEditorFocused == right.IsMessageEditorFocused
            && left.IsGroupChat == right.IsGroupChat
            && left.IsContactChat == right.IsContactChat
            && string.Equals(left.NormalizedChatTitle, right.NormalizedChatTitle, StringComparison.Ordinal);
    }
}
