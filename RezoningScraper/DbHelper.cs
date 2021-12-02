﻿using Dapper;
using Microsoft.Data.Sqlite;
using System.Text.Json;

namespace RezoningScraper;

public record Token(DateTimeOffset Expiration, string JWT);

/// <summary>
/// A bunch of extension methods for working with the database.
/// This is getting a bit messy; maybe refactor into a repository or similar if this project gets bigger.
/// </summary>
public static class DbHelper
{
    /// <summary>Creates or opens a SQLite DB</summary>
    /// <param name="filePath">A file path relative to the executable</param>
    /// <returns></returns>
    public static SqliteConnection CreateOrOpenFileDb(string filePath)
    {
        string dbAbsolutePath = Path.Combine(AppContext.BaseDirectory, filePath);

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
);

CREATE TABLE IF NOT EXISTS
TokenCache(
    Expiration INTEGER NOT NULL,
    Token TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS
Cache(
  Key TEXT PRIMARY KEY,
  Expiration INTEGER NOT NULL,
  Value TEXT NOT NULL
);
";
        conn.Execute(sql);
    }

    public static bool ContainsProject(this SqliteConnection conn, string id) 
        => conn.ExecuteScalar<int>("SELECT COUNT(*) FROM Projects WHERE id = @id", new { id }) > 0;

    public static bool ContainsProject(this SqliteConnection conn, Project p)
        => conn.ContainsProject(p.id!);

    public static Project GetProject(this SqliteConnection conn, string id)
    {
        if (!conn.ContainsProject(id))
        {
            throw new KeyNotFoundException();
        }

        string json = conn.QuerySingle<string>("SELECT Serialized FROM Projects where id = @id", new { id });

        var result = JsonSerializer.Deserialize<Project>(json);

        if (result == null)
        {
            throw new InvalidDataException($"Could not deserialize item with ID {id}");
        }

        return result;
    }

    public static void UpsertProject(this SqliteConnection conn, Project datum)
    {
        // TODO: add archive functionality, move old versions to an archive table or something

        string json = JsonSerializer.Serialize(datum);
        const string Sql = @"
INSERT INTO projects(Id, Serialized) VALUES(@id,@json)
  ON CONFLICT(Id) DO UPDATE SET Serialized = excluded.Serialized";
        conn.Execute(Sql, new { datum.id, json });
    }

    public static Token? GetToken(this SqliteConnection conn)
    {
        var tuple = conn.Query<(long, string)?>("select * from TokenCache").FirstOrDefault();

        if (tuple == null) return null;

        return new Token(DateTimeOffset.FromUnixTimeMilliseconds(tuple.Value.Item1), tuple.Value.Item2);
    }

    public static void SetToken(this SqliteConnection conn, Token t)
    {
        var tran = conn.BeginTransaction();
        conn.Execute("DELETE FROM TokenCache");

        conn.Execute("INSERT INTO TokenCache(Expiration, Token) VALUES(@Expiration,@Token)",
            new { Expiration = t.Expiration.ToUnixTimeMilliseconds(), Token=t.JWT });

        tran.Commit();
    }
}
