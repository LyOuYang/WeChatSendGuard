using WeChatSendGuard.App.Interop;
using WeChatSendGuard.Core.Configuration;
using WeChatSendGuard.Core.Guard;
using WeChatSendGuard.Core.Logging;

namespace WeChatSendGuard.App.Services;

internal sealed class InputGateController : IDisposable
{
    private static readonly TimeSpan FocusRecoveryTimeout = TimeSpan.FromMilliseconds(900);
    private static readonly TimeSpan FocusRecoveryRetryDelay = TimeSpan.FromMilliseconds(30);

    private readonly LowLevelKeyboardHook _keyboardHook;
    private readonly WeixinContextMonitor _contextMonitor;
    private readonly SendGuardStateMachine _stateMachine;
    private readonly TemporaryBypassRegistry _bypasses;
    private readonly IConfirmationService _confirmationService;
    private readonly IInputInjector _inputInjector;
    private readonly IAuditLog _auditLog;
    private readonly object _confirmationSync = new();
    private AppSettings _settings;
    private ActiveConfirmation? _activeConfirmation;
    private int _suppressPhysicalEnterKeyUp;
    private nint _suppressedWindowHandle;
    private bool _disposed;

    public InputGateController(
        LowLevelKeyboardHook keyboardHook,
        WeixinContextMonitor contextMonitor,
        SendGuardStateMachine stateMachine,
        TemporaryBypassRegistry bypasses,
        IConfirmationService confirmationService,
        IInputInjector inputInjector,
        IAuditLog auditLog,
        AppSettings settings)
    {
        _keyboardHook = keyboardHook;
        _contextMonitor = contextMonitor;
        _stateMachine = stateMachine;
        _bypasses = bypasses;
        _confirmationService = confirmationService;
        _inputInjector = inputInjector;
        _auditLog = auditLog;
        _settings = settings;
        _keyboardHook.KeyDown += HandleKeyDown;
        _keyboardHook.KeyUp += HandleKeyUp;
        _contextMonitor.ContextChanged += ContextMonitor_ContextChanged;
    }

    public void Start() => _keyboardHook.Start();

    public void UpdateSettings(AppSettings settings) => Interlocked.Exchange(ref _settings, settings);

    public bool TryGrantCurrentChatBypass(int minutes, out string protectedChatName)
    {
        protectedChatName = string.Empty;
        if (minutes is not (1 or 5 or 15))
        {
            return false;
        }

        var context = _contextMonitor.LastRecognizedWeixin;
        var decision = ProtectedChatMatcher.Evaluate(context, Volatile.Read(ref _settings), _bypasses, DateTimeOffset.UtcNow);
        if (decision.Kind != ProtectionDecisionKind.ConfirmProtected || decision.ProtectedChat is null)
        {
            return false;
        }

        _bypasses.Grant(decision.ProtectedChat.Id, TimeSpan.FromMinutes(minutes), DateTimeOffset.UtcNow);
        protectedChatName = decision.ProtectedChat.DisplayName;
        WriteAudit(decision.ProtectedChat.Id, "temporary-bypass", $"granted-{minutes}m");
        return true;
    }

    public void Dispose()
    {
        if (_disposed)
        {
            return;
        }

        _disposed = true;
        CancelPendingConfirmation(null, writeAudit: false);
        _keyboardHook.KeyDown -= HandleKeyDown;
        _keyboardHook.KeyUp -= HandleKeyUp;
        _contextMonitor.ContextChanged -= ContextMonitor_ContextChanged;
        _keyboardHook.Dispose();
    }

