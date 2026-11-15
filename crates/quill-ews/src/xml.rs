//! As much XML as EWS takes and gives: a writer that escapes, and a reader
//! that trusts nothing.
//!
//! What is written holds ids and other text from callers, so every value
//! is escaped where it is put and cannot end its element or its attribute.
//! What is read came from a server that may be anyone's: it is held to a
//! depth and a number of elements, a document type is refused outright, and
//! no entity is known but the five XML itself defines, so there is nothing
//! to expand.

use std::borrow::Cow;
use std::fmt;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use quick_xml::escape::resolve_predefined_entity;
use quick_xml::events::{BytesStart, Event};
use quick_xml::name::ResolveResult;
use quick_xml::{NsReader, XmlVersion};

/// The namespace of the SOAP 1.1 envelope, which is what EWS speaks.
pub(crate) const SOAP: &str = "http://schemas.xmlsoap.org/soap/envelope/";
/// The namespace of the things EWS is about: items, folders, ids.
pub(crate) const TYPES: &str = "http://schemas.microsoft.com/exchange/services/2006/types";
/// The namespace of what is asked of EWS and what it answers.
pub(crate) const MESSAGES: &str = "http://schemas.microsoft.com/exchange/services/2006/messages";
/// The namespace of the detail in a SOAP fault from EWS.
const ERRORS: &str = "http://schemas.microsoft.com/exchange/services/2006/errors";
/// SOAP 1.2. EWS does not answer in it; a fault in it is still a fault.
const SOAP_12: &str = "http://www.w3.org/2003/05/soap-envelope";

/// How deep elements may be inside one another. EWS goes a dozen deep.
const DEPTH: usize = 64;

/// How many elements one answer may hold. A full round of a folder sync
/// has some thirty thousand.
const ELEMENTS: usize = 1_000_000;

/// How many attributes one element may have. EWS uses four at most.
const ATTRIBUTES: usize = 64;

/// Writes one XML document, element by element.
///
/// Names are the program's own and written as they are. Values are
/// escaped. An element is closed by [`Writer::close`] in the order it was
/// opened, so the document cannot come out lopsided.
pub(crate) struct Writer {
    out: String,
    open: Vec<&'static str>,
}

impl Writer {
    pub fn new() -> Self {
        Self {
            out: String::from("<?xml version=\"1.0\" encoding=\"utf-8\"?>"),
            open: Vec::new(),
        }
    }

    fn tag(&mut self, name: &'static str, attributes: &[(&'static str, &str)]) {
        self.out.push('<');
        self.out.push_str(name);
        for (key, value) in attributes {
            self.out.push(' ');
            self.out.push_str(key);
            self.out.push_str("=\"");
            escape(&mut self.out, value, true);
            self.out.push('"');
        }
    }

    /// `<name key="value">`, to be closed later.
    pub fn open(&mut self, name: &'static str, attributes: &[(&'static str, &str)]) -> &mut Self {
        self.tag(name, attributes);
        self.out.push('>');
        self.open.push(name);
        self
    }

    /// Closes the element opened last and not yet closed.
    pub fn close(&mut self) -> &mut Self {
        if let Some(name) = self.open.pop() {
            self.out.push_str("</");
            self.out.push_str(name);
            self.out.push('>');
        }
        self
    }

    /// `<name key="value"/>`.
    pub fn empty(&mut self, name: &'static str, attributes: &[(&'static str, &str)]) -> &mut Self {
        self.tag(name, attributes);
        self.out.push_str("/>");
        self
    }

    /// `<name>text</name>`.
    pub fn text(&mut self, name: &'static str, text: &str) -> &mut Self {
        self.tag(name, &[]);
        self.out.push('>');
        escape(&mut self.out, text, false);
        self.out.push_str("</");
        self.out.push_str(name);
        self.out.push('>');
        self
    }

    /// `<name key="value">` with `bytes` inside as base64, written straight
    /// into the document: a message of many megabytes is not copied twice.
    pub fn base64(
        &mut self,
        name: &'static str,
        attributes: &[(&'static str, &str)],
        bytes: &[u8],
    ) -> &mut Self {
        self.tag(name, attributes);
        self.out.push('>');
        BASE64.encode_string(bytes, &mut self.out);
        self.out.push_str("</");
        self.out.push_str(name);
        self.out.push('>');
        self
    }

    /// The document, with whatever was still open closed.
    pub fn finish(mut self) -> String {
        while !self.open.is_empty() {
            self.close();
        }
        self.out
    }
}

/// Writes `value` so that it reads back as itself and as nothing else.
///
/// The markup characters become references. In an attribute so do the
/// quotes and the white space a reader would otherwise turn into spaces;
/// in text a carriage return, which a reader would otherwise drop or turn
/// into a line feed. A character XML has no way of writing at all (most
/// control characters) becomes U+FFFD: nothing an Exchange server names
/// has one in it.
fn escape(out: &mut String, value: &str, attribute: bool) {
    for c in value.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' if attribute => out.push_str("&quot;"),
            '\'' if attribute => out.push_str("&apos;"),
            '\t' if attribute => out.push_str("&#9;"),
            '\n' if attribute => out.push_str("&#10;"),
            '\r' => out.push_str("&#13;"),
            '\t' | '\n' => out.push(c),
            c if c < ' ' || c == '\u{fffe}' || c == '\u{ffff}' => out.push('\u{fffd}'),
            c => out.push(c),
        }
    }
}

