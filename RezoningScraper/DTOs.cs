using System.Text.Json.Serialization;

namespace RezoningScraper;

// C# classes that map directly to the JSON returned by the ShapeYourCity API
// Created using the Visual Studio "Paste JSON as Classes" feature

public class Projects
{
    public Datum[]? data { get; set; }
    public Links? links { get; set; }
    public Meta? meta { get; set; }
}

public class Links
{
    public string? self { get; set; }
    public string? first { get; set; }
    public object? prev { get; set; }
    public string? next { get; set; }
    public string? last { get; set; }
}

public class Meta
{
    public int all { get; set; }
    public int published { get; set; }
    public int draft { get; set; }
    public int archived { get; set; }
    public int hidden { get; set; }
}

public class Datum
{
    public string? id { get; set; }
    public string? type { get; set; }
    public Attributes? attributes { get; set; }
    public Relationships? relationships { get; set; }
    public Links1? links { get; set; }
}

public class Attributes
{
    public string? name { get; set; }
    public string? permalink { get; set; }
    public string? state { get; set; }
    [JsonPropertyName("visibility-mode")]
    public string? visibilitymode { get; set; }
    [JsonPropertyName("published-at")]
    public DateTime publishedat { get; set; }
    [JsonPropertyName("survey-count")]
    public int surveycount { get; set; }
    [JsonPropertyName("banner-url")]
    public string? bannerurl { get; set; }
    public string? description { get; set; }
    [JsonPropertyName("project-tag-list")]
    public string[]? projecttaglist { get; set; }
    [JsonPropertyName("created-at")]
    public DateTime createdat { get; set; }
    [JsonPropertyName("archival-reason-message")]
    public string? archivalreasonmessage { get; set; }
    [JsonPropertyName("image-url")]
    public string? imageurl { get; set; }
    [JsonPropertyName("image-caption")]
    public string? imagecaption { get; set; }
    [JsonPropertyName("image-description")]
    public string? imagedescription { get; set; }
    [JsonPropertyName("meta-description")]
    public string? metadescription { get; set; }
    [JsonPropertyName("parent-id")]
    public int? parentid { get; set; }
    public bool access { get; set; }
}

public class Relationships
{
    public Site? site { get; set; }
}

public class Site
{
    public Data? data { get; set; }
}

public class Data
{
    public string? id { get; set; }
    public string? type { get; set; }
}

public class Links1
{
    public string? self { get; set; }
}