    private bool HandleKeyDown(KeyboardStroke stroke)
    {
        if (_disposed || stroke.IsInjected || stroke.VirtualKey != NativeMethods.VkReturn)
        {
            return false;
        }

        var settings = Volatile.Read(ref _settings);
        if (!settings.InterceptKeyboardEnter)
        {
            return false;
        }

        if (stroke.IsNumpadEnter && !settings.InterceptNumpadEnter)
        {
            return false;
        }

        if (settings.ShiftEnterPassThrough && LowLevelKeyboardHook.IsKeyDown((int)NativeMethods.VkShift))
        {
            return false;
        }

        var context = _contextMonitor.Current;
        if (NativeMethods.GetForegroundWindow() != context.WindowHandle)
        {
            return false;
        }

        if (!context.IsTrustedWeixin || !context.IsCompatibilityAvailable || !context.IsMessageEditorFocused)
        {
            return false;
        }

        if (ImeCompositionDetector.IsComposing(context.WindowHandle))
        {
            return false;
        }

        var decision = ProtectedChatMatcher.Evaluate(context, settings, _bypasses, DateTimeOffset.UtcNow);
        if (!decision.ShouldSuppress)
        {
            return false;
        }

        Interlocked.Exchange(ref _suppressedWindowHandle, context.WindowHandle);
        Interlocked.Exchange(ref _suppressPhysicalEnterKeyUp, 1);
        if (decision.Kind == ProtectionDecisionKind.BlockUnknown)
        {
            QueueCallbackWork(() => WriteAudit(null, "send-blocked", "unknown-chat"));
            return true;
        }

        if (_stateMachine.TryBegin(
            context,
            decision,
            stroke.IsNumpadEnter,
            TimeSpan.FromSeconds(settings.Confirmation.TimeoutSeconds),
            DateTimeOffset.UtcNow,
            out var pending))
        {
            var cancellation = RegisterConfirmation(pending!);
            QueueCallbackWork(() => _ = ConfirmAndMaybeSendAsync(pending!, settings, cancellation));
        }

        return true;
    }

    private bool HandleKeyUp(KeyboardStroke stroke)
    {
        var shouldSuppress = !_disposed
            && !stroke.IsInjected
            && stroke.VirtualKey == NativeMethods.VkReturn
            && Interlocked.Exchange(ref _suppressPhysicalEnterKeyUp, 0) == 1;
        if (!shouldSuppress)
        {
            return false;
        }

        var suppressedWindow = Interlocked.Exchange(ref _suppressedWindowHandle, nint.Zero);
        return NativeMethods.GetForegroundWindow() == suppressedWindow;
    }

    private async Task ConfirmAndMaybeSendAsync(PendingConfirmation pending, AppSettings settings, CancellationTokenSource cancellation)
    {
        var cancellationToken = cancellation.Token;
        try
        {
            if (cancellationToken.IsCancellationRequested || _stateMachine.Current?.AttemptId != pending.AttemptId)
            {
                return;
            }

            var displayPending = pending with
            {
                DraftPreview = await _contextMonitor.TryReadDraftPreviewAsync(pending.OriginalContext).ConfigureAwait(false),
            };
            if (cancellationToken.IsCancellationRequested)
            {
                return;
            }

            var outcome = await _confirmationService.ConfirmAsync(displayPending, settings.Confirmation, cancellationToken).ConfigureAwait(false);
            if (cancellationToken.IsCancellationRequested)
            {
                return;
            }

            if (outcome != ConfirmationOutcome.Confirmed)
            {
                _stateMachine.Resolve(pending.AttemptId, outcome, ChatContext.Inactive, DateTimeOffset.UtcNow);
                WriteAudit(pending.Decision.ProtectedChat?.Id, "confirmation", outcome.ToString().ToLowerInvariant());
                return;
            }

            WriteSendDiagnostic(pending, "confirmation-confirmed");
            var focusRecovery = await RestoreEditorFocusAndRevalidateAsync(pending, cancellationToken).ConfigureAwait(false);
            if (!focusRecovery.Succeeded)
            {
                _stateMachine.Resolve(pending.AttemptId, ConfirmationOutcome.Confirmed, ChatContext.Inactive, DateTimeOffset.UtcNow);
                WriteAudit(pending.Decision.ProtectedChat?.Id, "send", "cancelled-editor-focus");
                WriteSendDiagnostic(pending, focusRecovery.ToAuditResult());
                return;
            }

            WriteSendDiagnostic(pending, focusRecovery.ToAuditResult());
            var resolution = _stateMachine.Resolve(pending.AttemptId, ConfirmationOutcome.Confirmed, focusRecovery.Context!, DateTimeOffset.UtcNow);
            if (!resolution.ShouldInject)
            {
                WriteAudit(pending.Decision.ProtectedChat?.Id, "send", "cancelled-context-changed");
                WriteSendDiagnostic(pending, "state-resolution-rejected");
                return;
            }

            try
            {
                _inputInjector.SendEnter(pending.IsNumpadEnter);
                WriteAudit(pending.Decision.ProtectedChat?.Id, "send", "injected");
                WriteSendDiagnostic(pending, "send-input-complete");
            }
            catch (InputInjectionException ex)
            {
                WriteAudit(pending.Decision.ProtectedChat?.Id, "send", "injection-failed");
                WriteSendDiagnostic(
                    pending,
                    $"injection-failed;stage={ex.Stage};input-size={ex.InputSize};sent={ex.SentInputCount};win32={ex.Win32Error}");
            }
            catch (Exception ex)
            {
                WriteAudit(pending.Decision.ProtectedChat?.Id, "send", "injection-failed");
                WriteSendDiagnostic(pending, $"injection-failed;exception={ex.GetType().Name}");
            }
        }
        catch (OperationCanceledException) when (cancellationToken.IsCancellationRequested)
        {
        }
        catch
        {
            _stateMachine.CancelActive();
            WriteAudit(pending.Decision.ProtectedChat?.Id, "confirmation", "error");
        }
        finally
        {
            ReleaseConfirmation(pending.AttemptId, cancellation);
        }
    }

