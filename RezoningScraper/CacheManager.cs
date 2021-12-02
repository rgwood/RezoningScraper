using Polly.Caching;
using System.Text.Json;
using System.Security.Cryptography;
using System.Text;
using static Spectre.Console.AnsiConsole;
using Dapper;

namespace RezoningScraper
{
    public class CacheManager<TResult> : IAsyncCacheProvider<TResult>
    {
        public async Task PutAsync(string key, TResult value, Ttl ttl, CancellationToken ct, bool continueOnCapturedContext)
        {
            using var dbConnection = DbHelper.CreateOrOpenFileDb("RezoningScraper.db");
            using var sha = SHA256.Create();
            var hashbytes = sha.ComputeHash(Encoding.UTF8.GetBytes(key));
            var hashstring = new StringBuilder();
            foreach (var b in hashbytes)
            {
                hashstring.Append(b.ToString("X2"));
            }

            using var ms = new MemoryStream();
            await JsonSerializer.SerializeAsync(ms, value, cancellationToken: ct);

            
            using var tran = dbConnection.BeginTransaction();
            dbConnection.Execute(
                "REPLACE INTO Cache(Key, Expiration, Value) VALUES(@Key, @Expiration,@Value)",
                new {
                    Key = hashstring.ToString(),
                    Expiration = (DateTimeOffset.UtcNow + ttl.Timespan).ToUnixTimeMilliseconds(),
                    Value = Encoding.UTF8.GetString(ms.ToArray())
                });

            tran.Commit();
            WriteLine($"Completed put cache for {key}");
        }

        public async Task<(bool, TResult)> TryGetAsync(string key, CancellationToken ct, bool continueOnCapturedContext)
        {
            using var dbConnection = DbHelper.CreateOrOpenFileDb("RezoningScraper.db");
            using var sha = SHA256.Create();
            var hashbytes = sha.ComputeHash(Encoding.UTF8.GetBytes(key));
            var hashstring = new StringBuilder();
            foreach (var b in hashbytes)
            {
                hashstring.Append(b.ToString("X2"));
            }

            var cachedValue = await dbConnection.ExecuteScalarAsync<string>("SELECT Value FROM Cache WHERE Key = @Key AND Expiration > @Expiration",
                new
                {
                    Key=hashstring.ToString(),
                    Expiration=DateTimeOffset.UtcNow.ToUnixTimeMilliseconds()
                });
            try
            {
                if (string.IsNullOrEmpty(cachedValue))
                {
                    WriteLine($"No value in cache for {key}");
                    return (false, default(TResult));
                }
                var model = JsonSerializer.Deserialize<TResult>(cachedValue);
                    WriteLine($"Found value in cache for {key}");
                    return (true, model);
            }
            catch
            {
                WriteLine($"Error on get cache for {key}");
                return (false, default(TResult));
            }
        }
    }
}
