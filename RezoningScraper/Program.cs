using RezoningScraper;
using Spectre.Console;
using System.Diagnostics;
using System.Reflection;
using static Spectre.Console.AnsiConsole;

MarkupLine($"[green]Welcome to RezoningScraper v{ Assembly.GetExecutingAssembly().GetName().Version}[/]");
WriteLine();

await Status().StartAsync("Opening DB...", async ctx => 
{
    try
    {
        var db = DbHelper.CreateOrOpenFileDb("RezoningScraper.db");
        db.InitializeSchemaIfNeeded();

        ctx.Status = "Loading token...";
        var token = await TokenHelper.GetTokenFromDbOrWebsite(db);

        ctx.Status = "Querying API...";
        WriteLine("Starting API query...");
        var stopwatch = Stopwatch.StartNew();
        var results = await API.GetAllProjects(token.JWT).ToListAsync();
        MarkupLine($"API query finished: retrieved {results.Count} projects in [yellow]{stopwatch.ElapsedMilliseconds}ms[/]");

        ctx.Status = "Comparing against existing database...";

        List<Project> newProjects = new();
        List<(Project Old, Project Latest)> modifiedProjects = new();

        stopwatch.Restart();
        var tran = db.BeginTransaction();
        foreach (var project in results)
        {
            if (db.ContainsProject(project.id!))
            {
                var oldVersion = db.GetProject(project.id!);
                if (DidProjectChange(oldVersion, project, out var changedAttributes))
                {
                    modifiedProjects.Add((oldVersion, project));
                }
            }
            else
            {
                newProjects.Add(project);
            }

            db.UpsertProject(project);
        }
        tran.Commit();

        MarkupLine($"Upserted {results.Count} projects to the DB in [yellow]{stopwatch.ElapsedMilliseconds}ms[/]");
        MarkupLine($"Found [green]{newProjects.Count}[/] new projects and [green]{modifiedProjects.Count}[/] modified projects.");

        // We've got the info we need; now do something with it
        HandleNewProjects(newProjects);
        HandleModifiedProjects(modifiedProjects);
    }
    catch (Exception ex)
    {
        MarkupLine("[red]Fatal exception thrown[/]");
        WriteException(ex);
    }
});

void HandleNewProjects(List<Project> newProjects)
{
    if (newProjects.Count == 0) return;

    WriteLine();
    MarkupLine("[bold underline green]New Projects[/]");
    WriteLine();

    foreach (var project in newProjects)
    {
        MarkupLine($"[bold underline]{project.attributes!.name!.EscapeMarkup()}[/]");
        MarkupLine($"State: {project.attributes.state.EscapeMarkup()}");

        var tags = project?.attributes?.projecttaglist ?? new string[0];
        if (tags.Any())
        {
            MarkupLine($"Tags: {string.Join(',', tags).EscapeMarkup()}");
        }

        WriteLine($"URL: {project!.links!.self}");
        WriteLine();
    }
}

void HandleModifiedProjects(List<(Project Old, Project Latest)> modifiedProjects)
{
    // TODO: implement
}

bool DidProjectChange(Project oldVersion, Project newVersion, out Dictionary<string, AttributeChange> changedAttributes)
{
    bool changed = false;
    // todo: check more fields (all attributes), and return data about what exactly changed
    changedAttributes = new Dictionary<string, AttributeChange>();

    var oldAttrs = oldVersion.attributes!;
    var newAttrs = newVersion.attributes!;

    if (oldAttrs.name != newAttrs.name)
    {
        changed = true;
        changedAttributes.Add("Name", new(oldAttrs.name, newAttrs.name));
    }

    return oldVersion?.attributes?.name != newVersion?.attributes?.name;
}

public record AttributeChange(string OldValue, string NewValue);