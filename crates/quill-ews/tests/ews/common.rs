//! A stand-in for an Exchange server on this machine, and a source of
//! passwords that counts.
//!
//! Nothing here or in the tests reaches a real server. The backend is
//! built with `EwsBackend::loopback`, which refuses an address that is not
//! this machine's, and every test ends with [`Rig::check`]: each request
//! the backend made took one password from the source, so if the stand-in
//! received as many requests as passwords were handed out, none went
//! anywhere else. A test that forgets the check fails when its rig is
//! dropped.
//!
//! Everyone in the mail is invented and lives at example.com.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use quill_core::{Credential, Credentials, Result};
use quill_ews::{EwsBackend, EwsConfig};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockBuilder, MockServer, Request, ResponseTemplate};

/// Where EWS lives on an Exchange server.
pub const ENDPOINT: &str = "/EWS/Exchange.asmx";

/// Who the account signs in as.
pub const USER: &str = "robin@example.com";

/// What every request begins with: the envelope, and the schema asked for.
pub const HEAD: &str = "<?xml version=\"1.0\" encoding=\"utf-8\"?>\
    <soap:Envelope xmlns:soap=\"http://schemas.xmlsoap.org/soap/envelope/\" \
    xmlns:t=\"http://schemas.microsoft.com/exchange/services/2006/types\" \
    xmlns:m=\"http://schemas.microsoft.com/exchange/services/2006/messages\">\
    <soap:Header><t:RequestServerVersion Version=\"Exchange2013_SP1\"/></soap:Header>\
    <soap:Body>";

/// What every request ends with.
pub const TAIL: &str = "</soap:Body></soap:Envelope>";

/// What a folder sync asks for about each message.
pub const LISTED: &str = "<t:BaseShape>IdOnly</t:BaseShape><t:AdditionalProperties>\
    <t:FieldURI FieldURI=\"item:Subject\"/>\
    <t:FieldURI FieldURI=\"item:DateTimeReceived\"/>\
    <t:FieldURI FieldURI=\"item:DateTimeSent\"/>\
    <t:FieldURI FieldURI=\"item:Size\"/>\
    <t:FieldURI FieldURI=\"item:HasAttachments\"/>\
    <t:FieldURI FieldURI=\"item:IsDraft\"/>\
    <t:FieldURI FieldURI=\"item:ConversationId\"/>\
    <t:FieldURI FieldURI=\"item:Flag\"/>\
    <t:FieldURI FieldURI=\"message:From\"/>\
    <t:FieldURI FieldURI=\"message:IsRead\"/>\
    <t:FieldURI FieldURI=\"message:InternetMessageId\"/>\
    <t:ExtendedFieldURI PropertyTag=\"0x1081\" PropertyType=\"Integer\"/>\
    </t:AdditionalProperties>";

/// What is asked for about each message a sync reported, by id.
pub const WHOLE: &str = "<t:BaseShape>IdOnly</t:BaseShape><t:AdditionalProperties>\
    <t:FieldURI FieldURI=\"item:Subject\"/>\
    <t:FieldURI FieldURI=\"item:DateTimeReceived\"/>\
    <t:FieldURI FieldURI=\"item:DateTimeSent\"/>\
    <t:FieldURI FieldURI=\"item:Size\"/>\
    <t:FieldURI FieldURI=\"item:HasAttachments\"/>\
    <t:FieldURI FieldURI=\"item:IsDraft\"/>\
    <t:FieldURI FieldURI=\"item:ConversationId\"/>\
    <t:FieldURI FieldURI=\"item:Flag\"/>\
    <t:FieldURI FieldURI=\"message:From\"/>\
    <t:FieldURI FieldURI=\"message:IsRead\"/>\
    <t:FieldURI FieldURI=\"message:InternetMessageId\"/>\
    <t:FieldURI FieldURI=\"item:InReplyTo\"/>\
    <t:FieldURI FieldURI=\"message:ToRecipients\"/>\
    <t:FieldURI FieldURI=\"message:CcRecipients\"/>\
    <t:FieldURI FieldURI=\"message:References\"/>\
    <t:ExtendedFieldURI PropertyTag=\"0x1081\" PropertyType=\"Integer\"/>\
    </t:AdditionalProperties>";

