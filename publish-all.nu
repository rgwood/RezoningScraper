enter RezoningScraper

build-zip "win-x64" "RezoningScraper.exe"
build-zip "linux-x64" "RezoningScraper"
build-zip "osx-x64" "RezoningScraper"

def build-zip [rid executableName] {
  dotnet publish --configuration Release $"-p:PublishProfile=($rid)"
  enter $"publish/($rid)/"
  7z a $"..\\all\RezoningScraper-($rid).zip" $executableName
  exit
}