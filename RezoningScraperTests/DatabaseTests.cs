using FluentAssertions;
using RezoningScraper;
using Xunit;

namespace RezoningScraperTests;

public class DatabaseTests
{
    [Fact]
    public void CanInitializeDb()
    {
        var db = DbHelpers.CreateInMemoryDb();
        db.InitializeSchemaIfNeeded();
    }

    [Fact]
    public void ContainsWorks()
    {
        var db = DbHelpers.CreateInMemoryDb();
        db.InitializeSchemaIfNeeded();
        
        db.Contains("foo").Should().BeFalse();
        db.Upsert(new Datum() { id = "foo" });
        db.Contains("foo").Should().BeTrue();
    }

    [Fact]
    public void UpsertWorks()
    {
        var db = DbHelpers.CreateInMemoryDb();
        db.InitializeSchemaIfNeeded();

        db.Upsert(new Datum() { id = "foo", type="first" });
        db.Get("foo").type.Should().Be("first");

        db.Upsert(new Datum() { id = "foo", type = "second" });
        db.Get("foo").type.Should().Be("second");
    }
}
