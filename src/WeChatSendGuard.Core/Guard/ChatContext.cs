using WeChatSendGuard.Core.Configuration;

namespace WeChatSendGuard.Core.Guard;

public sealed record ChatContext
{
    public nint WindowHandle { get; init; }

    public int ProcessId { get; init; }

    public string ProcessPath { get; init; } = string.Empty;

    public bool IsTrustedWeixin { get; init; }

    public bool RequiresElevation { get; init; }

    public bool IsCompatibilityAvailable { get; init; }

    public bool IsMessageEditorFocused { get; init; }

    public bool IsGroupChat { get; init; }

    public bool IsContactChat { get; init; }

    public bool IsKnownChat => IsGroupChat || IsContactChat;

    public ChatTargetKind? TargetKind => IsGroupChat
        ? ChatTargetKind.Group
        : IsContactChat
            ? ChatTargetKind.Contact
            : null;

    public string? ChatTitle { get; init; }

    public long Generation { get; init; }

    public DateTimeOffset ObservedAt { get; init; }

    public string NormalizedChatTitle => ChatTitleNormalizer.Normalize(ChatTitle);

    public static ChatContext Inactive { get; } = new();
}
