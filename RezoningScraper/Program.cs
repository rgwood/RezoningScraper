using Spectre.Console;
using System.CommandLine;
using System.CommandLine.Invocation;
using System.Diagnostics;
using System.Reflection;
using System.Text;
using System.Text.Json;
using static Spectre.Console.AnsiConsole;

namespace RezoningScraper;

public class Program
{
    static async Task<int> Main(string[] args)
    {
        // Set up the CLI with System.CommandLine: https://github.com/dotnet/command-line-api/tree/main/docs
        var rootCommand = new RootCommand("A tool to detect new+modified postings on Vancouver's shapeyourcity.ca website. Data is stored in a local SQLite database next to the executable.")
        {
            new Option<string?>("--slack-webhook-url",
                getDefaultValue: () => "",
                description: "A Slack Incoming Webhook URL. If specified, RezoningScraper will post info about new+modified rezonings to this address."),
            new Option<bool>("--use-cache", description: "Use cached json queries, as long as they are fresh enough."),
            new Option<bool>("--save-to-db",
                getDefaultValue: () => true,
                description: "Whether to save the API results to database.")
        };

        rootCommand.Handler = CommandHandler.Create<string, bool, bool>(RunScraper);
        return await rootCommand.InvokeAsync(args);
    }

    static async Task RunScraper(string slackWebhookUrl, bool useCache, bool saveToDb)
    {
        MarkupLine($"[green]Welcome to RezoningScraper v{Assembly.GetExecutingAssembly().GetName().Version}[/]");
        if(string.IsNullOrWhiteSpace(slackWebhookUrl)) { WriteLine($"Slack URI not specified; will not publish updates to Slack."); }
        if (!saveToDb) { WriteLine("Dry run; will not write results to database"); }
        
        WriteLine();

        // Use Spectre.Console's Status UI https://spectreconsole.net/live/status
        await AnsiConsole.Status().StartAsync("Opening DB...", async ctx =>
        {
            try
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
            }
            catch (Exception ex)
            {
                MarkupLine("[red]Fatal exception thrown[/]");
                WriteException(ex);
            }
        });
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

        // Disabled posting changed projects for now; too much noise from minor description tweaks. 
        //foreach (var changedProject in changedProjects)
        //{
        //    // don't report on projects being archived, too much noise
        //    if (changedProject.Changes.Any(c => c.Key == "state" && c.Value.NewValue == "archived"))
        //    {
        //        continue;
        //    }

        //    message.AppendLine($"Changed item: *<{changedProject.LatestVersion.links!.self!}|{RemoveLineBreaks(changedProject.LatestVersion.attributes!.name!)}>*");

        //    foreach (var change in changedProject.Changes)
        //    {
        //        const int MaxLength = 100; // arbitrary; we just need some way to avoid huuuuuuuge descriptions that look bad in Slack
        //        if (change.Value.OldValue?.Length > MaxLength || change.Value.NewValue?.Length > MaxLength)
        //        {
        //            message.AppendLine($"• {Capitalize(change.Key)} changed, too big to show");
        //        }
        //        else
        //        {
        //            message.AppendLine($"• {Capitalize(change.Key)}: {change.Value.OldValue} ➡️ {change.Value.NewValue}");
        //        }

        //        message.AppendLine();
        //    }
        //}

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
