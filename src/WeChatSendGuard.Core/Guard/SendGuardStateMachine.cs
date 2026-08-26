namespace WeChatSendGuard.Core.Guard;

public enum ConfirmationOutcome
{
    Confirmed,
    Cancelled,
    TimedOut,
}

public sealed record PendingConfirmation(
    Guid AttemptId,
    ChatContext OriginalContext,
    ProtectionDecision Decision,
    bool IsNumpadEnter,
    DateTimeOffset CreatedAt,
    DateTimeOffset ExpiresAt)
{
    // This is displayed only in the confirmation window and is never persisted or logged.
    public string? DraftPreview { get; init; }
}

public sealed record ConfirmationResolution(bool ShouldInject, string Reason)
{
    public static ConfirmationResolution Rejected(string reason) => new(false, reason);

    public static ConfirmationResolution Accepted { get; } = new(true, "Confirmed");
}

public sealed class SendGuardStateMachine
{
    private readonly object _sync = new();
    private PendingConfirmation? _pending;

    public PendingConfirmation? Current
    {
        get
        {
            lock (_sync)
            {
                return _pending;
            }
        }
    }

    public bool TryBegin(
        ChatContext context,
        ProtectionDecision decision,
        bool isNumpadEnter,
        TimeSpan timeout,
        DateTimeOffset now,
        out PendingConfirmation? pending)
    {
        lock (_sync)
        {
            if (_pending is not null || !decision.RequiresConfirmation)
            {
                pending = null;
                return false;
            }

            var expiry = now.Add(timeout);
            _pending = new PendingConfirmation(Guid.NewGuid(), context, decision, isNumpadEnter, now, expiry);
            pending = _pending;
            return true;
        }
    }

    public ConfirmationResolution Resolve(Guid attemptId, ConfirmationOutcome outcome, ChatContext revalidatedContext, DateTimeOffset now)
    {
        PendingConfirmation? pending;
        lock (_sync)
        {
            if (_pending is null || _pending.AttemptId != attemptId)
            {
                return ConfirmationResolution.Rejected("The confirmation is no longer active.");
            }

            pending = _pending;
            _pending = null;
        }

        if (outcome != ConfirmationOutcome.Confirmed)
        {
            return ConfirmationResolution.Rejected(outcome.ToString());
        }

        if (now > pending.ExpiresAt)
        {
            return ConfirmationResolution.Rejected("Confirmation timed out.");
        }

        if (!RepresentsSameSendTarget(pending.OriginalContext, revalidatedContext))
        {
            return ConfirmationResolution.Rejected("The chat changed before confirmation completed.");
        }

        return ConfirmationResolution.Accepted;
    }

    public void CancelActive()
    {
        lock (_sync)
        {
            _pending = null;
        }
    }

    public static bool RepresentsSameSendTarget(ChatContext original, ChatContext current)
    {
        return RepresentsSameSession(original, current)
            && current.IsMessageEditorFocused;
    }

    // During the hand-off from the confirmation window back to Weixin the
    // editor can briefly lose focus. Keep the pending confirmation alive for
    // that transient condition; the final send check still requires focus.
    public static bool RepresentsSameSession(ChatContext original, ChatContext current)
    {
        if (!current.IsTrustedWeixin || !current.IsCompatibilityAvailable)
        {
            return false;
        }

        if (!original.IsKnownChat || !current.IsKnownChat || original.TargetKind != current.TargetKind)
        {
            return false;
        }

        if (original.WindowHandle != current.WindowHandle || original.ProcessId != current.ProcessId)
        {
            return false;
        }

        var originalTitle = original.NormalizedChatTitle;
        var currentTitle = current.NormalizedChatTitle;
        return !string.IsNullOrEmpty(originalTitle)
            && string.Equals(originalTitle, currentTitle, StringComparison.Ordinal);
    }
}
