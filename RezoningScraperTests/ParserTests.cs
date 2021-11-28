namespace RezoningScraperTests;

public class ParserTests
{
    [Fact]
    public void ExtractJWT()
    {
        string html = SerializationTestsHelpers.ReadResource("projectFinder.html");


        var jwt = TokenHelper.ExtractTokenFromHtml(html);

        jwt.Should().Be("eyJhbGciOiJIUzI1NiJ9.eyJpYXQiOjE2MzgxMjE1MjEsImp0aSI6IjQ5Yjk4NGFhNDc3NTFkMzM4ZmMzYmFmNzg5N2U5Njg1IiwiZXhwIjoxNjM4Mjk0MzIxLCJpc3MiOiJCYW5nIFRoZSBUYWJsZSBQdnQgTHRkIiwiZGF0YSI6eyJ1c2VyX2lkIjo0Njc0MTk5NDksInVzZXJfdHlwZSI6IkFub255bW91c1VzZXIifX0.p7FGWkT_7sWtC4gAsQ_HgaX-Z8aw88QQiGdHITmQleQ");
    }
}