/// Hands out `test-password-1` until told it was refused, then
/// `test-password-2`, and so on. Or tokens, numbered the same way.
#[derive(Default)]
pub struct Secrets {
    pub handed_out: AtomicUsize,
    pub refused: AtomicUsize,
    tokens: bool,
}

impl Secrets {
    /// The `Authorization` header the `number`th secret makes.
    pub fn authorization(&self, number: usize) -> String {
        if self.tokens {
            format!("Bearer test-access-token-{number}")
        } else {
            format!(
                "Basic {}",
                BASE64.encode(format!("{USER}:test-password-{number}"))
            )
        }
    }
}

#[async_trait]
impl Credentials for Secrets {
    async fn get(&self) -> Result<Credential> {
        self.handed_out.fetch_add(1, Ordering::SeqCst);
        let number = self.refused.load(Ordering::SeqCst) + 1;
        Ok(if self.tokens {
            Credential::OAuthToken(format!("test-access-token-{number}"))
        } else {
            Credential::Password(format!("test-password-{number}"))
        })
    }

    async fn refused(&self) {
        self.refused.fetch_add(1, Ordering::SeqCst);
    }
}

pub struct Rig {
    pub server: MockServer,
    pub secrets: Arc<Secrets>,
    pub ews: EwsBackend,
    page_size: usize,
    checked: AtomicBool,
}

impl Rig {
    pub async fn start() -> Self {
        Self::build(50, false).await
    }

    pub async fn with_page_size(page_size: usize) -> Self {
        Self::build(page_size, false).await
    }

    /// A rig whose account signs in with an OAuth token.
    pub async fn with_tokens() -> Self {
        Self::build(50, true).await
    }

    async fn build(page_size: usize, tokens: bool) -> Self {
        let server = MockServer::start().await;
        let secrets = Arc::new(Secrets {
            tokens,
            ..Secrets::default()
        });
        let ews = backend(&server, page_size, &secrets);
        Self {
            server,
            secrets,
            ews,
            page_size,
            checked: AtomicBool::new(false),
        }
    }

    /// Another backend for the same stand-in and source of passwords, to
    /// be set up differently from the rig's own.
    pub fn backend(&self) -> EwsBackend {
        backend(&self.server, self.page_size, &self.secrets)
    }

    /// Starts a mock for one EWS operation, such as `GetItem`.
    pub fn on(&self, operation: &str) -> MockBuilder {
        Mock::given(method("POST"))
            .and(path(ENDPOINT))
            .and(header("SOAPAction", action(operation).as_str()))
    }

    /// Mounts `answer` as what the stand-in says to `operation`.
    pub async fn answers(&self, operation: &str, answer: ResponseTemplate) {
        self.on(operation)
            .respond_with(answer)
            .mount(&self.server)
            .await;
    }

    /// Everything the stand-in has received, oldest first.
    pub async fn requests(&self) -> Vec<Request> {
        self.server.received_requests().await.unwrap_or_default()
    }

    /// Checks that every request the backend made came here, signed with a
    /// secret from the source, as an envelope asking for the one schema.
    /// Returns what each request asked, without the envelope, for the test
    /// to look at.
    pub async fn check(&self) -> Vec<String> {
        self.check_but(0).await
    }

