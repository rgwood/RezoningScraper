set shell := ["nu", "-c"]

publish:
    cross build --target x86_64-unknown-linux-musl --release
    rsync --progress target/x86_64-unknown-linux-musl/release/rezoning-scraper spudnik:bin/rezoning-scraper