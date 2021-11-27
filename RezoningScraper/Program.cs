using RezoningScraper;
using System.Diagnostics;
using System.Reflection;
using static Spectre.Console.AnsiConsole;

MarkupLine($"[green]Welcome to RezoningScraper v{ Assembly.GetExecutingAssembly().GetName().Version}[/]");

await Status().StartAsync("Querying API...", async ctx => 
{
    var queryStopwatch = Stopwatch.StartNew();

    var results = await API.GetAllProjects().ToListAsync();

    MarkupLine($"API query finished: retrieved {results.Count} projects in [yellow]{queryStopwatch.ElapsedMilliseconds}ms[/]");
});


WriteLine("Press any key to exit");
System.Console.ReadKey();
