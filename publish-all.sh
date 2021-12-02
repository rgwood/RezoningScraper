#!/bin/bash

dotnet publish --configuration Release -p:PublishProfile=win-x64
pushd RezoningScraper/publish/win-x64/
zip -FS RezoningScraper-win-x64.zip RezoningScraper.exe
popd

dotnet publish --configuration Release -p:PublishProfile=linux-x64
pushd RezoningScraper/publish/linux-x64/
zip -FS RezoningScraper-linux-x64.zip RezoningScraper
popd

dotnet publish --configuration Release -p:PublishProfile=osx-x64
pushd RezoningScraper/publish/osx-x64/
zip -FS RezoningScraper-osx-x64.zip RezoningScraper
popd