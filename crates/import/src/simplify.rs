//! Converts Apple dictionary entry XHTML into small, semantic HTML.
//!
//! Apple dictionaries style entries almost entirely through CSS on nested
//! `<span class="…">` elements. The app renders HTML natively and does not
//! apply stylesheets, so at import time we map well-known classes to plain
//! tags (block, heading, bold, italic), keep real structural HTML, rewrite
//! `x-dictionary:` links and drop everything else down to its text.

use quick_xml::escape::resolve_html5_entity;
use quick_xml::events::attributes::Attribute;
use quick_xml::events::{BytesStart, Event};
use quick_xml::{Reader, XmlVersion};

/// Link prefix for "look up this word" links.
pub const FIND_LINK: &str = "dict:find/";
/// Link prefix for "open the entry with this id" links.
pub const ENTRY_LINK: &str = "dict:entry/";
/// Prefix for image sources that live in the dictionary's resources directory.
pub const RESOURCE_LINK: &str = "dict:res/";

/// A `<d:index>` element: a search key for the entry it appears in.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IndexKey {
    pub value: String,
    pub title: String,
    pub anchor: Option<String>,
    pub yomi: Option<String>,
    pub parental: bool,
}

/// An entry after simplification.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParsedEntry {
    pub id: String,
    pub title: String,
    pub html: String,
    pub indexes: Vec<IndexKey>,
}

#[derive(Debug, thiserror::Error)]
#[error("malformed entry XHTML at byte {position}: {source}")]
pub struct SimplifyError {
    position: u64,
    source: quick_xml::Error,
}

/// How an element is rendered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action {
    /// Wrap the content in fixed markup.
    Wrap(&'static str, &'static str),
    /// The entry headword: an `<h1>` the first time, bold afterwards.
    Headword,
    Link,
    Image,
    LineBreak,
    /// Keep the content, drop the element.
    Unwrap,
    /// Drop the element and its content.
    Skip,
}

const BLOCK: Action = Action::Wrap("<div>", "</div>");
const BOLD: Action = Action::Wrap("<b>", "</b>");
const ITALIC: Action = Action::Wrap("<i>", "</i>");

/// Apple class names and how to render them. The first class of an element
/// that appears here wins.
const CLASS_RULES: &[(&str, Action)] = &[
    // Headword and header group.
    ("hw", Action::Headword),
    ("hg", BLOCK),
    ("x_xh0", BLOCK),
    ("x_xh1", BLOCK),
    // Sense groups, senses and other display blocks.
    ("sg", BLOCK),
    ("se1", BLOCK),
    ("se2", BLOCK),
    ("se3", BLOCK),
    ("msDict", BLOCK),
    ("subEntryBlock", BLOCK),
    ("subEntry", BLOCK),
    ("etym", BLOCK),
    ("note", BLOCK),
    ("x_blk", BLOCK),
    ("x_xd0", BLOCK),
    ("x_xd1", BLOCK),
    ("x_xd2", BLOCK),
    ("x_xd3", BLOCK),
    ("x_xo0", BLOCK),
    ("x_xo1", BLOCK),
    ("x_xo2", BLOCK),
    ("x_xo3", BLOCK),
    // Inline emphasis.
    ("sn", BOLD),
    ("l", BOLD),
    ("sw", BOLD),
    ("ex", ITALIC),
    ("pos", ITALIC),
    ("posg", ITALIC),
    ("lg", ITALIC),
    ("gg", ITALIC),
    ("reg", ITALIC),
    ("lbl", ITALIC),
    ("ge", ITALIC),
    ("sj", ITALIC),
    ("inf", ITALIC),
];

/// Classes whose elements Apple separates with CSS margins rather than
/// spaces, e.g. consecutive examples in the Swedish dictionary.
const SPACED_CLASSES: &[&str] = &["ex"];

fn class_action(classes: &str) -> Option<Action> {
    classes.split_ascii_whitespace().find_map(|class| {
        CLASS_RULES
            .iter()
            .find(|(name, _)| *name == class)
            .map(|&(_, action)| action)
    })
}

