using System.Text.Json;
using System.Text.Json.Serialization;

namespace WeChatSendGuard.Core.Configuration;

public sealed class FileSettingsStore(string path) : ISettingsStore
{
    private static readonly JsonSerializerOptions SerializerOptions = new()
    {
        WriteIndented = true,
        PropertyNamingPolicy = JsonNamingPolicy.CamelCase,
        Converters = { new JsonStringEnumConverter() },
    };

    public string Path { get; } = path;

    public async Task<AppSettings> LoadAsync(CancellationToken cancellationToken = default)
    {
        if (!File.Exists(Path))
        {
            return SettingsValidator.Sanitize(new AppSettings());
        }

        try
        {
            await using var stream = File.OpenRead(Path);
            var settings = await JsonSerializer.DeserializeAsync<AppSettings>(stream, SerializerOptions, cancellationToken);
            return SettingsValidator.Sanitize(settings);
        }
        catch (JsonException)
        {
            return SettingsValidator.Sanitize(new AppSettings());
        }
    }

    public async Task SaveAsync(AppSettings settings, CancellationToken cancellationToken = default)
    {
        var sanitized = SettingsValidator.Sanitize(settings);
        var directory = System.IO.Path.GetDirectoryName(Path)
            ?? throw new InvalidOperationException("The settings path must have a parent directory.");
        Directory.CreateDirectory(directory);

        var temporaryPath = System.IO.Path.Combine(directory, $".{System.IO.Path.GetFileName(Path)}.{Guid.NewGuid():N}.tmp");
        try
        {
            await using (var stream = File.Create(temporaryPath))
            {
                await JsonSerializer.SerializeAsync(stream, sanitized, SerializerOptions, cancellationToken);
            }

            File.Move(temporaryPath, Path, overwrite: true);
        }
        finally
        {
            if (File.Exists(temporaryPath))
            {
                File.Delete(temporaryPath);
            }
        }
    }
}
