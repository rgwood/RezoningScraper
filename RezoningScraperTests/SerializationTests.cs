using System.Text.Json;

namespace RezoningScraperTests;

public class SerializationTests
{
    [Fact]
    public void CanDeserialize()
    {
        string json = SerializationTestsHelpers.ReadResource("ExampleInput.json");
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
}
