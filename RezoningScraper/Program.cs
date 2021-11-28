using RezoningScraper;
using Spectre.Console;
using System.Diagnostics;
using System.Reflection;
using static Spectre.Console.AnsiConsole;

MarkupLine($"[green]Welcome to RezoningScraper v{ Assembly.GetExecutingAssembly().GetName().Version}[/]");
WriteLine();

await Status().StartAsync("Querying API...", async ctx => 
{
    var stopwatch = Stopwatch.StartNew();

    WriteLine("Starting shapeyourcity.ca query...");
    var results = await API.GetAllProjects().ToListAsync();
    MarkupLine($"API query finished: retrieved {results.Count} projects in [yellow]{stopwatch.ElapsedMilliseconds}ms[/]");

    ctx.Status = "Comparing against existing database...";

    var db = DbHelpers.CreateOrOpenFileDb("RezoningScraper.db");
    db.InitializeSchemaIfNeeded();

    List<Datum> newProjects = new();
    List<(Datum Old, Datum Latest)> modifiedProjects = new();

    stopwatch.Restart();
    var tran = db.BeginTransaction();
    foreach (var project in results)
    {
        if (db.Contains(project.id!))
        {
            var oldVersion = db.Get(project.id!);
            if (ProjectsHaveChanged(oldVersion, project))
            {
                modifiedProjects.Add((oldVersion, project));
            }
        }
        else
        {
            newProjects.Add(project);
        }

        db.Upsert(project);
    }
    tran.Commit();

    MarkupLine($"Upserted {results.Count} projects to the DB in [yellow]{stopwatch.ElapsedMilliseconds}ms[/]");
    MarkupLine($"Found [green]{newProjects.Count}[/] new projects and [green]{modifiedProjects.Count}[/] modified projects.");

    // We've got the info we need; now do something with it
    HandleNewProjects(newProjects);
    HandleModifiedProjects(modifiedProjects);
});

void HandleNewProjects(List<Datum> newProjects)
{
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

void HandleModifiedProjects(List<(Datum Old, Datum Latest)> modifiedProjects)
{
    // TODO: implement
}

//WriteLine("Press any key to exit");
//System.Console.ReadKey();

bool ProjectsHaveChanged(Datum oldVersion, Datum newVersion)
{
    // todo: check more fields (all attributes), and return data about what exactly changed

    return oldVersion?.attributes?.name != newVersion?.attributes?.name;
}