    /// As [`Rig::check`], for a test in which `lost` requests were meant
    /// never to arrive.
    pub async fn check_but(&self, lost: usize) -> Vec<String> {
        self.checked.store(true, Ordering::SeqCst);
        let requests = self.requests().await;
        assert_eq!(
            requests.len() + lost,
            self.secrets.handed_out.load(Ordering::SeqCst),
            "a request went somewhere other than the test's own server"
        );
        let secrets = self.secrets.refused.load(Ordering::SeqCst) + 1;
        requests
            .iter()
            .map(|request| {
                assert_eq!(request.method.as_str(), "POST");
                assert_eq!(request.url.path(), ENDPOINT);
                assert_eq!(request.url.query(), None);
                let authorization = one_header(request, "authorization");
                assert!(
                    (1..=secrets).any(|number| self.secrets.authorization(number) == authorization),
                    "a request was not signed with a secret from the source: {authorization:?}"
                );
                assert_eq!(
                    one_header(request, "content-type"),
                    "text/xml; charset=utf-8"
                );
                assert!(one_header(request, "user-agent").starts_with("Quill/"));
                let asked = asked(request);
                // The header names the operation the body asks for.
                let operation = asked
                    .strip_prefix("<m:")
                    .and_then(|rest| rest.split([' ', '>']).next())
                    .unwrap_or_default();
                assert_eq!(one_header(request, "soapaction"), action(operation));
                asked
            })
            .collect()
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            assert!(
                self.checked.load(Ordering::SeqCst),
                "the test did not check where its requests went"
            );
        }
    }
}

fn backend(server: &MockServer, page_size: usize, secrets: &Arc<Secrets>) -> EwsBackend {
    EwsBackend::loopback(
        EwsConfig {
            page_size,
            ..EwsConfig::new(format!("{}{ENDPOINT}", server.uri()), USER)
        },
        secrets.clone(),
    )
    .unwrap()
}

/// The `SOAPAction` header of `operation`, quotes and all.
fn action(operation: &str) -> String {
    format!("\"http://schemas.microsoft.com/exchange/services/2006/messages/{operation}\"")
}

/// What a request asks: its body without the envelope every request has.
pub fn asked(request: &Request) -> String {
    let body = std::str::from_utf8(&request.body).expect("a request that is not UTF-8");
    body.strip_prefix(HEAD)
        .and_then(|rest| rest.strip_suffix(TAIL))
        .unwrap_or_else(|| panic!("a request without the envelope: {body}"))
        .to_owned()
}

/// The one value of a request header, or nothing.
pub fn one_header(request: &Request, name: &str) -> String {
    let mut values = request.headers.get_all(name).iter();
    match (values.next(), values.next()) {
        (Some(value), None) => value.to_str().unwrap_or_default().to_owned(),
        (None, _) => String::new(),
        (Some(_), Some(_)) => panic!("{name} was sent more than once"),
    }
}

/// `<t:ItemId>` elements for these ids, as a request lists them.
pub fn item_ids(ids: &[&str]) -> String {
    let ids: String = ids
        .iter()
        .map(|id| format!("<t:ItemId Id=\"{id}\"/>"))
        .collect();
    format!("<m:ItemIds>{ids}</m:ItemIds>")
}

/// An HTTP answer with `body` inside a SOAP envelope, the way Exchange
/// wraps one.
pub fn soap(status: u16, body: &str) -> ResponseTemplate {
    let envelope = format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\
         <s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\">\
         <s:Header><h:ServerVersionInfo MajorVersion=\"15\" MinorVersion=\"2\" \
         MajorBuildNumber=\"1544\" MinorBuildNumber=\"11\" Version=\"V2017_07_11\" \
         xmlns:h=\"http://schemas.microsoft.com/exchange/services/2006/types\" \
         xmlns:xsd=\"http://www.w3.org/2001/XMLSchema\" \
         xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\"/></s:Header>\
         <s:Body xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\" \
         xmlns:xsd=\"http://www.w3.org/2001/XMLSchema\">{body}</s:Body></s:Envelope>"
    );
    ResponseTemplate::new(status).set_body_raw(envelope, "text/xml; charset=utf-8")
}

