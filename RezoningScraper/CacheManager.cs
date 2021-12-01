using Polly.Caching;
using System.IO.IsolatedStorage;
using System.Text.Json;
using System.Security.Cryptography;
using System.Text;
using static Spectre.Console.AnsiConsole;

namespace RezoningScraper
{
    public class CacheManager<TResult> : IAsyncCacheProvider<TResult>
    {
        record CachedRecord<T>(DateTime Ttl, T Record);
        public async Task PutAsync(string key, TResult value, Ttl ttl, CancellationToken ct, bool continueOnCapturedContext)
        {
            using IsolatedStorageFile isoStore = IsolatedStorageFile.GetStore(IsolatedStorageScope.User | IsolatedStorageScope.Domain | IsolatedStorageScope.Assembly, null, null);
            using var sha = SHA256.Create();
            var hashbytes = sha.ComputeHash(Encoding.UTF8.GetBytes(key));
            var hashstring = new StringBuilder();
            foreach (var b in hashbytes)
            {
                hashstring.Append(b.ToString("X2"));
            }
            using var openFile = isoStore.OpenFile(hashstring.ToString(), FileMode.Create, FileAccess.Write);
            await JsonSerializer.SerializeAsync(openFile, new CachedRecord<TResult>(DateTime.UtcNow + ttl.Timespan, value), cancellationToken: ct);
            WriteLine($"Completed put cache for {key}");
        }

        public Task<(bool, TResult)> TryGetAsync(string key, CancellationToken ct, bool continueOnCapturedContext)
        {
            using var sha = SHA256.Create();
            var hashbytes = sha.ComputeHash(Encoding.UTF8.GetBytes(key));
            var hashstring = new StringBuilder();
            foreach (var b in hashbytes)
            {
                hashstring.Append(b.ToString("X2"));
            }
            using IsolatedStorageFile isoStore = IsolatedStorageFile.GetStore(IsolatedStorageScope.User | IsolatedStorageScope.Domain | IsolatedStorageScope.Assembly, null, null);
            try
            {
                if (!isoStore.FileExists(hashstring.ToString()))
                {
                    WriteLine($"No value in cache for {key}");
                    return Task.FromResult((false, default(TResult)));
                }
                using var openFile = isoStore.OpenFile(hashstring.ToString(), FileMode.Open, FileAccess.Read);
                using var sr = new StreamReader(openFile);
                var model = JsonSerializer.Deserialize<CachedRecord<TResult>>(sr.ReadToEnd());
                if (model?.Ttl < DateTime.UtcNow || model is null || model.Record is null)
                {
                    WriteLine($"Expired/missing value in cache for {key}");
                    return Task.FromResult((false, default(TResult)));
                }
                else
                {

                    WriteLine($"Found value in cache for {key}");
                    return Task.FromResult((true, model.Record));
                }
            }
            catch
            {
                WriteLine($"Error on get cache for {key}");
                return Task.FromResult((false, default(TResult)));
            }
        }
    }
}
