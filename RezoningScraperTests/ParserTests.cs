namespace RezoningScraperTests;

public class ParserTests
{
    /* { "data": {
           "user_id": 467419949,
           "user_type": "AnonymousUser"
         },
         "exp": 1638294321,
         "iat": 1638121521,
         "iss": "Bang The Table Pvt Ltd",
         "jti": "49b984aa47751d338fc3baf7897e9685"
    }
     */


    private const string JWT = "eyJhbGciOiJIUzI1NiJ9.eyJpYXQiOjE2MzgxMjE1MjEsImp0aSI6IjQ5Yjk4NGFhNDc3NTFkMzM4ZmMzYmFmNzg5N2U5Njg1IiwiZXhwIjoxNjM4Mjk0MzIxLCJpc3MiOiJCYW5nIFRoZSBUYWJsZSBQdnQgTHRkIiwiZGF0YSI6eyJ1c2VyX2lkIjo0Njc0MTk5NDksInVzZXJfdHlwZSI6IkFub255bW91c1VzZXIifX0.p7FGWkT_7sWtC4gAsQ_HgaX-Z8aw88QQiGdHITmQleQ";
    private const long ExpirationInUnixSeconds = 1638294321;

    [Fact]
    public void ExtractJWT()
    {
        string html = SerializationTestsHelpers.ReadResource("projectFinder.html");
        var jwt = TokenHelper.ExtractTokenFromHtml(html);
        jwt.Should().Be(JWT);
    }

    [Fact]
    public void ExtractExpirationFromJWT()
    {
        var expiration = TokenHelper.GetExpirationFromEncodedJWT(JWT);
        expiration.Should().Be(DateTimeOffset.FromUnixTimeSeconds(ExpirationInUnixSeconds));
    }

}