    private void ContextMonitor_ContextChanged(object? sender, ChatContext context)
    {
        var pending = _stateMachine.Current;
        if (_disposed || pending is null)
        {
            return;
        }

        // The confirmation window temporarily takes focus. When it closes,
        // Weixin can report the same chat before its editor has regained focus.
        // Preserve this one pending send; the final revalidation below still
        // requires the editor focus before injecting Enter.
        if (context.IsTrustedWeixin && SendGuardStateMachine.RepresentsSameSession(pending.OriginalContext, context))
        {
            return;
        }

        if (!context.IsTrustedWeixin
            && _confirmationService.OwnsWindow(NativeMethods.GetForegroundWindow())
            && NativeMethods.IsWindow(pending.OriginalContext.WindowHandle))
        {
            return;
        }

        CancelPendingConfirmation(pending.AttemptId, writeAudit: true);
    }

    private CancellationTokenSource RegisterConfirmation(PendingConfirmation pending)
    {
        var cancellation = new CancellationTokenSource();
        lock (_confirmationSync)
        {
            _activeConfirmation?.Cancellation.Cancel();
            _activeConfirmation = new ActiveConfirmation(pending.AttemptId, cancellation);
        }

        return cancellation;
    }

    private void ReleaseConfirmation(Guid attemptId, CancellationTokenSource cancellation)
    {
        lock (_confirmationSync)
        {
            if (_activeConfirmation?.AttemptId == attemptId)
            {
                _activeConfirmation = null;
            }
        }

        cancellation.Dispose();
    }

    private void CancelPendingConfirmation(Guid? attemptId, bool writeAudit)
    {
        var pending = _stateMachine.Current;
        if (pending is null || (attemptId is not null && pending.AttemptId != attemptId.Value))
        {
            return;
        }

        lock (_confirmationSync)
        {
            if (_activeConfirmation?.AttemptId == pending.AttemptId)
            {
                _activeConfirmation.Cancellation.Cancel();
            }
        }

        _stateMachine.CancelActive();
        _confirmationService.CancelActive();
        if (writeAudit)
        {
            WriteAudit(pending.Decision.ProtectedChat?.Id, "confirmation", "cancelled-context-changed");
        }
    }

    private static void QueueCallbackWork(Action action)
    {
        ThreadPool.QueueUserWorkItem(_ =>
        {
            try
            {
                action();
            }
            catch
            {
                // Background follow-up must never interfere with the keyboard hook.
            }
        });
    }