/// Which vocabulary an element belongs to, whatever prefix the server
/// chose to write it with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Ns {
    Soap,
    Types,
    Messages,
    Errors,
    /// Some other namespace, or none.
    Other,
}

impl Ns {
    fn named(resolved: &ResolveResult<'_>) -> Self {
        let ResolveResult::Bound(namespace) = resolved else {
            return Self::Other;
        };
        match namespace.0 {
            SOAP | SOAP_12 => Self::Soap,
            TYPES => Self::Types,
            MESSAGES => Self::Messages,
            ERRORS => Self::Errors,
            _ => Self::Other,
        }
    }
}

/// One element of an answer: its name, its own attributes, the text
/// directly inside it and the elements inside it.
///
/// Text is borrowed from the answer where it can be, so a message of many
/// megabytes inside an element is not copied to be looked at.
#[derive(Debug)]
pub(crate) struct Element<'a> {
    ns: Ns,
    name: String,
    /// Attributes without a prefix, which is all EWS has. Declarations of
    /// namespaces are not among them.
    attributes: Vec<(String, String)>,
    text: Cow<'a, str>,
    children: Vec<Element<'a>>,
}

impl<'a> Element<'a> {
    fn new(ns: Ns, start: &BytesStart<'_>) -> Result<Self, Malformed> {
        let mut attributes = Vec::new();
        for attribute in start.attributes() {
            let attribute = attribute.map_err(|err| Malformed(err.to_string()))?;
            // `xmlns`, `xmlns:t`, `xml:lang`, `xsi:nil`: none is anything
            // EWS says about an item.
            if attribute.key.as_namespace_binding().is_some() || attribute.key.prefix().is_some() {
                continue;
            }
            if attributes.len() == ATTRIBUTES {
                return Err(Malformed(format!(
                    "an element with more than {ATTRIBUTES} attributes"
                )));
            }
            let value = attribute
                .normalized_value(XmlVersion::Implicit1_0)
                .map_err(|err| Malformed(err.to_string()))?;
            attributes.push((attribute.key.0.to_owned(), value.into_owned()));
        }
        Ok(Self {
            ns,
            name: start.local_name().into_inner().to_owned(),
            attributes,
            text: Cow::Borrowed(""),
            children: Vec::new(),
        })
    }

