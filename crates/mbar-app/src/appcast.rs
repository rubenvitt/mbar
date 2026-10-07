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
    let text = item[start..end].trim();
    Some(
        text.strip_prefix("<![CDATA[")
            .and_then(|t| t.strip_suffix("]]>"))
            .map_or(text, str::trim),
    )
}

/// `name="value"` on the item's `<enclosure>` (older Sparkle feeds put
/// `sparkle:version` and `sparkle:shortVersionString` there instead of elements).
fn enclosure_attr<'a>(item: &'a str, name: &str) -> Option<&'a str> {
    let start = item.find("<enclosure")?;
    let tag = &item[start..start + item[start..].find('>')?];
    let key = format!("{name}=");
    let at = tag
        .match_indices(&key)
        .map(|(i, _)| i)
        .find(|&i| tag[..i].ends_with(char::is_whitespace))?;
    let rest = &tag[at + key.len()..];
    let quote = rest.chars().next().filter(|c| *c == '"' || *c == '\'')?;
    let value = &rest[1..];
    Some(value[..value.find(quote)?].trim())
}

fn field<'a>(item: &'a str, name: &str) -> Option<&'a str> {
    element(item, name).or_else(|| enclosure_attr(item, name))
}

/// All items with a numeric `sparkle:version` and a `sparkle:shortVersionString`
/// (as elements or as `<enclosure>` attributes); anything else (HTML error pages, empty bodies, malformed items) yields nothing.
pub fn parse_appcast(xml: &str) -> Vec<AppcastItem> {
    xml.split("<item>")
        .skip(1)
        .filter_map(|chunk| {
            let item = &chunk[..chunk.find("</item>")?];
            Some(AppcastItem {
                build: field(item, "sparkle:version")?.parse().ok()?,
                short_version: field(item, "sparkle:shortVersionString")?.to_string(),
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

    /// Exactly what `packaging/macos/appcast.sh` prints.
    const RELEASE_FEED: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<rss version="2.0" xmlns:sparkle="http://www.andymatuschak.org/xml-namespaces/sparkle">
  <channel>
    <title>mbar</title>
    <item>
      <title>Version 0.3.0</title>
      <pubDate>Wed, 07 Oct 2026 18:22:05 +0000</pubDate>
      <sparkle:version>3000</sparkle:version>
      <sparkle:shortVersionString>0.3.0</sparkle:shortVersionString>
      <sparkle:minimumSystemVersion>13.0</sparkle:minimumSystemVersion>
      <sparkle:releaseNotesLink>https://github.com/rubenvitt/mbar/releases/tag/v0.3.0</sparkle:releaseNotesLink>
      <enclosure url="https://github.com/rubenvitt/mbar/releases/download/v0.3.0/mbar-0.3.0.zip" sparkle:edSignature="abc==" length="123" type="application/octet-stream"/>
    </item>
  </channel>
</rss>
"#;

    #[test]
    fn parses_release_feed() {
        assert_eq!(
            parse_appcast(RELEASE_FEED),
            [AppcastItem {
                build: 3000,
                short_version: "0.3.0".into(),
                minimum_system: Some("13.0".into()),
            }]
        );
    }

    #[test]
    fn parses_enclosure_attributes_and_cdata() {
        let xml = r#"<rss><channel>
<item><title><![CDATA[Version 1.2]]></title>
  <description><![CDATA[<ul><li>fix</li></ul>]]></description>
  <sparkle:minimumSystemVersion><![CDATA[ 14.0 ]]></sparkle:minimumSystemVersion>
  <enclosure url="https://x/a.zip"
    sparkle:version="1002000" sparkle:shortVersionString='1.2.0'
    length="1" type="application/octet-stream" />
</item>
<item><sparkle:version>1001000</sparkle:version><enclosure url="https://x/b.zip" sparkle:version="9"/></item>
</channel></rss>"#;
        assert_eq!(
            parse_appcast(xml),
            [AppcastItem {
                build: 1002000,
                short_version: "1.2.0".into(),
                minimum_system: Some("14.0".into()),
            }]
        );
    }

    #[test]
    fn skips_incomplete_items_keeps_valid_ones() {
        let xml = "<item><sparkle:version>abc</sparkle:version>\
<sparkle:shortVersionString>x</sparkle:shortVersionString></item>\
<item><sparkle:shortVersionString>0.1.0</sparkle:shortVersionString></item>\
<item><sparkle:version>2000</sparkle:version>\
<sparkle:shortVersionString>0.2.0</sparkle:shortVersionString></item>\
<item><sparkle:version>5000</sparkle:version>";
        let items = parse_appcast(xml);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].build, 2000);
        assert_eq!(best_item(&items, "26.0").unwrap().build, 2000);
    }

    #[test]
    fn garbage_is_empty() {
        assert!(parse_appcast("<html>404</html>").is_empty());
        assert!(parse_appcast("").is_empty());
    }
}
