//! Local EPUB paths and ordered conversion of host-normalized XHTML.

use ito_rs::html::Node;
use ito_rs::models::{Page, PageContent};
use ito_rs::{Error, Result};

fn invalid_path() -> Error {
    Error::Host("Invalid EPUB resource path".into())
}

fn invalid_document() -> Error {
    Error::Host("Unable to read EPUB document".into())
}

fn validate_book(book_id: &str) -> Result<()> {
    crate::api::validate_book_key(book_id).map_err(|_| invalid_path())
}

fn safe_segment(segment: &str) -> bool {
    !segment.is_empty()
        && !matches!(segment, "." | "..")
        && !segment
            .chars()
            .any(|c| c.is_control() || matches!(c, '/' | '\\' | '%' | ':'))
}

fn validate_internal(path: &str) -> Result<()> {
    if !path.split('/').all(safe_segment) {
        return Err(invalid_path());
    }
    Ok(())
}

fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

// Decode once, rejecting encoded separators and residual escapes. This also prevents
// a server or URL implementation performing a second decode from changing the path.
fn decode_segment(segment: &str) -> Result<String> {
    let bytes = segment.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut offset = 0;
    while offset < bytes.len() {
        let byte = if bytes[offset] == b'%' {
            let high = bytes
                .get(offset + 1)
                .and_then(|b| hex(*b))
                .ok_or_else(invalid_path)?;
            let low = bytes
                .get(offset + 2)
                .and_then(|b| hex(*b))
                .ok_or_else(invalid_path)?;
            offset += 3;
            let value = high * 16 + low;
            if matches!(value, b'/' | b'\\' | b'%') {
                return Err(invalid_path());
            }
            value
        } else {
            let value = bytes[offset];
            offset += 1;
            value
        };
        decoded.push(byte);
    }
    let decoded = String::from_utf8(decoded).map_err(|_| invalid_path())?;
    if decoded
        .chars()
        .any(|c| c.is_control() || matches!(c, '\\' | ':'))
    {
        return Err(invalid_path());
    }
    Ok(decoded)
}

fn without_suffix(reference: &str) -> Result<&str> {
    // Reject controls/backslashes even in a discarded suffix; never let URL parsers
    // disagree about whether such characters introduce a different authority.
    if reference.chars().any(|c| c.is_control() || c == '\\') {
        return Err(invalid_path());
    }
    Ok(reference.split(['?', '#']).next().unwrap_or_default())
}

/// Recover only a book-local path, never a server or credentials from a manifest URL.
pub fn spine_path(href: &str, book_id: &str) -> Result<String> {
    validate_book(book_id)?;
    let reference = without_suffix(href)?;
    if reference.starts_with("//") {
        return Err(invalid_path());
    }
    let path = if let Some((scheme, rest)) = reference.split_once("://") {
        if !matches!(scheme, "http" | "https") {
            return Err(invalid_path());
        }
        let (authority, path) = rest.split_once('/').ok_or_else(invalid_path)?;
        if authority.is_empty()
            || authority.contains('@')
            || authority
                .chars()
                .any(|c| c.is_whitespace() || matches!(c, '%' | '?' | '#'))
        {
            return Err(invalid_path());
        }
        // Deliberately discard the authority. Outgoing requests use configured origin.
        path
    } else {
        reference.strip_prefix('/').ok_or_else(invalid_path)?
    };
    let prefix = format!("api/v1/books/{book_id}/resource/");
    // A reverse proxy may prefix the manifest's endpoint. Only the book-local
    // suffix is reused; requests still start at the user's configured base.
    let internal = path
        .strip_prefix(&prefix)
        .or_else(|| {
            path.split_once(&format!("/{prefix}"))
                .map(|(_, internal)| internal)
        })
        .ok_or_else(invalid_path)?;
    let segments = internal
        .split('/')
        .map(decode_segment)
        .collect::<Result<Vec<_>>>()?;
    let decoded = segments.join("/");
    validate_internal(&decoded)?;
    Ok(decoded)
}

/// Encode every path segment independently; no caller-supplied URL can change origin.
pub fn resource_api_path(book_id: &str, resource_path: &str) -> Result<String> {
    validate_book(book_id)?;
    validate_internal(resource_path)?;
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut path = format!("/api/v1/books/{book_id}/resource/");
    for byte in resource_path.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~' | b'/') {
            path.push(char::from(byte));
        } else {
            path.push('%');
            path.push(char::from(HEX[(byte >> 4) as usize]));
            path.push(char::from(HEX[(byte & 15) as usize]));
        }
    }
    Ok(path)
}

