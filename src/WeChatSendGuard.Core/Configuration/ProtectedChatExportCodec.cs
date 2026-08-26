using System.Text.Json;
using System.Text.Json.Serialization;

namespace WeChatSendGuard.Core.Configuration;

public sealed record ProtectedChatExport(int SchemaVersion, List<ProtectedChat> ProtectedChats);

public static class ProtectedChatExportCodec
{
    private static readonly JsonSerializerOptions SerializerOptions = new()
    {
        WriteIndented = true,
        PropertyNamingPolicy = JsonNamingPolicy.CamelCase,
        Converters = { new JsonStringEnumConverter() },
    };

    public static string Export(IEnumerable<ProtectedChat> chats)
    {
        var sanitized = (chats ?? [])
            .Select(SettingsValidator.SanitizeChat)
            .Where(static chat => !string.IsNullOrEmpty(chat.MatchTitle))
            .ToList();
        return JsonSerializer.Serialize(new ProtectedChatExport(AppSettings.CurrentSchemaVersion, sanitized), SerializerOptions);
    }

    public static IReadOnlyList<ProtectedChat> Import(string json)
    {
        var payload = JsonSerializer.Deserialize<ProtectedChatExport>(json, SerializerOptions)
            ?? throw new JsonException("The import file is empty.");
        if (payload.SchemaVersion is not (1 or AppSettings.CurrentSchemaVersion))
        {
            throw new JsonException("The import file uses an unsupported schema version.");
        }

        return (payload.ProtectedChats ?? [])
            .Select(SettingsValidator.SanitizeChat)
            .Where(static chat => !string.IsNullOrEmpty(chat.MatchTitle))
            .GroupBy(static chat => (chat.TargetKind, chat.MatchTitle))
            .Select(static group => group.First())
            .ToList();
    }
}
