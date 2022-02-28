using Cocona;
using Sentry;
using Spectre.Console;
using System.Diagnostics;
using System.Reflection;
using System.Text;
using System.Text.Json;
using static Spectre.Console.AnsiConsole;

namespace RezoningScraper;

public class Program
{
    static void Main()
    {
        using var sentry = SentrySdk.Init(o =>
        {
            o.Dsn = "https://de37ba1d6fde4781b3bb1f400c0d01d7@o1100469.ingest.sentry.io/6125607";
            o.TracesSampleRate = 1.0; // Capture 100% of transactions
        });

        DotEnv.Load();

        var app = CoconaLiteApp.Create();

        app.AddCommand(RunScraperWithExceptionHandling).WithDescription("A tool to detect new+modified postings on Vancouver's shapeyourcity.ca website. Data is stored in a local SQLite database next to the executable.");
        app.AddCommand("tweet", TweetWithExceptionHandling).WithDescription("Tweet a recent item.");

        app.Run();
    }

    static async Task RunScraperWithExceptionHandling(
        [Option(Description = "A Slack Incoming Webhook URL. If specified, RezoningScraper will post info about new+modified rezonings to this address.")]
        string? slackWebhookUrl,
        [Option(Description = "Use cached json queries, as long as they are fresh enough.")]
        bool useCache,
        [Option(Description = "Whether to save the API results to database.")]
        bool saveToDb = true)
    {
        try
        {
            await RunScraper(slackWebhookUrl, useCache, saveToDb);
        }
        catch (Exception ex)
        {
            SentrySdk.CaptureException(ex);
            throw;
        }
    }

    static async Task RunScraper(string? slackWebhookUrl, bool useCache, bool saveToDb)
    {
        MarkupLine($"[green]Welcome to RezoningScraper v{Assembly.GetExecutingAssembly().GetName().Version}[/]");
        if (string.IsNullOrWhiteSpace(slackWebhookUrl)) { WriteLine($"Slack URI not specified; will not publish updates to Slack."); }
        if (!saveToDb) { WriteLine("Dry run; will not write results to database"); }

        WriteLine();

        // Use Spectre.Console's Status UI https://spectreconsole.net/live/status
        await AnsiConsole.Status().StartAsync("Opening DB...", async ctx =>
        {
            var db = DbHelper.CreateOrOpenFileDb("RezoningScraper.db");
            db.InitializeSchemaIfNeeded();

            ctx.Status = "Loading token...";
            var token = await TokenHelper.GetTokenFromDbOrWebsite(db, useCache);

            ctx.Status = "Querying API...";
            WriteLine("Starting API query...");
            var stopwatch = Stopwatch.StartNew();
            var latestProjects = await API.GetAllProjects(token.JWT, useCache).ToListAsync();
            MarkupLine($"API query finished: retrieved {latestProjects.Count} projects in [yellow]{stopwatch.ElapsedMilliseconds}ms[/]");

            ctx.Status = "Comparing against projects in local database...";
            stopwatch.Restart();
            List<Project> newProjects = new();
            List<ChangedProject> changedProjects = new();
            var tran = db.BeginTransaction();
            foreach (var project in latestProjects)
            {
                if (db.ContainsProject(project))
                {
                    var oldVersion = db.GetProject(project.id!);
                    var comparer = new ProjectComparer(oldVersion, project);

                    if (comparer.DidProjectChange(out var changes))
                    {
                        changedProjects.Add(new(oldVersion, project, changes));
                    }
                }
                else
                {
                    newProjects.Add(project);
                }

                if (saveToDb) { db.UpsertProject(project); }
            }
            tran.Commit();

            MarkupLine($"Upserted {latestProjects.Count} projects to the DB in [yellow]{stopwatch.ElapsedMilliseconds}ms[/]");
            MarkupLine($"Found [green]{newProjects.Count}[/] new projects and [green]{changedProjects.Count}[/] modified projects.");

            if (!string.IsNullOrEmpty(slackWebhookUrl) && (newProjects.Any() || changedProjects.Any()))
            {
                await PostToSlack(slackWebhookUrl, newProjects, changedProjects);
            }

            PrintNewProjects(newProjects);
            PrintChangedProjects(changedProjects);
        });
    }


    static async Task TweetWithExceptionHandling()
    {
        try
        {
            MarkupLine("[green]Time to tweet![/]");
            await Task.Delay(100);
            throw new NotImplementedException();
        }
        catch (Exception ex)
        {
            SentrySdk.CaptureException(ex);
            throw;
        }
    }

    private static async Task PostToSlack(string slackWebhookUri, List<Project> newProjects, List<ChangedProject> changedProjects)
    {
        WriteLine($"Posting to Slack...");

        var message = new StringBuilder();

        foreach (var proj in newProjects)
        {
            message.AppendLine($"New item: *<{proj.links!.self!}|{RemoveLineBreaks(proj.attributes!.name!)}>*");

            var tags = proj.attributes?.projecttaglist ?? new string[0];
            if (tags.Any())
            {
                message.AppendLine($"• Tags: {string.Join(", ", tags)}");
            }

            message.AppendLine($"• State: {Capitalize(proj.attributes?.state)}");
            message.AppendLine();
        }

        var json = JsonSerializer.Serialize(new { text = message.ToString() });
        var client = new HttpClient();
        var content = new StringContent(json, Encoding.UTF8, "application/json");

        await client.PostAsync(slackWebhookUri, content);
        WriteLine("Posted new+changed projects to Slack");
    }

    static void PrintNewProjects(List<Project> newProjects)
    {
        if (newProjects.Count == 0) return;

        WriteLine();
        MarkupLine("[bold underline green]New Projects[/]");
        WriteLine();

        foreach (var project in newProjects)
        {
            MarkupLine($"[bold underline]{project.attributes!.name!.EscapeMarkup()}[/]");
            WriteLine($"State: {project.attributes.state}");

            var tags = project?.attributes?.projecttaglist ?? new string[0];
            if (tags.Any())
            {
                WriteLine($"Tags: {string.Join(',', tags)}");
            }

            WriteLine($"URL: {project!.links!.self}");
            WriteLine();
        }
    }

    static void PrintChangedProjects(List<ChangedProject> changedProjects)
    {
        if (changedProjects.Count == 0) return;

        WriteLine();
        MarkupLine("[bold underline green]Changed Projects[/]");
        WriteLine();

        foreach (var changedProject in changedProjects)
        {
            MarkupLine($"[bold underline]{changedProject.LatestVersion.attributes!.name!.EscapeMarkup()}[/]");

            foreach (var change in changedProject.Changes)
            {
                WriteLine($"{change.Key}: '{change.Value.OldValue}' -> '{change.Value.NewValue}'");
            }

            WriteLine();
        }
    }

    private static string? Capitalize(string? str)
    {
        if (string.IsNullOrWhiteSpace(str)) { return str; }

        return str.Substring(0, 1).ToUpper() + str.Substring(1);
    }

    private static string RemoveLineBreaks(string str)
        => str.Replace("\r\n", "").Replace("\r", "").Replace("\n", "");

    record ChangedProject(Project OldVersion, Project LatestVersion, Dictionary<string, AttributeChange> Changes);
}