    private async Task<FocusRecoveryResult> RestoreEditorFocusAndRevalidateAsync(
        PendingConfirmation pending,
        CancellationToken cancellationToken)
    {
        var deadline = DateTimeOffset.UtcNow + FocusRecoveryTimeout;
        var attempts = 0;
        var foregroundFailures = 0;
        var focusRestoreFailures = 0;
        var targetRevalidationFailures = 0;
        while (DateTimeOffset.UtcNow < deadline)
        {
            cancellationToken.ThrowIfCancellationRequested();
            if (_stateMachine.Current?.AttemptId != pending.AttemptId)
            {
                return FocusRecoveryResult.Failed("pending-not-active", attempts, foregroundFailures, focusRestoreFailures, targetRevalidationFailures);
            }

            attempts++;
            NativeMethods.SetForegroundWindow(pending.OriginalContext.WindowHandle);
            if (NativeMethods.GetForegroundWindow() != pending.OriginalContext.WindowHandle)
            {
                foregroundFailures++;
                await Task.Delay(FocusRecoveryRetryDelay, cancellationToken).ConfigureAwait(false);
                continue;
            }

            if (!await _contextMonitor.TryRestoreMessageEditorFocusAsync(pending.OriginalContext).ConfigureAwait(false))
            {
                focusRestoreFailures++;
                await Task.Delay(FocusRecoveryRetryDelay, cancellationToken).ConfigureAwait(false);
                continue;
            }

            await Task.Delay(FocusRecoveryRetryDelay, cancellationToken).ConfigureAwait(false);
            var context = await _contextMonitor.RefreshNowAsync().ConfigureAwait(false);
            if (SendGuardStateMachine.RepresentsSameSendTarget(pending.OriginalContext, context))
            {
                return FocusRecoveryResult.Success(context, attempts, foregroundFailures, focusRestoreFailures, targetRevalidationFailures);
            }

            targetRevalidationFailures++;
            await Task.Delay(FocusRecoveryRetryDelay, cancellationToken).ConfigureAwait(false);
        }

        var reason = foregroundFailures == attempts
            ? "foreground-window-not-restored"
            : focusRestoreFailures + targetRevalidationFailures == attempts
                ? "editor-or-target-not-restored"
                : "focus-recovery-timeout";
        return FocusRecoveryResult.Failed(reason, attempts, foregroundFailures, focusRestoreFailures, targetRevalidationFailures);
    }

    private sealed record ActiveConfirmation(Guid AttemptId, CancellationTokenSource Cancellation);

    private sealed record FocusRecoveryResult(
        ChatContext? Context,
        string Result,
        int Attempts,
        int ForegroundFailures,
        int FocusRestoreFailures,
        int TargetRevalidationFailures)
    {
        public bool Succeeded => Context is not null;

        public static FocusRecoveryResult Success(
            ChatContext context,
            int attempts,
            int foregroundFailures,
            int focusRestoreFailures,
            int targetRevalidationFailures) =>
            new(context, "ready", attempts, foregroundFailures, focusRestoreFailures, targetRevalidationFailures);

        public static FocusRecoveryResult Failed(
            string result,
            int attempts,
            int foregroundFailures,
            int focusRestoreFailures,
            int targetRevalidationFailures) =>
            new(null, result, attempts, foregroundFailures, focusRestoreFailures, targetRevalidationFailures);

        public string ToAuditResult() =>
            $"focus-recovery;result={Result};attempts={Attempts};foreground-failures={ForegroundFailures};focus-failures={FocusRestoreFailures};target-failures={TargetRevalidationFailures}";
    }

    private void WriteAudit(Guid? protectedChatId, string eventType, string result)
    {
        _ = _auditLog.WriteAsync(new AuditEntry(DateTimeOffset.UtcNow, protectedChatId, eventType, result));
    }

    private void WriteSendDiagnostic(PendingConfirmation pending, string result)
    {
        var attempt = pending.AttemptId.ToString("N")[..8];
        WriteAudit(pending.Decision.ProtectedChat?.Id, "send-diagnostic", $"attempt={attempt};{result}");
    }
}
