using System.Reflection;

namespace RezoningScraperTests;
internal static class SerializationTestsHelpers
{
    public static string ReadResource(string name)
    {
        // Determine path
        var assembly = Assembly.GetExecutingAssembly();
        string resourcePath = name;
        // Format: "{Namespace}.{Folder}.{filename}.{Extension}"
        if (!name.StartsWith(nameof(RezoningScraperTests)))
        {
            resourcePath = assembly.GetManifestResourceNames()
                .Single(str => str.EndsWith(name));
        }

        using (Stream stream = assembly.GetManifestResourceStream(resourcePath)!)
        using (StreamReader reader = new StreamReader(stream))
        {
            return reader.ReadToEnd();
        }
    }
}
