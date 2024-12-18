use std::collections::HashMap;

use html2md::{TagHandler, TagHandlerFactory};
use scraper::{Html, Selector};

struct IgnoreHandlerFactory;
struct IgnoreHandler;

impl TagHandler for IgnoreHandler {
    fn handle(&mut self, _tag: &html2md::Handle, _printer: &mut html2md::StructuredPrinter) {}

    fn skip_descendants(&self) -> bool {
        true
    }

    fn after_handle(&mut self, _printer: &mut html2md::StructuredPrinter) {}
}

impl TagHandlerFactory for IgnoreHandlerFactory {
    fn instantiate(&self) -> Box<dyn TagHandler> {
        Box::new(IgnoreHandler)
    }
}

struct TextOnlyHandlerFactory;
struct TextOnlyHandler;

impl TagHandler for TextOnlyHandler {
    fn handle(&mut self, _tag: &html2md::Handle, _printer: &mut html2md::StructuredPrinter) {}

    fn after_handle(&mut self, _printer: &mut html2md::StructuredPrinter) {}
}

impl TagHandlerFactory for TextOnlyHandlerFactory {
    fn instantiate(&self) -> Box<dyn TagHandler> {
        Box::new(TextOnlyHandler)
    }
}

/// Convert HTML to Markdown, ignoring images and not including URLs
pub fn html_to_markdown(html: &str) -> String {
    let mut handlers = HashMap::<String, Box<dyn TagHandlerFactory>>::new();
    handlers.insert("img".to_string(), Box::new(IgnoreHandlerFactory));
    handlers.insert("a".to_string(), Box::new(TextOnlyHandlerFactory));

    html2md::parse_html_custom(&html, &handlers)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_html_to_markdown() {
        let description = r#"<p><img src="https://s3.ca-central-1.amazonaws.com/ehq-production-canada/17e7374a3b5c63231790827340fd28f639047b85/original/1675372309/aa7203d07fd579ed76f41da4a05ebf32_Capture.PNG?1675372309" style="width: 482px;" class="fr-fic fr-dib">Matthew Cheng Architect Inc. has applied to the City of Vancouver for permission to develop the following on this site:</p><ul><li>A new multiple dwelling building, containing six strata-titled dwelling units</li><li>A floor space ratio of 1.20 (approximately 6,650.24 sq. ft.)</li><li>A proposed height of approximately 33.3 ft.</li><li>Four parking spaces at the rear having access from the lane</li></ul><p>Under the site&rsquo;s existing <a href="https://bylaws.vancouver.ca/zoning/zoning-by-law-district-schedule-rm-8-all-districts.pdf">RM-8A zoning</a>, the application is &ldquo;conditional&rdquo; so it may be permitted. However, it requires the decision of the Director of Planning.</p>"#;

        let mut handlers = HashMap::<String, Box<dyn TagHandlerFactory>>::new();
        handlers.insert("img".to_string(), Box::new(IgnoreHandlerFactory));
        handlers.insert("a".to_string(), Box::new(TextOnlyHandlerFactory));

        let md = html2md::parse_html_custom(&description, &handlers);

        let expected = "Matthew Cheng Architect Inc. has applied to the City of Vancouver for permission to develop the following on this site:

* A new multiple dwelling building, containing six strata-titled dwelling units
* A floor space ratio of 1.20 (approximately 6,650.24 sq. ft.)
* A proposed height of approximately 33.3 ft.
* Four parking spaces at the rear having access from the lane

Under the site’s existing RM-8A zoning, the application is “conditional” so it may be permitted. However, it requires the decision of the Director of Planning.";

        assert_eq!(md, expected);
    }
}
