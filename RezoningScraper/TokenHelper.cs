using AngleSharp.Html.Parser;
using System.IdentityModel.Tokens.Jwt;
using System.Text.Json.Nodes;

namespace RezoningScraper;

public static class TokenHelper
{
    public static string ExtractTokenFromHtml(string html)
    {
        var parser = new HtmlParser();
        var document = parser.ParseDocument(html);

        var nextDataElement = document.QuerySelector("script#__NEXT_DATA__");
        if (nextDataElement == null) { throw new InvalidDataException("Could not find NEXT_DATA tag"); }

        var nextNode = JsonNode.Parse(nextDataElement.TextContent);
        if (nextNode == null) { throw new InvalidDataException("Could not parse NEXT_DATA JSON"); }

#pragma warning disable CS8602 // Dereference of a possibly null reference. TODO find a way to make nullable reference types play nicely with JSON queries like this
        return nextNode["props"]
            ["pageProps"]
            ["initialState"]
            ["anonymousUser"]
            ["token"].GetValue<string>();
#pragma warning restore CS8602 // Dereference of a possibly null reference.
    }

    public static DateTimeOffset GetExpirationFromEncodedJWT(string jwt)
    {
        var token = new JwtSecurityToken(jwt);
        string unparsedExp = token.Claims.Single(c => c.Type == "exp").Value;
        long exp = long.Parse(unparsedExp);
        return DateTimeOffset.FromUnixTimeSeconds(exp);
    }

}