/// Exchange's answer to `operation`: one message for each thing asked.
pub fn answer(operation: &str, messages: &[String]) -> ResponseTemplate {
    soap(
        200,
        &format!(
            "<m:{operation}Response \
             xmlns:m=\"http://schemas.microsoft.com/exchange/services/2006/messages\" \
             xmlns:t=\"http://schemas.microsoft.com/exchange/services/2006/types\">\
             <m:ResponseMessages>{}</m:ResponseMessages></m:{operation}Response>",
            messages.concat()
        ),
    )
}

/// The answer to one thing asked for, when it worked.
pub fn success(operation: &str, inside: &str) -> String {
    format!(
        "<m:{operation}ResponseMessage ResponseClass=\"Success\">\
         <m:ResponseCode>NoError</m:ResponseCode>{inside}</m:{operation}ResponseMessage>"
    )
}

/// The answer to one thing asked for, when it did not.
pub fn failure(operation: &str, code: &str, text: &str) -> String {
    format!(
        "<m:{operation}ResponseMessage ResponseClass=\"Error\">\
         <m:MessageText>{text}</m:MessageText><m:ResponseCode>{code}</m:ResponseCode>\
         <m:DescriptiveLinkKey>0</m:DescriptiveLinkKey></m:{operation}ResponseMessage>"
    )
}

/// What Exchange says about a message that is not there.
pub fn gone(operation: &str) -> String {
    failure(
        operation,
        "ErrorItemNotFound",
        "The specified object was not found in the store.",
    )
}

/// The refusal of a whole request, as Exchange words one.
pub fn fault(code: &str, text: &str, back_off: Option<u64>) -> ResponseTemplate {
    let wait = back_off
        .map(|milliseconds| {
            format!(
                "<t:MessageXml xmlns:t=\"http://schemas.microsoft.com/exchange/services/2006/types\">\
                 <t:Value Name=\"BackOffMilliseconds\">{milliseconds}</t:Value></t:MessageXml>"
            )
        })
        .unwrap_or_default();
    soap(
        500,
        &format!(
            "<s:Fault><faultcode \
             xmlns:a=\"http://schemas.microsoft.com/exchange/services/2006/types\">a:{code}</faultcode>\
             <faultstring xml:lang=\"en-US\">{text}</faultstring><detail>\
             <e:ResponseCode xmlns:e=\"http://schemas.microsoft.com/exchange/services/2006/errors\">{code}</e:ResponseCode>\
             <e:Message xmlns:e=\"http://schemas.microsoft.com/exchange/services/2006/errors\">{text}</e:Message>\
             {wait}</detail></s:Fault>"
        ),
    )
}

/// Exchange too busy to answer, asking to be left alone for a while.
pub fn busy(back_off: Option<u64>) -> ResponseTemplate {
    fault(
        "ErrorServerBusy",
        "The server cannot service this request right now. Try again later.",
        back_off,
    )
}

/// A message as a folder sync lists it: its sender by name alone, and
/// nobody it is to.
pub fn listed(id: &str, subject: &str, from: (&str, &str), received: &str, read: bool) -> String {
    format!(
        "<t:Message><t:ItemId Id=\"{id}\" ChangeKey=\"CQAAABYAAAB1{id}\"/>\
         <t:Subject>{subject}</t:Subject>\
         <t:DateTimeReceived>{received}</t:DateTimeReceived><t:Size>4821</t:Size>\
         <t:DateTimeSent>{received}</t:DateTimeSent>\
         <t:HasAttachments>false</t:HasAttachments><t:IsDraft>false</t:IsDraft>\
         <t:ConversationId Id=\"topic-{id}\"/>\
         <t:Flag><t:FlagStatus>NotFlagged</t:FlagStatus></t:Flag>\
         <t:From><t:Mailbox><t:Name>{}</t:Name></t:Mailbox></t:From>\
         <t:InternetMessageId>&lt;{id}@example.com&gt;</t:InternetMessageId>\
         <t:IsRead>{read}</t:IsRead></t:Message>",
        from.0
    )
}

