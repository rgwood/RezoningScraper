namespace RezoningScraperTests;

public class DatabaseTests
{
    [Fact]
    public void CanInitializeDb()
    {
        var db = DbHelper.CreateInMemoryDb();
        db.InitializeSchemaIfNeeded();
    }

    [Fact]
    public void ContainsWorks()
    {
        var db = DbHelper.CreateInMemoryDb();
        db.InitializeSchemaIfNeeded();
        
        db.ContainsProject("foo").Should().BeFalse();
        db.UpsertProject(new Datum() { id = "foo" });
        db.ContainsProject("foo").Should().BeTrue();
    }

    [Fact]
    public void UpsertWorks()
    {
        var db = DbHelper.CreateInMemoryDb();
        db.InitializeSchemaIfNeeded();

        db.UpsertProject(new Datum() { id = "foo", type="first" });
        db.GetProject("foo").type.Should().Be("first");

        db.UpsertProject(new Datum() { id = "foo", type = "second" });
        db.GetProject("foo").type.Should().Be("second");
    }


    [Fact]
    public void TokenWorks()
    {
        var db = DbHelper.CreateInMemoryDb();
        db.InitializeSchemaIfNeeded();
        db.GetToken().Should().BeNull();

        var token = new Token(DateTimeOffset.UtcNow, "foo");
        db.SetToken(token);
        var tokenFromDb = db.GetToken();
        
        token.Expiration.Should().BeCloseTo(tokenFromDb.Expiration, TimeSpan.FromSeconds(1)); // b/c we lose some timestamp fidelity when saving to DB
        token.JWT.Should().Be(tokenFromDb.JWT);
    }
}
