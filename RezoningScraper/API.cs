using Spectre.Console;
using System.Net.Http.Headers;
using System.Text.Json;

namespace RezoningScraper;

public static class API
{
	private const int ResultsPerPage = 200; // chosen arbitrarily; higher numbers work too

	/// <summary>
	/// Get all projects from the ShapeYourCity API.
	/// </summary>
	/// <returns>An async enumerable of projects (because the API is paginated)</returns>
	public static async IAsyncEnumerable<Datum> GetAllProjects(string jwt)
	{
		string startUrl = $"https://shapeyourcity.ca/api/v2/projects?per_page={ResultsPerPage}";
        HttpClient client = new();

		string? next = startUrl;

		int pageCount = 0;

		// loop over result pages
		while (next != null)
		{
            HttpRequestMessage message = new(HttpMethod.Get, next);
			message.Headers.Authorization = new AuthenticationHeaderValue("Bearer", jwt);
            HttpResponseMessage response = await client.SendAsync(message);
			string responseContent = await response.Content.ReadAsStringAsync();
            
			var deserialized = JsonSerializer.Deserialize<Projects>(responseContent);
			var data = deserialized?.data;

			if (data != null)
			{
				AnsiConsole.WriteLine($"Retrieved page {++pageCount} ({data.Count()} items)");
				foreach (var item in data)
				{
					yield return item;
				}
			}

			next = deserialized?.links?.next;
		}
	}
}