fn image_path(reference: &str, spine: &str) -> Result<String> {
    validate_internal(spine)?;
    let reference = without_suffix(reference)?;
    if reference.is_empty() || reference.starts_with('/') {
        return Err(invalid_path());
    }
    let mut segments: Vec<&str> = spine.split('/').collect();
    segments.pop();
    let decoded = reference
        .split('/')
        .map(decode_segment)
        .collect::<Result<Vec<_>>>()?;
    for (raw, segment) in reference.split('/').zip(&decoded) {
        // Literal parent references are normal EPUB links. Encoded dot references
        // are ambiguous across URL decoders and are never accepted.
        match segment.as_str() {
            "." if raw == "." => {}
            ".." if raw == ".." => {
                segments.pop().ok_or_else(invalid_path)?;
            }
            _ if safe_segment(segment) => segments.push(segment),
            _ => return Err(invalid_path()),
        }
    }
    let path = segments.join("/");
    validate_internal(&path)?;
    Ok(path)
}

fn push_page(pages: &mut Vec<Page>, content: PageContent) -> Result<()> {
    let index = i32::try_from(pages.len()).map_err(|_| invalid_document())?;
    pages.push(Page {
        index,
        content,
        has_description: false,
        description: None,
        headers: None,
    });
    Ok(())
}

fn flush_text(text: &mut String, pages: &mut Vec<Page>, preformatted: bool) -> Result<()> {
    if text.is_empty() {
        return Ok(());
    }
    // Text comes only from host serialization and stays escaped; retained inline
    // tags preserve host text semantics. Decode each complete run once rather
    // than inventing an incomplete HTML entity table.
    let fragment = preformatted.then(|| format!("<pre>{text}</pre>"));
    let node = Node::new(fragment.as_deref().unwrap_or(text.as_str()).as_bytes());
    let decoded = node.text().map_err(|_| invalid_document())?;
    text.clear();
    if !decoded.trim().is_empty() {
        push_page(pages, PageContent::Text(decoded))?;
    }
    Ok(())
}

fn block(tag: &str) -> bool {
    matches!(
        tag,
        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "p" | "pre" | "li" | "blockquote" | "div"
    )
}

// Find a serialized tag's end without mistaking a quoted '>' for markup.
fn tag_end(markup: &str) -> Result<usize> {
    let mut quote = None;
    for (index, byte) in markup.bytes().enumerate().skip(1) {
        match (quote, byte) {
            (Some(active), current) if active == current => quote = None,
            (None, b'\'' | b'"') => quote = Some(byte),
            (None, b'>') => return Ok(index + 1),
            _ => {}
        }
    }
    Err(invalid_document())
}

// Script/style contents are raw text and can contain '<' or '>' independently
// of markup. Find their real closing tag before resuming ordinary tokenization.
fn raw_element_end(markup: &str, tag: &str) -> Result<usize> {
    let prefix = format!("</{tag}");
    let bytes = markup.as_bytes();
    for offset in 0..bytes.len() {
        let Some(candidate) = bytes.get(offset..offset + prefix.len()) else {
            break;
        };
        if candidate.eq_ignore_ascii_case(prefix.as_bytes())
            && bytes
                .get(offset + prefix.len())
                .is_some_and(|b| b.is_ascii_whitespace() || *b == b'>')
        {
            return Ok(offset + tag_end(&markup[offset..])?);
        }
    }
    Err(invalid_document())
}

fn image_reference(tag: &str, svg: bool) -> Result<Option<String>> {
    // Query attributes on a host-parsed standalone element. This handles quoted,
    // unquoted and entity-encoded attributes using precisely the host's rules.
    // Keep SVG foreign-content context: standalone HTML <image> may become <img>.
    let wrapped = svg.then(|| format!("<svg>{tag}</svg>"));
    let node = Node::new(wrapped.as_deref().unwrap_or(tag).as_bytes());
    let selector = if svg { "image" } else { "img" };
    let elements = node.select(selector).map_err(|_| invalid_document())?;
    let element = elements.first().ok_or_else(invalid_document)?;
    let primary = element
        .attr(if svg { "href" } else { "src" })
        .map_err(|_| invalid_document())?;
    if svg && primary.as_deref().is_none_or(str::is_empty) {
        return element.attr("xlink:href").map_err(|_| invalid_document());
    }
    Ok(primary)
}

