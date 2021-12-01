using Spectre.Console;
using System.Net.Http.Headers;
using System.Text.Json;
using Polly;

namespace RezoningScraper;

public static class API
{
	private const int ResultsPerPage = 200; // chosen arbitrarily; higher numbers work too

	/// <summary>
	/// Get all projects from the ShapeYourCity API.
	/// </summary>
	/// <returns>An async enumerable of projects (because the API is paginated)</returns>
	public static async IAsyncEnumerable<Project> GetAllProjects(string jwt, bool useCache = false)
	{

			IAsyncPolicy<Projects> cachePolicy = useCache
				? Policy.CacheAsync<Projects>(new CacheManager<Projects>(), TimeSpan.FromHours(1))
				: Policy.NoOpAsync<Projects>();
			var client = new HttpClient();
		string startUrl = $"https://shapeyourcity.ca/api/v2/projects?per_page={ResultsPerPage}";

		string? next = startUrl;

		int pageCount = 0;

		// loop over result pages
		while (next != null)
		{
			HttpRequestMessage message = new(HttpMethod.Get, next);
			message.Headers.Authorization = new AuthenticationHeaderValue("Bearer", jwt);
			var deserializedResponse = await cachePolicy.ExecuteAsync(async context =>
			{
				var response = await Policy.Handle<Exception>().RetryAsync(3).ExecuteAsync(async () => await client.SendAsync(message));
				return JsonSerializer.Deserialize<Projects>(await response.Content.ReadAsStringAsync()) ?? new Projects();
			}, new Context(next));
			if (deserializedResponse?.data is not null)
			{
				AnsiConsole.WriteLine($"Retrieved page {++pageCount} ({deserializedResponse.data.Count()} items)");
				foreach (var item in deserializedResponse?.data ?? Enumerable.Empty<Project>())
				{
					yield return item;
				}
				next = deserializedResponse?.links?.next;
			}
		}
	}
}
