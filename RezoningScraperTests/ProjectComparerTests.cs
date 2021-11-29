namespace RezoningScraperTests;

public class ProjectComparerTests
{
    [Fact]
    public void ComparerSeesNoChange()
    {
        var p1 = new Project() { attributes = new Attributes() { name = "foo" } };

        var comparer = new ProjectComparer(p1, p1);

        comparer.DidProjectChange(out var changes).Should().BeFalse();
        changes.Should().BeEmpty();
    }

    [Fact]
    public void CanDetectNameChange()
    {
        var p1 = new Project() { attributes = new Attributes() { name = "foo" } };
        var p2 = new Project() { attributes = new Attributes() { name = "bar" } };

        var comparer = new ProjectComparer(p1, p2);

        comparer.DidProjectChange(out var changes).Should().BeTrue();
        changes.Single().Key.Should().Be("name");
    }

    [Fact]
    public void CanDetectMultipleChanges()
    {
        var p1 = new Project() { attributes = new Attributes() { name = "foo", description = "derp", state = "published" } };
        var p2 = new Project() { attributes = new Attributes() { name = "bar", description = "derp", state = "archived"} };

        var comparer = new ProjectComparer(p1, p2);

        comparer.DidProjectChange(out var changes).Should().BeTrue();
        changes.Count().Should().Be(2);
        changes.Should().ContainKey("name");
        changes.Should().ContainKey("state");
    }

}