/// Traverse the host-normalized body serialization rather than selector/own_text
/// pairs: those lose inline descendants and move container text past its children.
/// Every text node is consumed once, in order, including text directly in the body.
pub fn parse_document(xhtml: &[u8], book_id: &str, spine_path: &str) -> Result<Vec<Page>> {
    validate_book(book_id)?;
    validate_internal(spine_path)?;
    let document = Node::new(xhtml);
    let bodies = document.select("body").map_err(|_| invalid_document())?;
    let body = bodies.first().ok_or_else(invalid_document)?;
    let markup = body.outer_html().map_err(|_| invalid_document())?;
    if markup.is_empty() {
        return Err(invalid_document());
    }
    let mut remaining = markup.as_str();
    let mut pages = Vec::new();
    let mut text = String::new();
    let mut skipped: Vec<String> = Vec::new();
    let mut pre_depth = 0usize;
    while !remaining.is_empty() {
        if skipped
            .last()
            .is_some_and(|tag| matches!(tag.as_str(), "script" | "style"))
        {
            let active = skipped.pop().ok_or_else(invalid_document)?;
            let end = raw_element_end(remaining, &active)?;
            remaining = &remaining[end..];
            continue;
        }
        if !remaining.starts_with('<') {
            let end = remaining.find('<').unwrap_or(remaining.len());
            if skipped.is_empty() {
                text.push_str(&remaining[..end]);
            }
            remaining = &remaining[end..];
            continue;
        }
        if remaining.starts_with("<!--") {
            let end = remaining.find("-->").ok_or_else(invalid_document)? + 3;
            remaining = &remaining[end..];
            continue;
        }
        let end = tag_end(remaining)?;
        let serialized_tag = &remaining[..end];
        remaining = &remaining[end..];
        let inside = serialized_tag[1..serialized_tag.len() - 1].trim();
        let closing = inside.starts_with('/');
        let tag = inside
            .trim_start_matches('/')
            .split(|c: char| c.is_whitespace() || c == '/')
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();
        if let Some(active) = skipped.last() {
            if closing && &tag == active {
                skipped.pop();
            } else if !closing
                && !inside.ends_with('/')
                && (matches!(tag.as_str(), "script" | "style") || &tag == active)
            {
                skipped.push(tag);
            }
            continue;
        }
        if !closing && matches!(tag.as_str(), "script" | "style" | "template" | "head") {
            if !inside.ends_with('/') {
                skipped.push(tag);
            }
            continue;
        }
        if block(&tag) {
            flush_text(&mut text, &mut pages, pre_depth > 0)?;
        }
        if tag == "pre" {
            if closing {
                pre_depth = pre_depth.saturating_sub(1);
            } else {
                pre_depth += 1;
            }
        }
        if !closing && matches!(tag.as_str(), "img" | "image") {
            flush_text(&mut text, &mut pages, pre_depth > 0)?;
            if let Some(reference) = image_reference(serialized_tag, tag == "image")?
                && !reference.is_empty()
            {
                let path = image_path(&reference, spine_path)?;
                let api_path = resource_api_path(book_id, &path)?;
                push_page(
                    &mut pages,
                    PageContent::Url(format!("ito://com.atiinlabs.komga{api_path}")),
                )?;
            }
        } else if !closing && matches!(tag.as_str(), "br" | "hr") {
            text.push(' ');
        } else if matches!(
            tag.as_str(),
            "a" | "em"
                | "strong"
                | "span"
                | "b"
                | "i"
                | "u"
                | "s"
                | "small"
                | "sup"
                | "sub"
                | "code"
                | "ruby"
                | "rt"
        ) {
            text.push_str(serialized_tag);
        }
    }
    flush_text(&mut text, &mut pages, pre_depth > 0)?;
    // Blank spine documents are valid. The service rejects only an entirely empty book.
    Ok(pages)
}

#[cfg(test)]
mod tests {
    use super::{image_path, raw_element_end, resource_api_path, spine_path, tag_end};

    #[test]
    fn manifest_paths_are_local_and_decoded_once() {
        for href in [
            "/api/v1/books/book-1/resource/OEBPS/Text/ch%201.xhtml?download=true#part",
            "https://example.invalid/api/v1/books/book-1/resource/OEBPS/Text/ch%201.xhtml#part",
            "https://example.invalid/komga/api/v1/books/book-1/resource/OEBPS/Text/ch%201.xhtml#part",
            "/reverse/proxy/api/v1/books/book-1/resource/OEBPS/Text/ch%201.xhtml#part",
        ] {
            assert_eq!(spine_path(href, "book-1").unwrap(), "OEBPS/Text/ch 1.xhtml");
        }
        assert_eq!(
            resource_api_path("book-1", "OEBPS/Text/ch 1.xhtml").unwrap(),
            "/api/v1/books/book-1/resource/OEBPS/Text/ch%201.xhtml"
        );
    }

