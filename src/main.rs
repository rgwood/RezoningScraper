use anyhow::Result;
use scraper::{Html, Selector};

fn main() -> Result<()> {
    println!("Hello, world!");

    Ok(())
}


// tests
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_jwt() {
        let html = include_str!("../test_files/projectFinder.html");
    }
}