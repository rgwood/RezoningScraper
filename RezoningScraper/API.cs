using Spectre.Console;
using System.Net.Http.Headers;
using System.Text.Json;

namespace RezoningScraper;

public static class API
{
	// Required to access the API. Found in source of https://shapeyourcity.ca/embeds/projectfinder
	private const string AnonUserJWT = "eyJhbGciOiJIUzI1NiJ9.eyJpYXQiOjE2Mzc5MDczNjIsImp0aSI6IjM4NmEwNTJjODE0YTljMWE4YzE2M2ZmMmYxOGI1Mjc3IiwiZXhwIjoxNjM4MDgwMTYyLCJpc3MiOiJCYW5nIFRoZSBUYWJsZSBQdnQgTHRkIiwiZGF0YSI6eyJ1c2VyX2lkIjo0NjY0OTgzNTUsInVzZXJfdHlwZSI6IkFub255bW91c1VzZXIifX0.vPLEdWeV8Tow4S_ueShfp-oZl52-phvMomSp7YSL9tQ";

	private const int ResultsPerPage = 100;

	/// <summary>
	/// Get all projects from the ShapeYourCity API.
	/// </summary>
	/// <returns>An async enumerable of projects (because the API is paginated)</returns>
	public static async IAsyncEnumerable<Datum> GetAllProjects()
	{
		string startUrl = $"https://shapeyourcity.ca/api/v2/projects?per_page={ResultsPerPage}";
        HttpClient client = new();

		string? next = startUrl;

		int pageCount = 0;

		// loop over result pages
		while (next != null)
		{
            HttpRequestMessage message = new(HttpMethod.Get, next);
			message.Headers.Authorization = new AuthenticationHeaderValue("Bearer", AnonUserJWT);
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