    #[test]
    fn manifest_rejects_wrong_book_authority_and_traversal() {
        for href in [
            "/api/v1/books/other/resource/ch.xhtml",
            "//example.invalid/api/v1/books/book/resource/ch.xhtml",
            "file:///api/v1/books/book/resource/ch.xhtml",
            "https://user@example.invalid/api/v1/books/book/resource/ch.xhtml",
            "/api/v1/books/book/resource/",
            "/api/v1/books/book/resource/../ch.xhtml",
            "/api/v1/books/book/resource/a/./ch.xhtml",
            "/api/v1/books/book/resource/a//ch.xhtml",
            "/api/v1/books/book/resource/%2e%2e/ch.xhtml",
            "/api/v1/books/book/resource/%252e%252e/ch.xhtml",
            "/api/v1/books/book/resource/a%2f..%2fch.xhtml",
            "/api/v1/books/book/resource/a%5cch.xhtml",
            "/api/v1/books/book/resource/a%00ch.xhtml",
            "/api/v1/books/book/resource/a\\ch.xhtml",
            "/api/v1/books/book/resource/a%2.xhtml",
            "/api/v1/books/book/resource/a%GG.xhtml",
            "/api/v1/books/book/resource/a\n.xhtml",
        ] {
            assert!(spine_path(href, "book").is_err());
        }
    }

    #[test]
    fn image_references_resolve_inside_epub_root() {
        assert_eq!(
            image_path("../Images/a%20b.jpg?size=1#part", "OEBPS/Text/ch.xhtml").unwrap(),
            "OEBPS/Images/a b.jpg"
        );
        assert_eq!(
            image_path("../../cover.jpg", "OEBPS/Text/ch.xhtml").unwrap(),
            "cover.jpg"
        );
        assert_eq!(image_path("./a.jpg", "ch.xhtml").unwrap(), "a.jpg");
        assert_eq!(
            image_path("图 像.jpg", "Text/ch.xhtml").unwrap(),
            "Text/图 像.jpg"
        );
    }

    #[test]
    fn image_references_cannot_escape_or_change_authority() {
        for reference in [
            "../../../cover.jpg",
            "/cover.jpg",
            "//evil.invalid/a.jpg",
            "https://evil.invalid/a.jpg",
            "data:image/png;base64,AAAA",
            "javascript:alert(1)",
            "%2e%2e/a.jpg",
            "%252e%252e/a.jpg",
            "a%2fb.jpg",
            "a%5cb.jpg",
            "a\\b.jpg",
            "a%00.jpg",
            "a%7f.jpg",
            "a%FF.jpg",
            "a%.jpg",
            "a%GG.jpg",
            "a//b.jpg",
            "#fragment",
            "",
        ] {
            assert!(image_path(reference, "Text/ch.xhtml").is_err());
        }
    }

    #[test]
    fn outgoing_paths_encode_filenames_and_reject_ambiguous_inputs() {
        assert_eq!(
            resource_api_path("book", "a b/é?#.png").unwrap(),
            "/api/v1/books/book/resource/a%20b/%C3%A9%3F%23.png"
        );
        for path in [
            "",
            "/a",
            "a/",
            "a//b",
            "a/../b",
            "a/./b",
            "a\\b",
            "a%20b",
            "https:evil",
            "a\0b",
        ] {
            assert!(resource_api_path("book", path).is_err());
        }
        for book in ["", ".", "..", "a/b", "a%2fb", "a?b", "a\\b", "a\nb"] {
            assert!(resource_api_path(book, "a.jpg").is_err());
            assert!(spine_path("/api/v1/books/book/resource/a.xhtml", book).is_err());
        }
    }

    #[test]
    fn scanner_respects_quoted_markup_boundaries() {
        let markup = "<img src='a>b.jpg' title=\"x > y\">after";
        let end = tag_end(markup).unwrap();
        assert_eq!(&markup[end..], "after");
        assert!(tag_end("<img src='unterminated>").is_err());
    }

    #[test]
    fn raw_elements_do_not_tokenize_script_or_style_contents() {
        let script = "if (a < b) { value = '<img src=\"ignored.jpg\">'; }</ScRiPt><p>after</p>";
        let end = raw_element_end(script, "script").unwrap();
        assert_eq!(&script[end..], "<p>after</p>");
        let style = "a::before { content: '< >'; } </style><p>after</p>";
        let end = raw_element_end(style, "style").unwrap();
        assert_eq!(&style[end..], "<p>after</p>");
        assert!(raw_element_end("</scripted><p>not a closing tag</p>", "script").is_err());
        assert!(raw_element_end("unterminated < raw text", "style").is_err());
    }
}
