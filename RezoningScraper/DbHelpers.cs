using Dapper;
using Microsoft.Data.Sqlite;
using System.Reflection;
using System.Text.Json;

namespace RezoningScraper;

public static class DbHelpers
{
    /// <summary>Creates or opens a SQLite DB</summary>
    /// <param name="filePath">A file path relative to the executable</param>
    /// <returns></returns>
    public static SqliteConnection CreateOrOpenFileDb(string filePath)
    {
        string exePath = Assembly.GetEntryAssembly()!.Location;
        string exeDirPath = Directory.GetParent(exePath)!.FullName;
        string dbAbsolutePath = Path.Combine(exeDirPath, filePath);

        SqliteConnection connection = new SqliteConnection($"Data Source={dbAbsolutePath}");
        connection.Open();
        return connection;
    }

    public static SqliteConnection CreateInMemoryDb()
    {
        SqliteConnection connection = new("DataSource=:memory:");
        connection.Open();
        return connection;
    }


    public static void InitializeSchemaIfNeeded(this SqliteConnection conn)
    {
        string sql = @"
CREATE TABLE IF NOT EXISTS
Projects(
    Id TEXT PRIMARY KEY NOT NULL,
    Serialized TEXT NOT NULL
)";
        conn.Execute(sql);
    }

    public static bool Contains(this SqliteConnection conn, string id) 
        => conn.ExecuteScalar<int>("SELECT COUNT(*) FROM Projects WHERE id = @id", new { id }) > 0;

    public static Datum Get(this SqliteConnection conn, string id)
    {
        if (!conn.Contains(id))
        {
            throw new KeyNotFoundException();
        }

        string json = conn.QuerySingle<string>("SELECT Serialized FROM Projects where id = @id", new { id });

        var result = JsonSerializer.Deserialize<Datum>(json);

        if (result == null)
        {
            throw new InvalidDataException($"Could not deserialize item with ID {id}");
        }

        return result;
    }

    public static IEnumerable<Datum> GetAll(this SqliteConnection conn)
    {
        throw new NotImplementedException();
    }

    public static void Upsert(this SqliteConnection conn, Datum datum)
    {
        // TODO: add archive functionality, move old versions to an archive table or something

        string json = JsonSerializer.Serialize(datum);
        const string Sql = @"
INSERT INTO projects(Id, Serialized) VALUES(@id,@json)
  ON CONFLICT(Id) DO UPDATE SET Serialized = excluded.Serialized";
        conn.Execute(Sql, new { datum.id, json });
    }
}
