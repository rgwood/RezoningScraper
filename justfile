set shell := ["nu", "-c"]

watch:
    dotnet watch --project RezoningScraper/RezoningScraper.csproj

watch-tests:
    watch . { dotnet test } --glob=**/*.cs

build-release-all:
    just build-release linux-x64
    just build-release win-x64
    just build-release linux-arm64

build-release arch:
    dotnet publish RezoningScraper/RezoningScraper.csproj \
    --runtime {{arch}} \
    --output publish/{{arch}}/ \
    --configuration Release --self-contained true \
    -p:PublishTrimmed=true \
    -p:PublishSingleFile=true -p:DebugType=embedded -p:IncludeNativeLibrariesForSelfExtract=true;
    just publish {{arch}}

publish arch:
    scp publish/{{arch}}/* potato-pi:/mnt/QNAP1/rpm/dropbox/

# Razor tooling fails on this stupid bug: https://github.com/dotnet/razor-tooling/issues/6241
workaround-razor-bug:
    bash -c "export CLR_OPENSSL_VERSION_OVERRIDE=1.1; code ."
