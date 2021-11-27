using RezoningScraper2;
using System.Text.Json;
using 


Console.WriteLine("Hello, World!");

string url = @"https://shapeyourcity.ca/api/v2/projects";
string jwt = "eyJhbGciOiJIUzI1NiJ9.eyJpYXQiOjE2Mzc5MDczNjIsImp0aSI6IjM4NmEwNTJjODE0YTljMWE4YzE2M2ZmMmYxOGI1Mjc3IiwiZXhwIjoxNjM4MDgwMTYyLCJpc3MiOiJCYW5nIFRoZSBUYWJsZSBQdnQgTHRkIiwiZGF0YSI6eyJ1c2VyX2lkIjo0NjY0OTgzNTUsInVzZXJfdHlwZSI6IkFub255bW91c1VzZXIifX0.vPLEdWeV8Tow4S_ueShfp-oZl52-phvMomSp7YSL9tQ";

var message = new HttpRequestMessage(HttpMethod.Get, url);
message.Headers.Authorization = new System.Net.Http.Headers.AuthenticationHeaderValue("Bearer", jwt);

var client = new HttpClient();

var response = await client.SendAsync(message);
Console.WriteLine(response);

string content = await response.Content.ReadAsStringAsync();

var deserialized = JsonSerializer.Deserialize<Projects>(content);