    fn push_text(&mut self, text: Cow<'a, str>) {
        if self.text.is_empty() {
            self.text = text;
        } else {
            self.text.to_mut().push_str(&text);
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn is(&self, ns: Ns, name: &str) -> bool {
        self.ns == ns && self.name == name
    }

    pub fn attribute(&self, name: &str) -> Option<&str> {
        self.attributes
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    /// The text directly inside the element, without the white space
    /// around it.
    pub fn text(&self) -> &str {
        self.text.trim()
    }

    pub fn children(&self) -> &[Element<'a>] {
        &self.children
    }

    /// The first element directly inside this one with that name.
    pub fn child(&self, ns: Ns, name: &str) -> Option<&Element<'a>> {
        self.children.iter().find(|child| child.is(ns, name))
    }

    /// The first element directly inside this one called `name`, in
    /// whatever namespace. For the parts of a SOAP fault, which are in
    /// none, or in the envelope's if the server gave it a default one.
    pub fn child_called(&self, name: &str) -> Option<&Element<'a>> {
        self.children.iter().find(|child| child.name == name)
    }

    /// The text of [`Element::child`], if there is such an element.
    pub fn child_text(&self, ns: Ns, name: &str) -> Option<&str> {
        self.child(ns, name).map(Element::text)
    }

    /// The first element anywhere inside this one that `wanted` picks,
    /// parents before their children.
    pub fn find(&self, wanted: &impl Fn(&Element<'a>) -> bool) -> Option<&Element<'a>> {
        // As deep as the document, which `parse` has bounded.
        self.children.iter().find_map(|child| {
            if wanted(child) {
                Some(child)
            } else {
                child.find(wanted)
            }
        })
    }
}

/// Why an answer is not the XML it should be. In words of this program's
/// or the parser's own, which name a place and never quote the answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Malformed(String);

impl fmt::Display for Malformed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Reads a whole document and returns its outermost element.
pub(crate) fn parse(document: &str) -> Result<Element<'_>, Malformed> {
    let mut reader = NsReader::from_str(document);
    // The elements above the one being read, outermost first.
    let mut above: Vec<Element<'_>> = Vec::new();
    let mut root: Option<Element<'_>> = None;
    let mut elements = 0_usize;
    loop {
        let (resolved, event) = reader
            .read_resolved_event()
            .map_err(|err| Malformed(err.to_string()))?;
        match event {
            Event::Start(start) => {
                if root.is_some() {
                    return Err(Malformed("more than one outermost element".to_owned()));
                }
                if above.len() == DEPTH {
                    return Err(Malformed(format!(
                        "elements more than {DEPTH} deep inside one another"
                    )));
                }
                count(&mut elements)?;
                above.push(Element::new(Ns::named(&resolved), &start)?);
            }
            Event::Empty(start) => {
                if root.is_some() {
                    return Err(Malformed("more than one outermost element".to_owned()));
                }
                count(&mut elements)?;
                let element = Element::new(Ns::named(&resolved), &start)?;
                match above.last_mut() {
                    Some(parent) => parent.children.push(element),
                    None => root = Some(element),
                }
            }
            Event::End(_) => {
                let Some(element) = above.pop() else {
                    return Err(Malformed(
                        "an element is closed that was never opened".to_owned(),
                    ));
                };
                match above.last_mut() {
                    Some(parent) => parent.children.push(element),
                    None => root = Some(element),
                }
            }
            Event::Text(text) => {
                if let Some(element) = above.last_mut() {
                    let text = text.into_inner();
                    // The line breaks and indentation between elements
                    // are not anybody's text.
                    if !text.trim().is_empty() || !element.text.is_empty() {
                        element.push_text(text);
                    }
                }
            }
            Event::CData(text) => {
                if let Some(element) = above.last_mut() {
                    element.push_text(text.into_inner());
                }
            }
            Event::GeneralRef(reference) => {
                let Some(element) = above.last_mut() else {
                    return Err(Malformed("a reference outside any element".to_owned()));
                };
                let character = reference
                    .resolve_char_ref()
                    .map_err(|err| Malformed(err.to_string()))?;
                match character {
                    Some(character) => element.text.to_mut().push(character),
                    // `&lt;` and its four fellows. There are no others: a
                    // document type, which could define some, is refused.
                    None => match resolve_predefined_entity(&reference) {
                        Some(text) => element.text.to_mut().push_str(text),
                        None => {
                            return Err(Malformed(
                                "a reference to an entity XML does not define".to_owned(),
                            ));
                        }
                    },
                }
            }
            // Exchange never sends one. Somebody who does is after the
            // entities it can define, or the files it can name.
            Event::DocType(_) => {
                return Err(Malformed(
                    "a document type declaration, which no answer from Exchange has".to_owned(),
                ));
            }
            Event::Decl(_) | Event::Comment(_) | Event::PI(_) => {}
            Event::Eof => break,
        }
    }
    if !above.is_empty() {
        return Err(Malformed("the document ends in the middle".to_owned()));
    }
    root.ok_or_else(|| Malformed("no element at all".to_owned()))
}

fn count(elements: &mut usize) -> Result<(), Malformed> {
    *elements += 1;
    if *elements > ELEMENTS {
        return Err(Malformed(format!("more than {ELEMENTS} elements")));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_document_is_written_element_by_element() {
        let mut xml = Writer::new();
        xml.open("m:GetItem", &[]);
        xml.open("m:ItemShape", &[]);
        xml.text("t:BaseShape", "IdOnly");
        xml.close();
        xml.open("m:ItemIds", &[]);
        xml.empty("t:ItemId", &[("Id", "AAMkAGI="), ("ChangeKey", "CQAAAA==")]);
        assert_eq!(
            xml.finish(),
            "<?xml version=\"1.0\" encoding=\"utf-8\"?>\
             <m:GetItem><m:ItemShape><t:BaseShape>IdOnly</t:BaseShape></m:ItemShape>\
             <m:ItemIds><t:ItemId Id=\"AAMkAGI=\" ChangeKey=\"CQAAAA==\"/></m:ItemIds></m:GetItem>"
        );
    }

    #[test]
    fn closing_more_than_was_opened_writes_nothing() {
        let mut xml = Writer::new();
        xml.open("a", &[]).close().close().close();
        assert_eq!(
            xml.finish(),
            "<?xml version=\"1.0\" encoding=\"utf-8\"?><a></a>"
        );
    }

    #[test]
    fn a_value_cannot_get_out_of_its_attribute() {
        let mut xml = Writer::new();
        xml.empty(
            "t:ItemId",
            &[("Id", "x\"/><t:ItemId Id='y' & more\t\r\n<!--")],
        );
        let written = xml.finish();
        assert!(written.ends_with(
            "<t:ItemId Id=\"x&quot;/&gt;&lt;t:ItemId Id=&apos;y&apos; &amp; more&#9;&#13;&#10;&lt;!--\"/>"
        ));
        // And read back, it is one element with the value as it was given.
        let read = parse(&written).unwrap();
        assert!(read.children().is_empty());
        assert_eq!(
            read.attribute("Id"),
            Some("x\"/><t:ItemId Id='y' & more\t\r\n<!--")
        );
    }

    #[test]
    fn a_value_cannot_get_out_of_its_element() {
        let hostile = "</m:SyncState><m:SyncState>other]]>&amp;<![CDATA[ \"quoted\" 'single'\r\n";
        let mut xml = Writer::new();
        xml.open("a", &[]).text("b", hostile);
        let written = xml.finish();
        assert!(written.contains(
            "<b>&lt;/m:SyncState&gt;&lt;m:SyncState&gt;other]]&gt;&amp;amp;&lt;![CDATA[ \"quoted\" 'single'&#13;\n</b>"
        ));
        let read = parse(&written).unwrap();
        assert_eq!(read.children().len(), 1);
        // Trimmed of the line end, like all text read here.
        assert_eq!(read.children()[0].text(), hostile.trim_end());
    }

    #[test]
    fn what_xml_cannot_write_is_replaced_and_the_rest_left_alone() {
        let mut xml = Writer::new();
        xml.text(
            "a",
            "nul\u{0}esc\u{1b}bell\u{7}\u{fffe}\u{ffff} é 漢 🙂 \u{7f}",
        );
        assert!(
            xml.finish().ends_with(
                "<a>nul\u{fffd}esc\u{fffd}bell\u{fffd}\u{fffd}\u{fffd} é 漢 🙂 \u{7f}</a>"
            )
        );
        let mut xml = Writer::new();
        xml.empty("a", &[("b", "\u{0}\u{1f}")]);
        assert!(xml.finish().ends_with("<a b=\"\u{fffd}\u{fffd}\"/>"));
    }

    #[test]
    fn bytes_go_in_as_base64() {
        let mut xml = Writer::new();
        xml.base64(
            "t:MimeContent",
            &[("CharacterSet", "UTF-8")],
            b"Subject: x\r\n\r\n\xff\x00",
        );
        assert!(xml.finish().ends_with(
            "<t:MimeContent CharacterSet=\"UTF-8\">U3ViamVjdDogeA0KDQr/AA==</t:MimeContent>"
        ));
    }

    const ANSWER: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/">
  <s:Header>
    <h:ServerVersionInfo MajorVersion="15" MinorVersion="2" Version="V2017_07_11"
        xmlns:h="http://schemas.microsoft.com/exchange/services/2006/types"/>
  </s:Header>
  <s:Body>
    <!-- Prefixes are the server's to choose. -->
    <GetItemResponse xmlns="http://schemas.microsoft.com/exchange/services/2006/messages"
        xmlns:types="http://schemas.microsoft.com/exchange/services/2006/types">
      <ResponseMessages>
        <GetItemResponseMessage ResponseClass="Success">
          <ResponseCode>NoError</ResponseCode>
          <Items>
            <types:Message>
              <types:ItemId Id="AAMk&amp;1" ChangeKey="CQ=="/>
              <types:Subject>Fish &amp; chips &#x2014; <![CDATA[<today>]]> &lt;b&gt;</types:Subject>
            </types:Message>
          </Items>
        </GetItemResponseMessage>
      </ResponseMessages>
    </GetItemResponse>
  </s:Body>
</s:Envelope>"#;

    #[test]
    fn elements_are_known_by_namespace_whatever_the_prefix() {
        let envelope = parse(ANSWER).unwrap();
        assert!(envelope.is(Ns::Soap, "Envelope"));
        let header = envelope.child(Ns::Soap, "Header").unwrap();
        assert_eq!(
            header
                .child(Ns::Types, "ServerVersionInfo")
                .unwrap()
                .attribute("MajorVersion"),
            Some("15")
        );
        let body = envelope.child(Ns::Soap, "Body").unwrap();
        let response = body.child(Ns::Messages, "GetItemResponse").unwrap();
        // Not found under the wrong namespace.
        assert!(body.child(Ns::Types, "GetItemResponse").is_none());
        let message = response
            .child(Ns::Messages, "ResponseMessages")
            .unwrap()
            .child(Ns::Messages, "GetItemResponseMessage")
            .unwrap();
        assert_eq!(message.attribute("ResponseClass"), Some("Success"));
        assert_eq!(
            message.child_text(Ns::Messages, "ResponseCode"),
            Some("NoError")
        );
        let item = message
            .find(&|element| element.is(Ns::Types, "Message"))
            .unwrap();
        assert_eq!(
            item.child(Ns::Types, "ItemId").unwrap().attribute("Id"),
            Some("AAMk&1")
        );
        assert_eq!(
            item.child_text(Ns::Types, "Subject"),
            Some("Fish & chips \u{2014} <today> <b>")
        );
        // Indentation is nobody's text.
        assert_eq!(message.text(), "");
        assert_eq!(item.name(), "Message");
    }

    #[test]
    fn an_unknown_or_missing_namespace_is_other() {
        let root = parse("<a xmlns:x=\"urn:example\"><x:b/><c/><y:d/></a>");
        // An undeclared prefix is the parser's to refuse or not; either
        // way nothing is taken for an EWS element.
        if let Ok(root) = root {
            assert!(root.children().iter().all(|child| child.ns == Ns::Other));
        }
        let root = parse("<a xmlns:x=\"urn:example\"><x:b/><c/></a>").unwrap();
        assert_eq!(root.ns, Ns::Other);
        assert!(root.child(Ns::Other, "b").is_some());
        assert!(root.child_called("c").is_some());
    }

    #[test]
    fn a_document_type_is_refused_and_no_entity_is_expanded() {
        // The "billion laughs", two levels of it.
        let bomb = "<?xml version=\"1.0\"?>\
            <!DOCTYPE lolz [<!ENTITY lol \"lol\"><!ENTITY lol2 \"&lol;&lol;&lol;&lol;\">]>\
            <a>&lol2;</a>";
        let err = parse(bomb).unwrap_err();
        assert!(err.to_string().contains("document type"), "{err}");
        // A file on this machine, by name.
        let external = "<!DOCTYPE a [<!ENTITY xxe SYSTEM \"file:///etc/passwd\">]><a>&xxe;</a>";
        assert!(parse(external).is_err());
        // Without the declaration the reference is to nothing.
        let err = parse("<a>&lol2;</a>").unwrap_err();
        assert!(err.to_string().contains("entity"), "{err}");
        assert!(parse("<a b=\"&lol2;\"/>").is_err());
        // The five XML defines, and characters by number, are text.
        let root = parse("<a b=\"&quot;&#65;\">&lt;&gt;&amp;&apos;&quot;&#x41;&#66;</a>").unwrap();
        assert_eq!(root.text(), "<>&'\"AB");
        assert_eq!(root.attribute("b"), Some("\"A"));
    }

    #[test]
    fn nesting_is_bounded() {
        let deep = |depth: usize| format!("{}{}", "<a>".repeat(depth), "</a>".repeat(depth));
        assert!(parse(&deep(DEPTH)).is_ok());
        let err = parse(&deep(DEPTH + 1)).unwrap_err();
        assert!(err.to_string().contains("deep"), "{err}");
        // Far past the bound, and never closed: refused long before the end.
        assert!(parse(&"<a>".repeat(200_000)).is_err());
    }

    #[test]
    fn the_number_of_elements_is_bounded() {
        let wide = |count: usize| format!("<a>{}</a>", "<b/>".repeat(count));
        assert_eq!(parse(&wide(1000)).unwrap().children().len(), 1000);
        let err = parse(&wide(ELEMENTS)).unwrap_err();
        assert!(err.to_string().contains("elements"), "{err}");
        let many = format!(
            "<a {}/>",
            (0..=ATTRIBUTES)
                .map(|i| format!("b{i}=\"\""))
                .collect::<Vec<_>>()
                .join(" ")
        );
        assert!(parse(&many).is_err());
    }

    #[test]
    fn what_is_not_a_whole_document_is_an_error_and_never_a_panic() {
        for broken in [
            "",
            "   ",
            "not xml at all",
            "<",
            "<a",
            "<a>",
            "<a><b></a>",
            "</a>",
            "<a></b>",
            "<a/><b/>",
            "<a></a><b></b>",
            "<a b=\"1\" b=\"2\"/>",
            "<a b=1/>",
            "<a>&#0;</a>",
            "<a>&#xFFFFFFFF;</a>",
            "<a>&;</a>",
            "<a>& </a>",
            "<a><![CDATA[never closed</a>",
            "<a><!-- never closed</a>",
            "<?xml version=\"1.0\"?>",
            "<s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\"><s:Body>",
            "\u{feff}",
        ] {
            assert!(parse(broken).is_err(), "{broken:?}");
        }
        // Cut off at every byte, the answer above is an error and no more.
        for end in 0..ANSWER.len() - 1 {
            if ANSWER.is_char_boundary(end) {
                let cut = &ANSWER[..end];
                // Nothing is left out of the envelope's last tag but
                // white space, so only that one cut may still be whole.
                assert!(
                    parse(cut).is_err() || cut.trim_end().ends_with("</s:Envelope>"),
                    "cut at {end}"
                );
            }
        }
    }

    #[test]
    fn large_text_is_borrowed_not_copied() {
        let document = format!("<a><b>{}</b></a>", "QUJD".repeat(250_000));
        let root = parse(&document).unwrap();
        let text = &root.children()[0].text;
        assert_eq!(text.len(), 1_000_000);
        assert!(matches!(text, Cow::Borrowed(_)));
    }
}
