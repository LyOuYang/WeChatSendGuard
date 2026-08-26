using WeChatSendGuard.Core.Guard;

namespace WeChatSendGuard.Core.Configuration;

public static class SettingsValidator
{
    public static AppSettings Sanitize(AppSettings? settings)
    {
        settings ??= new AppSettings();
        var confirmation = settings.Confirmation ?? new ConfirmationSettings();
        var protectedChats = SanitizeChatList(settings.ProtectedChats);
        var exemptedChats = SanitizeChatList(settings.ExemptedChats);

        return settings with
        {
            SchemaVersion = AppSettings.CurrentSchemaVersion,
            ProtectedChats = protectedChats,
            ExemptedChats = exemptedChats,
            Confirmation = confirmation with
            {
                HoldMilliseconds = Math.Clamp(confirmation.HoldMilliseconds, 500, 3000),
                Phrase = string.IsNullOrWhiteSpace(confirmation.Phrase) ? "确认发送" : confirmation.Phrase.Trim(),
                TimeoutSeconds = Math.Clamp(confirmation.TimeoutSeconds, 1, 30),
            },
            ShiftEnterPassThrough = true,
            LogRetentionDays = Math.Clamp(settings.LogRetentionDays, 1, 30),
        };
    }

    public static ProtectedChat SanitizeChat(ProtectedChat? chat)
    {
        chat ??= new ProtectedChat();
        var title = ChatTitleNormalizer.Normalize(chat.MatchTitle);
        var aliases = (chat.Aliases ?? [])
            .Select(ChatTitleNormalizer.Normalize)
            .Where(static alias => !string.IsNullOrEmpty(alias))
            .Where(alias => !string.Equals(alias, title, StringComparison.Ordinal))
            .Distinct(StringComparer.Ordinal)
            .ToList();

        return chat with
        {
            Id = chat.Id == Guid.Empty ? Guid.NewGuid() : chat.Id,
            DisplayName = string.IsNullOrWhiteSpace(chat.DisplayName) ? title : chat.DisplayName.Trim(),
            MatchTitle = title,
            Aliases = aliases,
        };
    }

    public static List<ProtectedChat> SanitizeChatList(IEnumerable<ProtectedChat>? chats)
    {
        return (chats ?? [])
            .Select(SanitizeChat)
            .Where(static chat => !string.IsNullOrEmpty(chat.MatchTitle))
            .GroupBy(static chat => new ChatTargetKey(chat.TargetKind, chat.MatchTitle), ChatTargetKeyComparer.Instance)
            .Select(static group => group.First())
            .ToList();
    }

    private readonly record struct ChatTargetKey(ChatTargetKind TargetKind, string MatchTitle);

    private sealed class ChatTargetKeyComparer : IEqualityComparer<ChatTargetKey>
    {
        public static ChatTargetKeyComparer Instance { get; } = new();

        public bool Equals(ChatTargetKey x, ChatTargetKey y) => x.TargetKind == y.TargetKind
            && string.Equals(x.MatchTitle, y.MatchTitle, StringComparison.Ordinal);

        public int GetHashCode(ChatTargetKey value) => HashCode.Combine(value.TargetKind, StringComparer.Ordinal.GetHashCode(value.MatchTitle));
    }
}
