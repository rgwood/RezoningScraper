using System.Linq.Expressions;

namespace RezoningScraper;

internal record AttributeChange(string? OldValue, string? NewValue);

/// <summary>
/// Handles comparing 2 versions of the same project to see if they've changed enough to report on.
/// It's pretty subjective which fields we consider important; right now it's just a small number of them.
/// </summary>
internal class ProjectComparer
{
    private readonly Project _oldVersion;
    private readonly Project _newVersion;

    private Attributes OldAttributes => _oldVersion.attributes!;
    private Attributes NewAttributes => _newVersion.attributes!;

    public ProjectComparer(Project oldVersion, Project newVersion)
    {
        _oldVersion = oldVersion;
        _newVersion = newVersion;
    }

    internal bool DidProjectChange(out Dictionary<string, AttributeChange> changes)
    {
        bool changed = false;
        changes = new Dictionary<string, AttributeChange>();

        Compare(ref changed, in changes, a => a.name);
        Compare(ref changed, in changes, a => a.permalink);
        Compare(ref changed, in changes, a => a.state);
        Compare(ref changed, in changes, a => a.description);

        return changed;
    }


    //  https://stackoverflow.com/a/32992324/854694
    internal void Compare(ref bool changed, in Dictionary<string, AttributeChange> changes, Expression<Func<Attributes, string?>> expression)
    {
        string attributeName = ((MemberExpression)expression.Body).Member.Name;
        var compiled = expression.Compile();

        string? oldValue = compiled(OldAttributes);
        string? newValue = compiled(NewAttributes);

        if (oldValue == newValue)
        {
            return;
        }
        else
        {
            changed = true;
            changes.Add(attributeName, new AttributeChange(oldValue, newValue));
        }
    }
}