fn tag_action(name: &str, classes: Option<&str>) -> Action {
    let from_class = || classes.and_then(class_action);
    match name {
        "d:entry" => Action::Unwrap,
        "d:index" => Action::Skip,
        "div" | "section" | "article" | "header" | "footer" | "dl" | "dt" | "dd" | "figure"
        | "figcaption" | "details" | "summary" | "pre" | "address" => match from_class() {
            Some(Action::Headword) => Action::Headword,
            _ => BLOCK,
        },
        "p" => Action::Wrap("<p>", "</p>"),
        "h1" => Action::Wrap("<h1>", "</h1>"),
        "h2" => Action::Wrap("<h2>", "</h2>"),
        "h3" => Action::Wrap("<h3>", "</h3>"),
        "h4" => Action::Wrap("<h4>", "</h4>"),
        "h5" => Action::Wrap("<h5>", "</h5>"),
        "h6" => Action::Wrap("<h6>", "</h6>"),
        "ul" => Action::Wrap("<ul>", "</ul>"),
        "ol" => Action::Wrap("<ol>", "</ol>"),
        "li" => Action::Wrap("<li>", "</li>"),
        "table" => Action::Wrap("<table>", "</table>"),
        "thead" => Action::Wrap("<thead>", "</thead>"),
        "tbody" => Action::Wrap("<tbody>", "</tbody>"),
        "tr" => Action::Wrap("<tr>", "</tr>"),
        "td" => Action::Wrap("<td>", "</td>"),
        "th" => Action::Wrap("<th>", "</th>"),
        "blockquote" => Action::Wrap("<blockquote>", "</blockquote>"),
        "b" | "strong" => BOLD,
        "i" | "em" | "cite" | "var" | "dfn" => ITALIC,
        "u" | "ins" => Action::Wrap("<u>", "</u>"),
        "s" | "del" | "strike" => Action::Wrap("<s>", "</s>"),
        "code" | "kbd" | "samp" | "tt" => Action::Wrap("<code>", "</code>"),
        "mark" => Action::Wrap("<mark>", "</mark>"),
        // Furigana and similar readings: show them in parentheses.
        "rt" => Action::Wrap("(", ")"),
        "rp" => Action::Skip,
        "a" => Action::Link,
        "img" => Action::Image,
        "br" => Action::LineBreak,
        "script" | "style" | "head" | "title" | "link" | "meta" | "object" | "iframe" | "embed"
        | "audio" | "video" | "source" | "button" | "input" | "select" | "textarea" | "svg"
        | "math" | "hr" => Action::Skip,
        _ => from_class().unwrap_or(Action::Unwrap),
    }
}

