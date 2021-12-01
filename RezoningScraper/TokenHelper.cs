using AngleSharp.Html.Parser;
using Microsoft.Data.Sqlite;
using Polly;
using System.IdentityModel.Tokens.Jwt;
using System.Net;
using System.Text.Json.Nodes;
using static Spectre.Console.AnsiConsole;

namespace RezoningScraper;

internal static class TokenHelper
{
    internal static async Task<Token> GetTokenFromDbOrWebsite(SqliteConnection db, bool useCache)
    {
        Token? tokenFromDb = db.GetToken();
        if (tokenFromDb != null && tokenFromDb.Expiration > DateTimeOffset.UtcNow.AddMinutes(1))
        {
            WriteLine($"Loaded API token from database. Cached token will expire on {tokenFromDb.Expiration}");
            return tokenFromDb;
        }
        else
        {
            // TODO: add retries, this page seems unreliable
            IAsyncPolicy<Token> cachePolicy = useCache
                ? Policy.CacheAsync<Token>(new CacheManager<Token>(), TimeSpan.FromMinutes(1))
                : Policy.NoOpAsync<Token>();
            WriteLine("Getting latest anonymous user token from shapeyourcity.ca");
            var client = new HttpClient() { Timeout = TimeSpan.FromSeconds(20) };
            return await cachePolicy.ExecuteAsync(async context =>
            {
                var htmlToParse = await Policy.Handle<Exception>()
                    .RetryAsync(3)
                    .ExecuteAsync(async () => await client.GetStringAsync("https://shapeyourcity.ca/embeds/projectfinder"));
                string jwt = ExtractTokenFromHtml(htmlToParse);

                DateTimeOffset expiration = GetExpirationFromEncodedJWT(jwt);

                WriteLine($"Retrieved JWT with expiration date {expiration}");

                var newToken = new Token(expiration, jwt);

                db.SetToken(newToken);
                WriteLine($"Cached JWT in local database");

                return newToken;
            }, new Context("https://shapeyourcity.ca/embeds/projectfinder"));
        }
    }

    internal static string ExtractTokenFromHtml(string html)
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

    internal static DateTimeOffset GetExpirationFromEncodedJWT(string jwt)
    {
        var token = new JwtSecurityToken(jwt);
        string unparsedExp = token.Claims.Single(c => c.Type == "exp").Value;
        long exp = long.Parse(unparsedExp);
        return DateTimeOffset.FromUnixTimeSeconds(exp);
    }
}
