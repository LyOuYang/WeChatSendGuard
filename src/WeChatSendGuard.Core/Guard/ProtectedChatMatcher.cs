using WeChatSendGuard.Core.Configuration;

namespace WeChatSendGuard.Core.Guard;

public static class ProtectedChatMatcher
{
    public static ProtectionDecision Evaluate(ChatContext context, AppSettings settings, TemporaryBypassRegistry bypasses, DateTimeOffset now)
    {
        ArgumentNullException.ThrowIfNull(context);
        ArgumentNullException.ThrowIfNull(settings);
        ArgumentNullException.ThrowIfNull(bypasses);

        if (!settings.Enabled || !context.IsTrustedWeixin || !context.IsCompatibilityAvailable || !context.IsMessageEditorFocused)
        {
            return ProtectionDecision.Pass;
        }

        var title = context.NormalizedChatTitle;
        if (!context.IsKnownChat || string.IsNullOrEmpty(title) || context.TargetKind is null)
        {
            return settings.UnknownContextBehavior == UnknownContextBehavior.Block
                ? new ProtectionDecision(ProtectionDecisionKind.BlockUnknown)
                : new ProtectionDecision(ProtectionDecisionKind.ConfirmUnknown);
        }

        var targetKind = context.TargetKind.Value;
        if (settings.RuleMode == RuleMode.ConfirmUnlessExcluded)
        {
            var exemption = settings.ExemptedChats.FirstOrDefault(chat => chat.Enabled && chat.TargetKind == targetKind && TitleMatches(chat, title));
            return exemption is not null
                ? ProtectionDecision.Pass
                : new ProtectionDecision(ProtectionDecisionKind.ConfirmUnlisted);
        }

        var match = settings.ProtectedChats.FirstOrDefault(chat => chat.Enabled
            && chat.TargetKind == targetKind
            && TitleMatches(chat, title));
        if (match is null || bypasses.IsActive(match.Id, now))
        {
            return ProtectionDecision.Pass;
        }

        return new ProtectionDecision(ProtectionDecisionKind.ConfirmProtected, match);
    }

    public static bool TitleMatches(ProtectedChat chat, string normalizedTitle)
    {
        if (string.Equals(chat.MatchTitle, normalizedTitle, StringComparison.Ordinal))
        {
            return true;
        }

        foreach (var alias in chat.Aliases ?? [])
        {
            if (string.Equals(alias, normalizedTitle, StringComparison.Ordinal))
            {
                return true;
            }
        }

        return false;
    }
}
