using System.Globalization;
using System.Windows;
using System.Windows.Data;
using System.Windows.Media;
using WeChatSendGuard.Core.Configuration;

namespace WeChatSendGuard.App.Converters;

public class ChatTargetKindToTextConverter : IValueConverter
{
    public object Convert(object value, Type targetType, object parameter, CultureInfo culture)
    {
        return value is ChatTargetKind kind && kind == ChatTargetKind.Contact ? "联系人" : "群聊";
    }

    public object ConvertBack(object value, Type targetType, object parameter, CultureInfo culture) => throw new NotSupportedException();
}

public class ChatTargetKindToBadgeBgConverter : IValueConverter
{
    public object Convert(object value, Type targetType, object parameter, CultureInfo culture)
    {
        if (Application.Current?.Resources is { } res)
        {
            return value is ChatTargetKind kind && kind == ChatTargetKind.Contact
                ? res["BadgeContactBgBrush"]
                : res["BadgeGroupBgBrush"];
        }

        return Brushes.LightGray;
    }

    public object ConvertBack(object value, Type targetType, object parameter, CultureInfo culture) => throw new NotSupportedException();
}

public class ChatTargetKindToBadgeFgConverter : IValueConverter
{
    public object Convert(object value, Type targetType, object parameter, CultureInfo culture)
    {
        if (Application.Current?.Resources is { } res)
        {
            return value is ChatTargetKind kind && kind == ChatTargetKind.Contact
                ? res["BadgeContactFgBrush"]
                : res["BadgeGroupFgBrush"];
        }

        return Brushes.Black;
    }

    public object ConvertBack(object value, Type targetType, object parameter, CultureInfo culture) => throw new NotSupportedException();
}

public class AliasesToSummaryConverter : IValueConverter
{
    public object Convert(object value, Type targetType, object parameter, CultureInfo culture)
    {
        if (value is IEnumerable<string> aliases)
        {
            var list = aliases.Where(s => !string.IsNullOrWhiteSpace(s)).ToList();
            if (list.Count > 0)
            {
                return $"别名: {string.Join(", ", list)}";
            }
        }

        return string.Empty;
    }

    public object ConvertBack(object value, Type targetType, object parameter, CultureInfo culture) => throw new NotSupportedException();
}

public class StringNotEmptyToVisibilityConverter : IValueConverter
{
    public object Convert(object value, Type targetType, object parameter, CultureInfo culture)
    {
        return !string.IsNullOrWhiteSpace(value as string) ? Visibility.Visible : Visibility.Collapsed;
    }

    public object ConvertBack(object value, Type targetType, object parameter, CultureInfo culture) => throw new NotSupportedException();
}
