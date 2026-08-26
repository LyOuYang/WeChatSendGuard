using System.Text.Json.Serialization;

namespace WeChatSendGuard.Core.Configuration;

public enum ConfirmationMode
{
    Click,
    Hold,
    Phrase,
}

public enum UnknownContextBehavior
{
    Confirm,
    Block,
}

public enum ChatTargetKind
{
    Group,
    Contact,
}

public enum RuleMode
{
    ProtectListed,
    ConfirmUnlessExcluded,
}

public sealed record ConfirmationSettings
{
    public ConfirmationMode Mode { get; init; } = ConfirmationMode.Hold;

    public int HoldMilliseconds { get; init; } = 800;

    public string Phrase { get; init; } = "确认发送";

    public int TimeoutSeconds { get; init; } = 10;
}

public sealed record ProtectedChat
{
    public Guid Id { get; init; } = Guid.NewGuid();

    public string DisplayName { get; init; } = string.Empty;

    public string MatchTitle { get; init; } = string.Empty;

    public List<string> Aliases { get; init; } = [];

    public bool Enabled { get; init; } = true;

    public ChatTargetKind TargetKind { get; init; } = ChatTargetKind.Group;

    [JsonIgnore]
    public string DisplayNameWithKind => TargetKind == ChatTargetKind.Contact
        ? $"[联系人] {DisplayName}"
        : $"[群聊] {DisplayName}";
}

public sealed record AppSettings
{
    public const int CurrentSchemaVersion = 2;

    public int SchemaVersion { get; init; } = CurrentSchemaVersion;

    public bool Enabled { get; init; } = true;

    public RuleMode RuleMode { get; init; } = RuleMode.ProtectListed;

    // In ProtectListed mode these entries require confirmation.
    public List<ProtectedChat> ProtectedChats { get; init; } = [];

    // In ConfirmUnlessExcluded mode these entries bypass confirmation.
    public List<ProtectedChat> ExemptedChats { get; init; } = [];

    public ConfirmationSettings Confirmation { get; init; } = new();

    public UnknownContextBehavior UnknownContextBehavior { get; init; } = UnknownContextBehavior.Confirm;

    public bool InterceptNumpadEnter { get; init; } = true;

    // Main keyboard Enter interception. Numpad Enter is controlled separately
    // and is ignored whenever this switch is disabled.
    public bool InterceptKeyboardEnter { get; init; } = true;

    public bool ShiftEnterPassThrough { get; init; } = true;

    public bool StartWithWindows { get; init; }

    public int LogRetentionDays { get; init; } = 7;
}