/// Parses one `<d:entry>` element and simplifies its body.
pub fn parse_entry(xhtml: &str) -> Result<ParsedEntry, SimplifyError> {
    let mut reader = Reader::from_str(xhtml);
    reader.config_mut().check_end_names = false;
    reader.config_mut().allow_unmatched_ends = true;

    let mut entry = ParsedEntry::default();
    let mut out = Output::default();
    // Closing markup for each open element; `None` while inside a skipped element.
    let mut stack: Vec<Option<(&'static str, bool)>> = Vec::new();
    let mut skip_depth = 0usize;
    let mut has_heading = false;
    let mut seen_root = false;

    loop {
        let event = reader.read_event().map_err(|source| SimplifyError {
            position: reader.error_position(),
            source,
        })?;
        match event {
            Event::Start(ref e) | Event::Empty(ref e) => {
                let is_empty = matches!(event, Event::Empty(_));
                let name = e.name();
                let name = name.as_ref();
                if !seen_root {
                    seen_root = true;
                    if name == "d:entry" {
                        entry.id = attr(e, "id").unwrap_or_default();
                        entry.title = attr(e, "d:title").unwrap_or_default();
                    }
                }
                if name == "d:index" {
                    entry.indexes.push(index_key(e));
                }
                if skip_depth > 0 {
                    if !is_empty {
                        skip_depth += 1;
                        stack.push(None);
                    }
                    continue;
                }
                let classes = attr(e, "class");
                let mut action = tag_action(name, classes.as_deref());
                if action == Action::Headword {
                    action = if has_heading {
                        BOLD
                    } else {
                        Action::Wrap("<h1>", "</h1>")
                    };
                }
                if let Action::Wrap("<h1>", _) = action {
                    has_heading = true;
                }
                let close = match action {
                    Action::Skip => {
                        if !is_empty {
                            skip_depth = 1;
                            stack.push(None);
                        }
                        continue;
                    }
                    Action::LineBreak => {
                        out.void("<br>");
                        ""
                    }
                    Action::Image => {
                        out.image(e);
                        ""
                    }
                    Action::Link => match attr(e, "href").and_then(|h| rewrite_href(&h)) {
                        Some(href) => {
                            out.open(&format!("<a href=\"{}\">", escape_attr(&href)));
                            "</a>"
                        }
                        None => "",
                    },
                    Action::Wrap(open, close) => {
                        out.open(open);
                        close
                    }
                    Action::Unwrap | Action::Headword => "",
                };
                let spaced = classes.as_deref().is_some_and(|c| {
                    c.split_ascii_whitespace()
                        .any(|c| SPACED_CLASSES.contains(&c))
                });
                if is_empty {
                    out.close(close);
                } else {
                    stack.push(Some((close, spaced)));
                }
            }
            Event::End(_) => {
                if skip_depth > 0 {
                    skip_depth -= 1;
                    stack.pop();
                } else if let Some(Some((close, spaced))) = stack.pop() {
                    out.close(close);
                    if spaced {
                        out.soft_space();
                    }
                }
            }
            Event::Text(t) if skip_depth == 0 => out.text(&t.xml10_content()),
            Event::CData(t) if skip_depth == 0 => out.text(&t.xml10_content()),
            Event::GeneralRef(r) if skip_depth == 0 => {
                if let Ok(Some(c)) = r.resolve_char_ref() {
                    out.text(c.encode_utf8(&mut [0; 4]));
                } else if let Some(s) = resolve_html5_entity(&r.xml10_content()) {
                    out.text(s);
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    // Close anything left open by malformed input.
    while let Some(open) = stack.pop() {
        if let Some((close, _)) = open {
            out.close(close);
        }
    }

    let mut html = out.finish();
    if !has_heading && !entry.title.is_empty() {
        let mut heading = String::from("<h1>");
        escape_into(&mut heading, &entry.title, false);
        heading.push_str("</h1>");
        html.insert_str(0, &heading);
    }
    entry.html = html;
    Ok(entry)
}

fn index_key(e: &BytesStart<'_>) -> IndexKey {
    let value = attr(e, "d:value").unwrap_or_default();
    IndexKey {
        title: attr(e, "d:title").unwrap_or_else(|| value.clone()),
        value,
        anchor: attr(e, "d:anchor").map(|a| anchor_id(&a)),
        yomi: attr(e, "d:yomi"),
        parental: attr(e, "d:parental-control").is_some_and(|v| v.trim() == "1"),
    }
}

/// Extracts `x` from `xpointer(//*[@id='x'])`; other values are returned as is.
pub fn anchor_id(anchor: &str) -> String {
    anchor
        .split_once("@id=")
        .and_then(|(_, rest)| {
            let quote = rest.chars().next().filter(|c| *c == '\'' || *c == '"')?;
            let rest = &rest[1..];
            rest.find(quote).map(|end| rest[..end].to_owned())
        })
        .unwrap_or_else(|| anchor.to_owned())
}

/// Maps a source link to one the app understands, or `None` to drop the link.
fn rewrite_href(href: &str) -> Option<String> {
    let href = href.trim();
    if let Some(rest) = href.strip_prefix("x-dictionary:") {
        let (kind, target) = rest.split_once(':')?;
        let target = strip_bundle_suffix(target);
        if target.is_empty() {
            return None;
        }
        return match kind {
            "d" => Some(format!("{FIND_LINK}{target}")),
            "r" => Some(format!("{ENTRY_LINK}{target}")),
            _ => None,
        };
    }
    if href.starts_with("http://") || href.starts_with("https://") || href.starts_with("mailto:") {
        return Some(href.to_owned());
    }
    None
}

/// `x-dictionary` targets may end in `:<bundle identifier>`, e.g.
/// `make:com.apple.dictionary.NOAD`.
fn strip_bundle_suffix(target: &str) -> &str {
    match target.rsplit_once(':') {
        Some((head, bundle)) if bundle.contains('.') && !bundle.contains(char::is_whitespace) => {
            head
        }
        _ => target,
    }
}

fn attr(e: &BytesStart<'_>, name: &str) -> Option<String> {
    e.attributes()
        .with_checks(false)
        .flatten()
        .find(|a| a.key.as_ref() == name)
        .map(|a| attr_value(&a))
}

fn attr_value(a: &Attribute<'_>) -> String {
    a.normalized_value_with(XmlVersion::Implicit1_0, 8, resolve_html5_entity)
        .map(|v| v.into_owned())
        .unwrap_or_else(|_| a.value.clone().into_owned())
}

/// Builds the output HTML, collapsing whitespace as it goes.
///
/// The app's HTML renderer drops whitespace at the start or end of an
/// element's content, so spaces are never written there: a space is held
/// back until the next text and then written outside any tags opened since.
#[derive(Default)]
struct Output {
    html: String,
    pending_space: bool,
    /// A space that is dropped if the next text starts with punctuation.
    soft_space: bool,
    /// Where the run of tags opened since the last text began.
    open_run_start: Option<usize>,
}

impl Output {
    fn open(&mut self, s: &str) {
        self.flush_space();
        self.open_run_start.get_or_insert(self.html.len());
        self.html.push_str(s);
    }

    fn close(&mut self, s: &str) {
        self.open_run_start = None;
        self.html.push_str(s);
    }

    /// A void element such as `<br>`, which needs no space around it.
    fn void(&mut self, s: &str) {
        self.pending_space = false;
        self.soft_space = false;
        self.open_run_start = None;
        self.html.push_str(s);
    }

    fn soft_space(&mut self) {
        self.soft_space = true;
    }

    fn text(&mut self, s: &str) {
        for (i, part) in s.split(|c: char| c.is_ascii_whitespace()).enumerate() {
            if i > 0 {
                self.pending_space = true;
            }
            if !part.is_empty() {
                if self.soft_space
                    && part.starts_with([',', '.', ';', ':', '!', '?', ')', ']', '}'])
                {
                    self.soft_space = false;
                }
                self.flush_space();
                self.open_run_start = None;
                escape_into(&mut self.html, part, false);
            }
        }
    }

    fn escaped(&mut self, s: &str, in_attr: bool) {
        escape_into(&mut self.html, s, in_attr);
    }

    fn image(&mut self, e: &BytesStart<'_>) {
        let Some(src) = attr(e, "src") else { return };
        let src = if src.contains("://") || src.starts_with("data:") {
            src
        } else {
            format!("{RESOURCE_LINK}{}", src.trim_start_matches("./"))
        };
        self.flush_space();
        self.open_run_start = None;
        self.html.push_str("<img src=\"");
        self.escaped(&src, true);
        self.html.push('"');
        for name in ["alt", "width", "height"] {
            if let Some(value) = attr(e, name) {
                self.html.push(' ');
                self.html.push_str(name);
                self.html.push_str("=\"");
                self.escaped(&value, true);
                self.html.push('"');
            }
        }
        self.html.push('>');
    }

    fn flush_space(&mut self) {
        let soft = std::mem::take(&mut self.soft_space);
        if !std::mem::take(&mut self.pending_space) && !soft {
            return;
        }
        let at = self.open_run_start.unwrap_or(self.html.len());
        if at > 0 && !self.html[..at].ends_with(' ') {
            self.html.insert(at, ' ');
            if let Some(start) = &mut self.open_run_start {
                *start += 1;
            }
        }
    }

    fn finish(self) -> String {
        let trimmed = self.html.trim();
        if trimmed.len() == self.html.len() {
            self.html
        } else {
            trimmed.to_owned()
        }
    }
}

fn escape_attr(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    escape_into(&mut out, s, true);
    out
}

fn escape_into(out: &mut String, s: &str, in_attr: bool) {
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' if in_attr => out.push_str("&quot;"),
            c => out.push(c),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn html(xhtml: &str) -> String {
        parse_entry(xhtml).unwrap().html
    }

    #[test]
    fn extracts_root_attributes_and_index_keys() {
        let e = parse_entry(
            r#"<d:entry xmlns:d="http://www.apple.com/DTDs/DictionaryService-1.0.rng" id="make_1" d:title="make">
                <d:index d:value="make" d:title="make"/>
                <d:index d:value="made" d:title="made (make)"/>
                <d:index d:value="make it" d:parental-control="1" d:anchor="xpointer(//*[@id='make_it'])"/>
                <h1>make</h1>
            </d:entry>"#,
        )
        .unwrap();
        assert_eq!(e.id, "make_1");
        assert_eq!(e.title, "make");
        assert_eq!(e.html, "<h1>make</h1>");
        assert_eq!(e.indexes.len(), 3);
        assert_eq!(e.indexes[1].title, "made (make)");
        assert_eq!(e.indexes[2].title, "make it");
        assert_eq!(e.indexes[2].anchor.as_deref(), Some("make_it"));
        assert!(e.indexes[2].parental);
    }

    #[test]
    fn maps_apple_classes() {
        let out = html(
            r#"<d:entry id="m_1" d:title="make">
              <span class="hg x_xh0"><span class="hw">make </span><span class="prx"> | māk | </span></span>
              <span class="sg"><span class="se1 x_xd0"><span class="posg x_xdh"><span class="pos">verb</span></span>
                <span class="msDict x_xd1"><span class="sn">1</span> <span class="df">form by putting parts together</span>:
                  <span class="eg"><span class="ex">she makes her own clothes</span></span></span></span></span>
              <span class="subEntry"><span class="hw">maker</span></span>
            </d:entry>"#,
        );
        assert_eq!(
            out,
            "<div><h1>make</h1> | māk |</div> <div><div><i><i>verb</i></i> \
             <div><b>1</b> form by putting parts together: <i>she makes her own clothes</i></div></div></div> \
             <div><b>maker</b></div>"
        );
    }

    #[test]
    fn adds_heading_from_title_when_missing() {
        assert_eq!(
            html(r#"<d:entry id="a" d:title="R&amp;D"><p>research</p></d:entry>"#),
            "<h1>R&amp;D</h1><p>research</p>"
        );
    }

    #[test]
    fn rewrites_links() {
        let out = html(
            r#"<d:entry id="a" d:title="a"><h1>a</h1>
              <a href="x-dictionary:d:make">make</a>
              <a href="x-dictionary:r:m_en_us123:com.apple.dictionary.NOAD">see</a>
              <a href="https://example.com/?a=1&amp;b=&quot;2&quot;">web</a>
              <a href="Images/foo.html">local</a>
            </d:entry>"#,
        );
        assert_eq!(
            out,
            "<h1>a</h1> <a href=\"dict:find/make\">make</a> \
             <a href=\"dict:entry/m_en_us123\">see</a> \
             <a href=\"https://example.com/?a=1&amp;b=&quot;2&quot;\">web</a> local"
        );
    }

    #[test]
    fn skips_unrenderable_content_and_resolves_entities() {
        let out = html(
            "<d:entry id=\"a\" d:title=\"a\"><h1>a&nbsp;b &#x263A; &lt;x&gt;</h1>\
             <style>.x{}</style><span class=\"x\"><script>alert(1)</script>kept</span>\
             <br/><img src=\"Images/pic.png\" alt=\"a pic\"/><ruby>漢<rp>(</rp><rt>かん</rt><rp>)</rp></ruby></d:entry>",
        );
        assert_eq!(
            out,
            "<h1>a\u{a0}b ☺ &lt;x&gt;</h1>kept<br><img src=\"dict:res/Images/pic.png\" alt=\"a pic\">漢(かん)"
        );
    }

    #[test]
    fn keeps_spaces_that_css_would_otherwise_provide() {
        // Trailing spaces inside labels move outside the closing tags.
        assert_eq!(
            html(
                r#"<d:entry id="a" d:title="a"><h1>a</h1><span class="lg"><span class="reg">informal </span></span><span class="syn">chirpy</span></d:entry>"#
            ),
            "<h1>a</h1><i><i>informal</i></i> chirpy"
        );
        // Leading spaces move in front of the opening tags.
        assert_eq!(
            html(
                r#"<d:entry id="a" d:title="a"><h1>a</h1>see:<span class="ex"> an example</span></d:entry>"#
            ),
            "<h1>a</h1>see: <i>an example</i>"
        );
        // Consecutive examples are separated, but not from punctuation.
        assert_eq!(
            html(
                r#"<d:entry id="a" d:title="a"><h1>a</h1><span class="ex">husgrund<span class="gp">;</span></span><span class="ex">hustak</span><span class="gp">: </span>x</d:entry>"#
            ),
            "<h1>a</h1><i>husgrund;</i> <i>hustak</i>: x"
        );
    }

    #[test]
    fn anchor_ids() {
        assert_eq!(anchor_id("xpointer(//*[@id='make_it'])"), "make_it");
        assert_eq!(anchor_id(r#"xpointer(//*[@id="x.1"])"#), "x.1");
        assert_eq!(anchor_id("plain"), "plain");
    }
}
