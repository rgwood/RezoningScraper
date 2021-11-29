namespace RezoningScraper;

internal class ProjectComparer
{
    private readonly Datum _oldVersion;
    private readonly Datum _newVersion;

    internal record AttributeChange(string OldValue, string NewValue);

    public ProjectComparer(Datum oldVersion, Datum newVersion)
    {
        this._oldVersion = oldVersion;
        this._newVersion = newVersion;
    }

    internal bool DidProjectChange(Datum oldVersion, Datum newVersion, out Dictionary<string, AttributeChange> changedAttributes)
    {
        bool changed = false;
        // todo: check more fields (all attributes), and return data about what exactly changed
        changedAttributes = new Dictionary<string, AttributeChange>();

        var oldAttrs = oldVersion.attributes!;
        var newAttrs = newVersion.attributes!;




        if (oldAttrs.name != newAttrs.name)
        {
            changed = true;
            changedAttributes.Add("Name", new(oldAttrs.name, newAttrs.name));
        }

        return changed;
    }

    //internal void CompareAttribute(Attributes oldAttributes, Attributes newAttributes, out bool changed, out bool 
}