/// The same message as `GetItem` describes it: with its sender's address
/// and the people it is to.
pub fn whole(id: &str, subject: &str, from: (&str, &str), received: &str, read: bool) -> String {
    format!(
        "<t:Message><t:ItemId Id=\"{id}\" ChangeKey=\"CQAAABYAAAB1{id}\"/>\
         <t:Subject>{subject}</t:Subject>\
         <t:DateTimeReceived>{received}</t:DateTimeReceived><t:Size>4821</t:Size>\
         <t:DateTimeSent>{received}</t:DateTimeSent>\
         <t:HasAttachments>false</t:HasAttachments><t:IsDraft>false</t:IsDraft>\
         <t:ConversationId Id=\"topic-{id}\"/>\
         <t:Flag><t:FlagStatus>NotFlagged</t:FlagStatus></t:Flag>\
         <t:ToRecipients><t:Mailbox><t:Name>Robin Okafor</t:Name>\
         <t:EmailAddress>robin@example.com</t:EmailAddress>\
         <t:RoutingType>SMTP</t:RoutingType><t:MailboxType>Mailbox</t:MailboxType>\
         </t:Mailbox></t:ToRecipients>\
         <t:From><t:Mailbox><t:Name>{}</t:Name><t:EmailAddress>{}</t:EmailAddress>\
         <t:RoutingType>SMTP</t:RoutingType><t:MailboxType>OneOff</t:MailboxType>\
         </t:Mailbox></t:From>\
         <t:InternetMessageId>&lt;{id}@example.com&gt;</t:InternetMessageId>\
         <t:IsRead>{read}</t:IsRead></t:Message>",
        from.0, from.1
    )
}

/// `GetItem`'s answer about one message.
pub fn got(item: &str) -> String {
    success("GetItem", &format!("<m:Items>{item}</m:Items>"))
}

/// `GetItem`'s answer with a whole message in it, as base64.
pub fn mime(id: &str, raw: &[u8]) -> String {
    got(&format!(
        "<t:Message><t:MimeContent CharacterSet=\"UTF-8\">{}</t:MimeContent>\
         <t:ItemId Id=\"{id}\" ChangeKey=\"CQAAABYAAAB1\"/></t:Message>",
        BASE64.encode(raw)
    ))
}

/// A message of two megabytes with every byte value in it, so that
/// anything done to it on the way shows.
pub fn large_message() -> Vec<u8> {
    let mut raw = b"From: Robin Okafor <robin@example.com>\r\n\
        To: Ana Ruiz <ana.ruiz@example.com>\r\n\
        Subject: Survey data\r\n\
        Message-ID: <survey.data@example.com>\r\n\
        MIME-Version: 1.0\r\n\
        Content-Type: application/octet-stream\r\n\
        Content-Transfer-Encoding: binary\r\n\
        \r\n"
        .to_vec();
    raw.extend((0..2 * 1024 * 1024u32).map(|i| (i.wrapping_mul(31) % 256) as u8));
    raw
}

/// The bytes a request's `<t:MimeContent>` stands for.
pub fn posted_message(request: &Request) -> Vec<u8> {
    let asked = asked(request);
    let start = asked
        .find("<t:MimeContent CharacterSet=\"UTF-8\">")
        .expect("a request with no message in it")
        + "<t:MimeContent CharacterSet=\"UTF-8\">".len();
    let end = asked.find("</t:MimeContent>").unwrap();
    BASE64.decode(&asked[start..end]).unwrap()
}

/// Waits, in real time, until the stand-in has received `count` requests.
pub async fn received(server: &MockServer, count: usize) {
    for _ in 0..1000 {
        if server.received_requests().await.unwrap_or_default().len() >= count {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("the server never received {count} requests");
}
