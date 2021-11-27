using FluentAssertions;
using Xunit;
using RezoningScraper2;
using System.IO;
using System.Reflection;
using System.Text.Json;

namespace RezoningScraperTests;

public class Tests
{
    [Fact]
    public void CanDeserialize()
    {
        string json = ReadResource("ExampleInput.json");
        json.Should().NotBeNullOrEmpty();

        var result = JsonSerializer.Deserialize<Projects>(json);
        result.Should().NotBeNull();

        result.data.Count().Should().Be(30);
    }

    private record Foo(int ID, string Name);

    [Fact]
    public void JsonPlayground()
    {
        Foo f = JsonSerializer.Deserialize<Foo>("{}")!;
        // default values if missing
        f.ID.Should().Be(0);
        f.Name.Should().BeNull();
    }


    public string ReadResource(string name)
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

        using (Stream stream = assembly.GetManifestResourceStream(resourcePath))
        using (StreamReader reader = new StreamReader(stream))
        {
            return reader.ReadToEnd();
        }
    }
}
