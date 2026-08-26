using WeChatSendGuard.Core.Configuration;

namespace WeChatSendGuard.Core.Guard;

public enum ProtectionDecisionKind
{
    Pass,
    ConfirmProtected,
    ConfirmUnlisted,
    ConfirmUnknown,
    BlockUnknown,
}

public sealed record ProtectionDecision(ProtectionDecisionKind Kind, ProtectedChat? ProtectedChat = null)
{
    public bool RequiresConfirmation => Kind is ProtectionDecisionKind.ConfirmProtected or ProtectionDecisionKind.ConfirmUnlisted or ProtectionDecisionKind.ConfirmUnknown;

    public bool ShouldSuppress => Kind is not ProtectionDecisionKind.Pass;

    public static ProtectionDecision Pass { get; } = new(ProtectionDecisionKind.Pass);
}
