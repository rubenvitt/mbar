//! Sparkle appcast (RSS) reading: the daemon only needs each item's build number,
//! display version and minimum macOS version.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppcastItem {
    pub build: u64,
    pub short_version: String,
    pub minimum_system: Option<String>,
}

fn element<'a>(item: &'a str, name: &str) -> Option<&'a str> {
    let open = format!("<{name}>");
    let start = item.find(&open)? + open.len();
    let end = item[start..].find(&format!("</{name}>"))? + start;
    Some(item[start..end].trim())
}

/// All items with a numeric `sparkle:version` and a `sparkle:shortVersionString`;
/// anything else (HTML error pages, empty bodies, malformed items) yields nothing.
pub fn parse_appcast(xml: &str) -> Vec<AppcastItem> {
    xml.split("<item>")
        .skip(1)
        .filter_map(|chunk| {
            let item = &chunk[..chunk.find("</item>")?];
            Some(AppcastItem {
                build: element(item, "sparkle:version")?.parse().ok()?,
                short_version: element(item, "sparkle:shortVersionString")?.to_string(),
                minimum_system: element(item, "sparkle:minimumSystemVersion").map(str::to_string),
            })
        })
        .collect()
}

fn parts(v: &str) -> Vec<u64> {
    v.split('.')
        .map(|p| p.trim().parse().unwrap_or(0))
        .collect()
}

/// Numeric dotted-version comparison (`14.10 >= 14.9`, missing parts are 0).
pub fn os_at_least(os: &str, min: &str) -> bool {
    let (a, b) = (parts(os), parts(min));
    for i in 0..a.len().max(b.len()) {
        let (x, y) = (
            a.get(i).copied().unwrap_or(0),
            b.get(i).copied().unwrap_or(0),
        );
        if x != y {
            return x > y;
        }
    }
    true
}

/// The newest item this macOS version can run.
pub fn best_item<'a>(items: &'a [AppcastItem], os_version: &str) -> Option<&'a AppcastItem> {
    items
        .iter()
        .filter(|i| {
            i.minimum_system
                .as_deref()
                .map_or(true, |m| os_at_least(os_version, m))
        })
        .max_by_key(|i| i.build)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FEED: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<rss version="2.0" xmlns:sparkle="http://www.andymatuschak.org/xml-namespaces/sparkle">
<channel><title>mbar</title>
<item><title>Version 0.3.0</title>
  <sparkle:version>3000</sparkle:version>
  <sparkle:shortVersionString>0.3.0</sparkle:shortVersionString>
  <sparkle:minimumSystemVersion>15.0</sparkle:minimumSystemVersion>
  <enclosure url="https://x/mbar-0.3.0.zip" length="1" type="application/octet-stream" sparkle:edSignature="s"/>
</item>
<item><title>Version 0.2.0</title>
  <sparkle:version>2000</sparkle:version>
  <sparkle:shortVersionString>0.2.0</sparkle:shortVersionString>
  <sparkle:minimumSystemVersion>13.0</sparkle:minimumSystemVersion>
  <enclosure url="https://x/mbar-0.2.0.zip" length="1" type="application/octet-stream" sparkle:edSignature="s"/>
</item>
</channel></rss>"#;

    #[test]
    fn parses_items() {
        let items = parse_appcast(FEED);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].build, 3000);
        assert_eq!(items[0].short_version, "0.3.0");
        assert_eq!(items[1].minimum_system.as_deref(), Some("13.0"));
    }

    #[test]
    fn best_item_respects_minimum_system() {
        let items = parse_appcast(FEED);
        assert_eq!(best_item(&items, "14.6.1").unwrap().build, 2000);
        assert_eq!(best_item(&items, "15.0").unwrap().build, 3000);
        assert!(best_item(&items, "12.7").is_none());
    }

    #[test]
    fn os_comparison() {
        assert!(os_at_least("26.0", "13.0"));
        assert!(os_at_least("13.0", "13"));
        assert!(!os_at_least("13.6", "14.0"));
        assert!(os_at_least("14.10", "14.9"));
    }

    #[test]
    fn garbage_is_empty() {
        assert!(parse_appcast("<html>404</html>").is_empty());
        assert!(parse_appcast("").is_empty());
    }
}
