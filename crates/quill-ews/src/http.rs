//! The HTTP side of EWS: one request at a time, signed with the account's
//! password or token, with the waiting and the second tries the server
//! asks for.
//!
//! Four rules are kept here so that nothing above has to remember them.
//! The secret goes to the configured server and nowhere else: no redirect
//! is followed, and EWS hands out no links to follow. A password is sent
//! only over TLS. The secret is never part of an error. And every request
//! has a time limit and every answer a size limit, so that neither a
//! silent server nor an endless one can hang a sync.

use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use quill_core::{Credential, Credentials, Error, Result};
use quill_net::Trust;
use reqwest::header::{
    ACCEPT, AUTHORIZATION, CONTENT_LENGTH, CONTENT_TYPE, HeaderMap, HeaderValue, RETRY_AFTER,
    USER_AGENT, WWW_AUTHENTICATE,
};
use reqwest::{StatusCode, Url};

use crate::request::Request;
use crate::xml::{self, Element, Ns};

/// How long the server may stay silent in the middle of a request before
/// the connection is given up as lost.
const SILENCE: Duration = Duration::from_secs(60);

/// How long one request may take from start to finish, a large message on
/// a slow line included.
pub(crate) const REQUEST_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// The longest wait the server can ask for and get. Asked to wait longer,
/// the request is given up as `Temporary` and whoever called decides when
/// to come back.
const LONGEST_WAIT: Duration = Duration::from_secs(60);

/// How many times a request is sent again after the server said to wait.
const WAITS: u32 = 3;

/// The most of the server's own words put in an error.
const WORDS_LIMIT: usize = 300;

/// The most of one answer that is read: room for a message of well over a
/// hundred megabytes, which arrives as base64 inside XML.
pub(crate) const ANSWER_LIMIT: usize = 256 * 1024 * 1024;

/// What Quill calls itself. An Exchange administrator can allow or block
/// EWS programs by this name, and one that gave none could only be let in
/// by letting everything in.
const AGENT: &str = concat!("Quill/", env!("CARGO_PKG_VERSION"));

/// What every operation's `SOAPAction` begins with.
const ACTIONS: &str = "http://schemas.microsoft.com/exchange/services/2006/messages/";

/// The code of an answer that worked.
const NO_ERROR: &str = "NoError";

/// The code of an answer that says: not now, come back later.
const BUSY: &str = "ErrorServerBusy";

/// Which servers a client may be pointed at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Reach {
    /// A real server: HTTPS and nothing else.
    Https,
    /// A stand-in on this machine, for tests. Plain HTTP is allowed.
    Loopback,
}

/// The ways of signing in that are spoken here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scheme {
    Basic,
    Bearer,
}

impl Scheme {
    fn name(self) -> &'static str {
        match self {
            Self::Basic => "basic",
            Self::Bearer => "bearer",
        }
    }

    /// Why signing in this way is no use against a server that offers only
    /// `offers`. `None` when it offers this way too, or does not say what
    /// it offers.
    fn not_offered(self, offers: &[String]) -> Option<Error> {
        if offers.is_empty() || offers.iter().any(|offer| offer == self.name()) {
            return None;
        }
        let windows_only = offers
            .iter()
            .all(|offer| matches!(offer.as_str(), "ntlm" | "negotiate" | "kerberos"));
        Some(Error::Auth(match (self, windows_only) {
            (Self::Basic, true) => "the Exchange server takes only Windows sign-in (NTLM) for \
                                    Exchange Web Services, which Quill does not speak. Basic \
                                    authentication has to be enabled for EWS on the server before \
                                    this account can be used"
                .to_owned(),
            (Self::Basic, false) => format!(
                "the Exchange server does not take a password for Exchange Web Services the way \
                 Quill sends one (Basic authentication); it offers {}",
                offers.join(", ")
            ),
            (Self::Bearer, _) => format!(
                "the Exchange server does not take an OAuth sign-in for Exchange Web Services; \
                 it offers {}",
                offers.join(", ")
            ),
        }))
    }
}

/// The `Authorization` header for one try, and what must never be
/// repeated from it.
struct Signed {
    scheme: Scheme,
    header: HeaderValue,
    /// The secret, and the form it is sent in.
    secrets: Vec<String>,
}

pub(crate) struct Client {
    http: reqwest::Client,
    url: Url,
    username: String,
    credentials: Arc<dyn Credentials>,
    timeout: Duration,
    limit: usize,
}

impl Client {
    pub fn new(
        url: &str,
        reach: Reach,
        username: &str,
        credentials: Arc<dyn Credentials>,
    ) -> Result<Self> {
        let url = Url::parse(url)
            .ok()
            .filter(|url| !url.cannot_be_a_base() && url.host_str().is_some())
            .ok_or_else(|| Error::Backend(format!("\"{url}\" is not the address of a server")))?;
        if !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(Error::Backend(
                "the Exchange Web Services address must be a plain one: no user name, query or \
                 fragment"
                    .to_owned(),
            ));
        }
        match reach {
            Reach::Https if url.scheme() != "https" => {
                return Err(Error::Insecure(format!(
                    "Exchange Web Services is reached over HTTPS only, and {url} is not"
                )));
            }
            Reach::Loopback if !is_loopback(&url) => {
                return Err(Error::Insecure(format!(
                    "{url} is not on this machine; only a test server on this machine may be reached this way"
                )));
            }
            _ => {}
        }
        // Basic authentication is `name:password`, and has no way of
        // writing a name with a colon in it.
        if username.contains(':') || username.chars().any(char::is_control) {
            return Err(Error::Backend(
                "the user name for Exchange has a colon or a control character in it, and cannot \
                 be sent"
                    .to_owned(),
            ));
        }

        let (tls, _presented) = quill_net::tls::client_config(&Trust::default())?;
        let mut builder = reqwest::Client::builder()
            .tls_backend_preconfigured(Arc::unwrap_or_clone(tls))
            .https_only(reach == Reach::Https)
            // Exchange's web server falls back to HTTP/1.1 wherever Windows
            // sign-in is on offer, which for EWS is nearly everywhere.
            // Nothing is gained by asking for more.
            .http1_only()
            // A redirect would carry the request, and the password with
            // it, to a server nobody chose. EWS never redirects.
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(quill_net::PATIENCE)
            .read_timeout(SILENCE);
        if reach == Reach::Loopback {
            // A proxy from the environment must not come between a test and
            // its server.
            builder = builder.no_proxy();
        }
        let http = builder
            .build()
            .map_err(|err| Error::Backend(format!("cannot set up HTTPS: {err}")))?;
        Ok(Self {
            http,
            url,
            username: username.to_owned(),
            credentials,
            timeout: REQUEST_TIMEOUT,
            limit: ANSWER_LIMIT,
        })
    }

    pub fn set_timeout(&mut self, timeout: Duration) {
        self.timeout = timeout;
    }

    pub fn set_limit(&mut self, limit: usize) {
        self.limit = limit;
    }

    pub fn url(&self) -> &str {
        self.url.as_str()
    }

    /// Sends `request` and has `read` make what it will of the answer to
    /// each thing asked for, in the order asked.
    ///
    /// Two answers are dealt with here and never handed on. A refused
    /// password or token is reported to where it came from and the request
    /// sent once more with what that gives next; a second refusal is
    /// `Error::Auth`. When what it gives next is the same again, as a
    /// password is, the request is not sent again at all: the answer would
    /// be the same, and a server counts wrong passwords and locks the
    /// account out at some number. And when the server says it is busy,
    /// the request
    /// waits as long as it was told to and goes again, a few times over,
    /// before giving up with `Error::Temporary`. Told to wait longer than
    /// a minute, it gives up at once: going again sooner than asked only
    /// makes the server's patience shorter.
    ///
    /// A request as a whole can fail where each thing in it could not:
    /// that is a [`Trouble`], for the caller to put into words.
    pub async fn call<T>(
        &self,
        request: &Request,
        read: impl FnOnce(&[Message<'_>]) -> Result<T> + Send,
    ) -> std::result::Result<T, Trouble> {
        let turned_away =
            || Error::Auth("the Exchange server would not accept the account's sign-in".to_owned());
        // What the server has refused once already.
        let mut refused: Option<HeaderValue> = None;
        let mut waits = 0;
        loop {
            // Asked for every time: where it comes from knows when a token
            // is about to run out, and this does not.
            let signed = self.sign(&self.credentials.get().await?)?;
            if refused.as_ref() == Some(&signed.header) {
                return Err(turned_away().into());
            }
            let Some(reply) = self.attempt(request, &signed).await? else {
                return Err(Trouble::TooLarge(self.limit));
            };
            if reply.status == StatusCode::UNAUTHORIZED {
                if let Some(pointless) = signed.scheme.not_offered(&reply.offers) {
                    return Err(pointless.into());
                }
                if refused.is_some() {
                    return Err(turned_away().into());
                }
                self.credentials.refused().await;
                refused = Some(signed.header);
                continue;
            }

            let (wait, words) = {
                let document = std::str::from_utf8(&reply.body)
                    .map(|text| text.trim_start_matches('\u{feff}'))
                    .map_err(|_| "it is not UTF-8 text".to_owned())
                    .and_then(|text| xml::parse(text).map_err(|err| err.to_string()));
                match Answer::read(&reply, document.as_ref(), &signed.secrets) {
                    Answer::Busy { wait, words } => (wait, words),
                    Answer::Messages(messages) => return read(&messages).map_err(Trouble::Error),
                    Answer::Fault(fault) => return Err(Trouble::Fault(fault)),
                    Answer::Unreadable(why) => return Err(Trouble::Unreadable(why)),
                }
            };
            // Without a time from the server: two seconds, then four, then
            // eight.
            let wait = wait.unwrap_or_else(|| Duration::from_secs(2 << waits.min(4)));
            if waits == WAITS || wait > LONGEST_WAIT {
                return Err(Error::Temporary(format!(
                    "the Exchange server is busy and asked to wait {} seconds: {words}",
                    wait.as_secs()
                ))
                .into());
            }
            waits += 1;
            tokio::time::sleep(wait).await;
        }
    }

    /// The `Authorization` header that `credential` makes.
    fn sign(&self, credential: &Credential) -> Result<Signed> {
        let (scheme, header, secrets) = match credential {
            Credential::Password(password) => {
                if self.username.is_empty() {
                    return Err(Error::Auth(
                        "the account has no user name to sign in to Exchange with".to_owned(),
                    ));
                }
                let pair = BASE64.encode(format!("{}:{password}", self.username));
                (
                    Scheme::Basic,
                    format!("Basic {pair}"),
                    vec![password.clone(), pair],
                )
            }
            Credential::OAuthToken(token) => (
                Scheme::Bearer,
                format!("Bearer {token}"),
                vec![token.clone()],
            ),
        };
        let mut header = HeaderValue::from_str(&header).map_err(|_| {
            Error::Auth("the account's access token is not one that can be sent".to_owned())
        })?;
        header.set_sensitive(true);
        Ok(Signed {
            scheme,
            header,
            secrets,
        })
    }

    /// One try. `None` when the answer is larger than is read.
    async fn attempt(&self, request: &Request, signed: &Signed) -> Result<Option<Reply>> {
        let mut response = self
            .http
            .post(self.url.clone())
            .timeout(self.timeout)
            .header(AUTHORIZATION, signed.header.clone())
            .header(CONTENT_TYPE, "text/xml; charset=utf-8")
            .header(ACCEPT, "text/xml")
            .header(USER_AGENT, AGENT)
            .header("SOAPAction", format!("\"{ACTIONS}{}\"", request.operation))
            .body(request.body.clone())
            .send()
            .await
            .map_err(unreachable)?;
        let status = response.status();
        let wait = retry_after(response.headers());
        let offers = offers(response.headers());
        let announced = response
            .headers()
            .get(CONTENT_LENGTH)
            .and_then(|length| length.to_str().ok())
            .and_then(|length| length.trim().parse::<usize>().ok());
        if announced.is_some_and(|length| length > self.limit) {
            return Ok(None);
        }
        // Room is made as the answer comes, not on the server's word for
        // how much is coming.
        let mut body = Vec::with_capacity(announced.unwrap_or(0).min(1024 * 1024));
        while let Some(chunk) = response.chunk().await.map_err(unreachable)? {
            if body.len() + chunk.len() > self.limit {
                return Ok(None);
            }
            body.extend_from_slice(&chunk);
        }
        Ok(Some(Reply {
            status,
            wait,
            offers,
            body,
        }))
    }
}

/// Whether `url` names this machine by address. A name such as `localhost`
/// is not taken on trust: what it resolves to is somebody else's decision.
fn is_loopback(url: &Url) -> bool {
    url.host_str()
        .and_then(|host| {
            host.trim_start_matches('[')
                .trim_end_matches(']')
                .parse::<IpAddr>()
                .ok()
        })
        .is_some_and(|address| address.is_loopback())
}

/// How long a `Retry-After` header says to wait, when it says it in
/// seconds.
fn retry_after(headers: &HeaderMap) -> Option<Duration> {
    let seconds = headers
        .get(RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse()
        .ok()?;
    Some(Duration::from_secs(seconds))
}

/// The ways of signing in a refusal says the server would take, in lower
/// case: `basic`, `ntlm`, `negotiate`, `bearer`.
///
/// IIS sends one `WWW-Authenticate` header for each. Several in one
/// header, with commas between, are told apart from a challenge's own
/// parameters by being names that are known.
fn offers(headers: &HeaderMap) -> Vec<String> {
    const KNOWN: [&str; 6] = ["basic", "bearer", "digest", "kerberos", "negotiate", "ntlm"];
    let mut offers: Vec<String> = Vec::new();
    for value in headers.get_all(WWW_AUTHENTICATE) {
        let Ok(value) = value.to_str() else { continue };
        for part in value.split(',') {
            let Some(word) = part.split_whitespace().next() else {
                continue;
            };
            let word = word.to_ascii_lowercase();
            if KNOWN.contains(&word.as_str()) && !offers.contains(&word) {
                offers.push(word);
            }
        }
    }
    offers
}

/// A request that could not be made or finished. The address is left out:
/// it is no help to the person reading.
fn unreachable(err: reqwest::Error) -> Error {
    if err.is_timeout() {
        return Error::Connection("the Exchange server did not answer in time".to_owned());
    }
    let refused_here = err.is_builder();
    let err = err.without_url();
    let mut text = err.to_string();
    let mut source = std::error::Error::source(&err);
    while let Some(cause) = source {
        let cause_text = cause.to_string();
        if !text.contains(&cause_text) {
            text.push_str(": ");
            text.push_str(&cause_text);
        }
        source = cause.source();
    }
    if refused_here {
        // Never sent: the address was not one this client will go to.
        Error::Backend(text)
    } else {
        Error::Connection(text)
    }
}

/// The server's words made fit for an error people read: short, free of
/// control characters, and with the secret struck out should a server
/// ever echo it.
fn tidy(text: &str, secrets: &[String]) -> String {
    let mut text = text.to_owned();
    for secret in secrets {
        if !secret.is_empty() {
            text = text.replace(secret.as_str(), "[secret]");
        }
    }
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(WORDS_LIMIT)
        .collect::<String>()
        .trim()
        .to_owned()
}

/// A code and the words that came with it, as one phrase.
fn words(code: &str, text: &str, status: Option<StatusCode>) -> String {
    match (text.is_empty(), code.is_empty(), status) {
        (false, false, _) => format!("{text} ({code})"),
        (false, true, _) => text.to_owned(),
        (true, false, Some(status)) => format!("{code} (HTTP {})", status.as_u16()),
        (true, false, None) => code.to_owned(),
        (true, true, Some(status)) => format!("HTTP {status}"),
        (true, true, None) => "it did not say why".to_owned(),
    }
}

/// What kind of failure one of EWS's error codes is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    /// The message or folder asked about is not there.
    Missing,
    /// The server's own trouble, which passes.
    Passing,
    /// The account itself may not sign in.
    SignIn,
    /// Anything else: the request was understood and turned down.
    Refusal,
}

impl Kind {
    pub fn of(code: &str) -> Self {
        match code {
            "ErrorItemNotFound"
            | "ErrorFolderNotFound"
            | "ErrorSyncFolderNotFound"
            | "ErrorToFolderNotFound"
            | "ErrorParentFolderNotFound" => Self::Missing,
            BUSY
            | "ErrorInternalServerError"
            | "ErrorInternalServerTransientError"
            | "ErrorTimeoutExpired"
            | "ErrorConnectionFailed"
            | "ErrorMailboxStoreUnavailable"
            | "ErrorMailboxMoveInProgress"
            | "ErrorMailboxFailover"
            | "ErrorADUnavailable"
            | "ErrorInsufficientResources"
            | "ErrorExceededConnectionCount"
            | "ErrorExceededFindCountLimit"
            | "ErrorTooManyObjectsOpened"
            // A SOAP fault that blames the server and says no more.
            | "Server"
            | "Receiver" => Self::Passing,
            "ErrorPasswordExpired" | "ErrorPasswordChangeRequired" | "ErrorAccountDisabled" => {
                Self::SignIn
            }
            // "Access denied" is here too, and is not `Error::Auth`: the
            // sign-in was accepted, and what it may not do can be this one
            // message as well as the whole mailbox. The sync engine holds
            // an account's whole queue back on `Auth`, which one message
            // that may not be touched must not be able to do.
            _ => Self::Refusal,
        }
    }

    fn error(self, text: String) -> Error {
        match self {
            Self::Missing => Error::NotFound(text),
            Self::Passing => Error::Temporary(text),
            Self::SignIn => Error::Auth(text),
            Self::Refusal => Error::Backend(text),
        }
    }
}

/// The raw answer to one try.
struct Reply {
    status: StatusCode,
    wait: Option<Duration>,
    offers: Vec<String>,
    body: Vec<u8>,
}

/// What an answer amounts to, once read.
enum Answer<'a> {
    /// An answer to each thing asked for.
    Messages(Vec<Message<'a>>),
    /// Nothing was done, and the request is welcome again after a wait.
    Busy {
        wait: Option<Duration>,
        words: String,
    },
    /// The request as a whole was turned down.
    Fault(Fault),
    /// The server said all was well in a way that cannot be read.
    Unreadable(String),
}

impl<'a> Answer<'a> {
    fn read(
        reply: &Reply,
        document: std::result::Result<&'a Element<'a>, &String>,
        secrets: &[String],
    ) -> Self {
        let body = document
            .ok()
            .filter(|envelope| envelope.is(Ns::Soap, "Envelope"))
            .and_then(|envelope| envelope.child(Ns::Soap, "Body"));
        if let Some(fault) = body.and_then(|body| body.child(Ns::Soap, "Fault")) {
            let detail = fault
                .child_called("detail")
                .or_else(|| fault.child_called("Detail"));
            let code = detail
                .and_then(|detail| detail.child_text(Ns::Errors, "ResponseCode"))
                .or_else(|| {
                    // `a:ErrorSchemaValidation`, `s:Client`.
                    fault
                        .child_called("faultcode")
                        .map(|code| code.text().rsplit(':').next().unwrap_or_default())
                })
                .unwrap_or_default();
            let text = detail
                .and_then(|detail| detail.child_text(Ns::Errors, "Message"))
                .or_else(|| fault.child_called("faultstring").map(Element::text))
                .unwrap_or_default();
            let fault_read = Fault {
                status: reply.status,
                code: tidy(code, secrets),
                text: tidy(text, secrets),
            };
            if fault_read.code == BUSY {
                return Self::Busy {
                    wait: back_off(fault).or(reply.wait),
                    words: fault_read.words(),
                };
            }
            return Self::Fault(fault_read);
        }

        let bare = Fault {
            status: reply.status,
            code: String::new(),
            text: String::new(),
        };
        // Both say the request was turned away untouched, so even a
        // message being sent can go again without going twice. Other
        // failures of the server do not say that, and are left to the
        // caller.
        if matches!(
            reply.status,
            StatusCode::TOO_MANY_REQUESTS | StatusCode::SERVICE_UNAVAILABLE
        ) {
            return Self::Busy {
                wait: reply.wait,
                words: bare.words(),
            };
        }
        if !reply.status.is_success() {
            return Self::Fault(bare);
        }

        let document = match document {
            Ok(document) => document,
            Err(why) => return Self::Unreadable(why.clone()),
        };
        let listed = body
            .and_then(|body| {
                body.children()
                    .iter()
                    .find(|child| child.name().ends_with("Response"))
            })
            .and_then(|response| response.child(Ns::Messages, "ResponseMessages"));
        let Some(listed) = listed else {
            return Self::Unreadable(format!(
                "it is a <{}> with no answer from Exchange Web Services in it",
                tidy(document.name(), secrets)
            ));
        };
        let messages: Vec<Message<'a>> = listed
            .children()
            .iter()
            .map(|element| Message::read(element, secrets))
            .collect();
        if !messages.is_empty() && messages.iter().all(|message| message.code == BUSY) {
            return Self::Busy {
                wait: messages[0].wait,
                words: messages[0].words(),
            };
        }
        Self::Messages(messages)
    }
}

/// How long Exchange says to stay away, when it says so: the number of
/// milliseconds it calls `BackOffMilliseconds`, somewhere inside.
fn back_off(element: &Element<'_>) -> Option<Duration> {
    let value = element.find(&|inner| {
        inner.name() == "Value" && inner.attribute("Name") == Some("BackOffMilliseconds")
    })?;
    value.text().parse().ok().map(Duration::from_millis)
}

/// The answer to one of the things a request asked for.
pub(crate) struct Message<'a> {
    /// `NoError` when it worked, otherwise why not.
    pub code: String,
    text: String,
    wait: Option<Duration>,
    /// What was asked for, when it worked.
    pub body: &'a Element<'a>,
}

impl<'a> Message<'a> {
    fn read(element: &'a Element<'a>, secrets: &[String]) -> Self {
        let code = match element.child_text(Ns::Messages, "ResponseCode") {
            Some(code) => tidy(code, secrets),
            // A success need not repeat itself. Anything else must say
            // what it is, and is a failure whatever it meant to say.
            None if element.attribute("ResponseClass") == Some("Success") => NO_ERROR.to_owned(),
            None => String::new(),
        };
        Self {
            code,
            text: tidy(
                element
                    .child_text(Ns::Messages, "MessageText")
                    .unwrap_or_default(),
                secrets,
            ),
            wait: back_off(element),
            body: element,
        }
    }

    pub fn ok(&self) -> bool {
        self.code == NO_ERROR
    }

    pub fn kind(&self) -> Kind {
        Kind::of(&self.code)
    }

    /// The message this was about is not on the server. Whether that is an
    /// error depends on what was being done: deleting a message that is
    /// already gone is not one.
    pub fn item_missing(&self) -> bool {
        self.code == "ErrorItemNotFound"
    }

    /// What the server said went wrong.
    pub fn words(&self) -> String {
        words(&self.code, &self.text, None)
    }

    /// Why `doing` failed, as the kind of error the sync engine acts on.
    pub fn error(&self, doing: &str) -> Error {
        self.kind()
            .error(format!("Exchange could not {doing}: {}", self.words()))
    }

    /// What was asked for, when it worked; otherwise why `doing` did not.
    pub fn done(&self, doing: &str) -> Result<&'a Element<'a>> {
        if self.ok() {
            Ok(self.body)
        } else {
            Err(self.error(doing))
        }
    }
}

/// The server's refusal of a whole request: a SOAP fault, or an HTTP
/// status with nothing of EWS in it.
#[derive(Debug)]
pub(crate) struct Fault {
    pub status: StatusCode,
    /// EWS's code for it, or the fault's own, or nothing.
    pub code: String,
    text: String,
}

impl Fault {
    pub fn words(&self) -> String {
        words(&self.code, &self.text, Some(self.status))
    }

    /// What kind of failure it is. Without a code there is the status to
    /// go by: the server's own trouble passes, and nothing else does. In
    /// particular an address that is "not found" is a wrong address, and
    /// not a message that has gone.
    pub fn kind(&self) -> Kind {
        match self.code.as_str() {
            "" if self.status.is_server_error() => Kind::Passing,
            "" => Kind::Refusal,
            code => Kind::of(code),
        }
    }
}

/// A request that did not come back with an answer to each thing asked.
#[derive(Debug)]
pub(crate) enum Trouble {
    /// It could not be sent, or signed, or the server stayed busy: already
    /// the error to report.
    Error(Error),
    /// The server turned the whole request down.
    Fault(Fault),
    /// The server said all was well, in an answer that cannot be read.
    Unreadable(String),
    /// The answer was larger than this many bytes, and was not read.
    TooLarge(usize),
}

impl From<Error> for Trouble {
    fn from(err: Error) -> Self {
        Self::Error(err)
    }
}

impl Trouble {
    /// Why `doing` failed, as the kind of error the sync engine acts on.
    pub fn error(self, doing: &str) -> Error {
        match self {
            Self::Error(err) => err,
            Self::Fault(fault) => fault
                .kind()
                .error(format!("Exchange could not {doing}: {}", fault.words())),
            Self::Unreadable(why) => Error::Backend(format!(
                "Exchange's answer could not be read while trying to {doing}: {why}"
            )),
            Self::TooLarge(limit) => Error::Backend(format!(
                "Exchange's answer was larger than the {limit} bytes Quill reads while trying to \
                 {doing}"
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client(url: &str, reach: Reach) -> Result<Client> {
        Client::new(
            url,
            reach,
            "robin@example.com",
            quill_core::credentials::password("test-password"),
        )
    }

    #[test]
    fn a_real_server_is_https_or_nothing() {
        assert!(client("https://mail.example.com/EWS/Exchange.asmx", Reach::Https).is_ok());
        for plain in [
            "http://mail.example.com/EWS/Exchange.asmx",
            "http://127.0.0.1:8080/EWS/Exchange.asmx",
        ] {
            assert!(
                matches!(client(plain, Reach::Https), Err(Error::Insecure(_))),
                "{plain}"
            );
        }
    }

    #[test]
    fn only_this_machine_is_reached_as_a_test_server() {
        assert!(client("http://127.0.0.1:8080/EWS/Exchange.asmx", Reach::Loopback).is_ok());
        assert!(client("http://[::1]:8080/EWS/Exchange.asmx", Reach::Loopback).is_ok());
        for elsewhere in [
            "http://mail.example.com/EWS/Exchange.asmx",
            "https://mail.example.com/EWS/Exchange.asmx",
            "http://localhost:8080/EWS/Exchange.asmx",
            "http://192.0.2.7/EWS/Exchange.asmx",
        ] {
            assert!(
                matches!(client(elsewhere, Reach::Loopback), Err(Error::Insecure(_))),
                "{elsewhere}"
            );
        }
    }

    #[test]
    fn what_is_not_a_plain_address_is_refused() {
        for bad in [
            "",
            "mail.example.com",
            "mailto:robin@example.com",
            "https://robin:secret@mail.example.com/EWS/Exchange.asmx",
            "https://mail.example.com/EWS/Exchange.asmx?x=1",
            "https://mail.example.com/EWS/Exchange.asmx#top",
        ] {
            assert!(
                matches!(client(bad, Reach::Https), Err(Error::Backend(_))),
                "{bad}"
            );
        }
    }

    #[test]
    fn a_user_name_that_basic_cannot_carry_is_refused() {
        let with = |username: &str| {
            Client::new(
                "https://mail.example.com/EWS/Exchange.asmx",
                Reach::Https,
                username,
                quill_core::credentials::password("test-password"),
            )
        };
        assert!(with("EXAMPLE\\robin").is_ok());
        assert!(with("robin@example.com").is_ok());
        // None at all is fine until a password has to be sent with it.
        assert!(with("").is_ok());
        assert!(matches!(with("robin:okafor"), Err(Error::Backend(_))));
        assert!(matches!(
            with("robin\r\nX-Injected: 1"),
            Err(Error::Backend(_))
        ));
    }

    #[test]
    fn a_password_is_signed_as_basic_and_a_token_as_bearer() {
        let client = client("https://mail.example.com/EWS/Exchange.asmx", Reach::Https).unwrap();
        let signed = client
            .sign(&Credential::Password("pässword:1".to_owned()))
            .unwrap();
        assert_eq!(signed.scheme, Scheme::Basic);
        assert!(signed.header.is_sensitive());
        let pair = BASE64.encode("robin@example.com:pässword:1");
        assert_eq!(signed.header.to_str().unwrap(), format!("Basic {pair}"));
        assert_eq!(signed.secrets, ["pässword:1".to_owned(), pair]);

        let signed = client
            .sign(&Credential::OAuthToken("test-access-token-1".to_owned()))
            .unwrap();
        assert_eq!(signed.scheme, Scheme::Bearer);
        assert_eq!(
            signed.header.to_str().unwrap(),
            "Bearer test-access-token-1"
        );

        // Not one that can be put in a header, and not put in an error.
        let err = client
            .sign(&Credential::OAuthToken("line\r\nbreak".to_owned()))
            .err()
            .unwrap();
        assert!(matches!(err, Error::Auth(_)));
        assert!(!err.to_string().contains("break"));
    }

    #[test]
    fn a_password_without_a_user_name_is_not_sent() {
        let client = Client::new(
            "https://mail.example.com/EWS/Exchange.asmx",
            Reach::Https,
            "",
            quill_core::credentials::password("test-password"),
        )
        .unwrap();
        assert!(matches!(
            client.sign(&Credential::Password("test-password".to_owned())),
            Err(Error::Auth(_))
        ));
        assert!(
            client
                .sign(&Credential::OAuthToken("test-access-token-1".to_owned()))
                .is_ok()
        );
    }

    fn offered(values: &[&str]) -> Vec<String> {
        let mut headers = HeaderMap::new();
        for value in values {
            headers.append(WWW_AUTHENTICATE, value.parse().unwrap());
        }
        offers(&headers)
    }

    #[test]
    fn what_a_refusal_offers_is_read_from_its_headers() {
        assert_eq!(
            offered(&["Negotiate", "NTLM", "Basic realm=\"mail.example.com\""]),
            ["negotiate", "ntlm", "basic"]
        );
        assert_eq!(
            offered(&[
                "Bearer client_id=\"00000002-0000-0ff1-ce00-000000000000\", \
                 trusted_issuers=\"00000001-0000-0000-c000-000000000000@*\", \
                 token_types=\"app_asserted_user_v1 service_asserted_app_v1\"",
                "Basic Realm=\"\""
            ]),
            ["bearer", "basic"]
        );
        assert_eq!(offered(&["negotiate, ntlm, NTLM"]), ["negotiate", "ntlm"]);
        assert!(offered(&[]).is_empty());
        assert!(offered(&["", "Unheard-Of abc"]).is_empty());
    }

    #[test]
    fn a_server_that_wants_ntlm_is_told_apart_from_a_wrong_password() {
        let wants = |scheme: Scheme, offers: &[&str]| {
            let offers: Vec<String> = offers.iter().map(|offer| (*offer).to_owned()).collect();
            scheme.not_offered(&offers).map(|err| err.to_string())
        };
        // Offered, or nothing said: the secret itself was refused.
        assert_eq!(wants(Scheme::Basic, &["negotiate", "ntlm", "basic"]), None);
        assert_eq!(wants(Scheme::Basic, &[]), None);
        assert_eq!(wants(Scheme::Bearer, &["bearer"]), None);

        let ntlm = wants(Scheme::Basic, &["negotiate", "ntlm"]).unwrap();
        assert!(ntlm.contains("NTLM") && ntlm.contains("Basic authentication has to be enabled"));
        let oauth = wants(Scheme::Basic, &["bearer"]).unwrap();
        assert!(oauth.contains("it offers bearer"), "{oauth}");
        let password = wants(Scheme::Bearer, &["negotiate", "ntlm", "basic"]).unwrap();
        assert!(
            password.contains("does not take an OAuth sign-in"),
            "{password}"
        );
    }

    #[test]
    fn retry_after_is_read_in_seconds() {
        let with = |value: &str| {
            let mut headers = HeaderMap::new();
            headers.insert(RETRY_AFTER, value.parse().unwrap());
            retry_after(&headers)
        };
        assert_eq!(with("2"), Some(Duration::from_secs(2)));
        assert_eq!(with(" 30 "), Some(Duration::from_secs(30)));
        assert_eq!(with("Wed, 21 Oct 2026 07:28:00 GMT"), None);
        assert_eq!(with("-1"), None);
        assert_eq!(retry_after(&HeaderMap::new()), None);
    }

    #[test]
    fn the_servers_words_are_short_clean_and_never_hold_the_secret() {
        let secrets = ["test-password-1".to_owned(), "cm9iaW46dGVzdA==".to_owned()];
        let text = format!(
            "Bad password test-password-1 in cm9iaW46dGVzdA==\u{1b}[31m {}",
            "x".repeat(1000)
        );
        let tidied = tidy(&text, &secrets);
        assert!(tidied.starts_with("Bad password [secret] in [secret] [31m x"));
        assert_eq!(tidied.chars().count(), WORDS_LIMIT);
        // An empty secret strikes nothing out.
        assert_eq!(tidy(" plain ", &[String::new()]), "plain");
    }

    #[test]
    fn a_code_and_its_words_are_one_phrase() {
        let status = Some(StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(
            words("ErrorItemNotFound", "Not there.", status),
            "Not there. (ErrorItemNotFound)"
        );
        assert_eq!(words("", "Not there.", None), "Not there.");
        assert_eq!(
            words("ErrorItemNotFound", "", status),
            "ErrorItemNotFound (HTTP 500)"
        );
        assert_eq!(words("ErrorItemNotFound", "", None), "ErrorItemNotFound");
        assert_eq!(words("", "", status), "HTTP 500 Internal Server Error");
        assert_eq!(words("", "", None), "it did not say why");
    }

    #[test]
    fn each_code_is_the_error_the_sync_engine_expects() {
        assert_eq!(Kind::of("ErrorItemNotFound"), Kind::Missing);
        assert_eq!(Kind::of("ErrorFolderNotFound"), Kind::Missing);
        assert_eq!(Kind::of("ErrorServerBusy"), Kind::Passing);
        assert_eq!(Kind::of("ErrorInternalServerError"), Kind::Passing);
        assert_eq!(Kind::of("ErrorPasswordExpired"), Kind::SignIn);
        // The sign-in worked; what it may do is another matter.
        assert_eq!(Kind::of("ErrorAccessDenied"), Kind::Refusal);
        assert_eq!(Kind::of("ErrorSchemaValidation"), Kind::Refusal);
        assert_eq!(Kind::of("Client"), Kind::Refusal);
        assert_eq!(Kind::of(""), Kind::Refusal);

        let bare = |status: u16| Fault {
            status: StatusCode::from_u16(status).unwrap(),
            code: String::new(),
            text: String::new(),
        };
        assert_eq!(bare(502).kind(), Kind::Passing);
        // A wrong address, not a message that has gone.
        assert_eq!(bare(404).kind(), Kind::Refusal);
        assert_eq!(bare(403).kind(), Kind::Refusal);
    }

    fn reply(status: u16, body: &str) -> Reply {
        Reply {
            status: StatusCode::from_u16(status).unwrap(),
            wait: None,
            offers: Vec::new(),
            body: body.as_bytes().to_vec(),
        }
    }

    /// What `reply` amounts to, in a word, with what it carries.
    fn read(reply: &Reply) -> String {
        let text = std::str::from_utf8(&reply.body).unwrap();
        let document = xml::parse(text).map_err(|err| err.to_string());
        match Answer::read(reply, document.as_ref(), &[]) {
            Answer::Messages(messages) => format!(
                "messages: {}",
                messages
                    .iter()
                    .map(|message| message.code.as_str())
                    .collect::<Vec<_>>()
                    .join(" ")
            ),
            Answer::Busy { wait, words } => format!("busy {wait:?}: {words}"),
            Answer::Fault(fault) => format!("fault: {}", fault.words()),
            Answer::Unreadable(why) => format!("unreadable: {why}"),
        }
    }

    const BUSY_FAULT: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/">
  <s:Body>
    <s:Fault>
      <faultcode xmlns:a="http://schemas.microsoft.com/exchange/services/2006/types">a:ErrorServerBusy</faultcode>
      <faultstring xml:lang="en-US">The server cannot service this request right now. Try again later.</faultstring>
      <detail>
        <e:ResponseCode xmlns:e="http://schemas.microsoft.com/exchange/services/2006/errors">ErrorServerBusy</e:ResponseCode>
        <e:Message xmlns:e="http://schemas.microsoft.com/exchange/services/2006/errors">The server cannot service this request right now. Try again later.</e:Message>
        <t:MessageXml xmlns:t="http://schemas.microsoft.com/exchange/services/2006/types">
          <t:Value Name="BackOffMilliseconds">1500</t:Value>
        </t:MessageXml>
      </detail>
    </s:Fault>
  </s:Body>
</s:Envelope>"#;

    #[test]
    fn a_busy_fault_says_how_long_to_stay_away() {
        assert_eq!(
            read(&reply(500, BUSY_FAULT)),
            "busy Some(1.5s): The server cannot service this request right now. Try again later. \
             (ErrorServerBusy)"
        );
        // The header's word counts when the fault has none of its own.
        let mut without = reply(500, &BUSY_FAULT.replace("BackOffMilliseconds", "Other"));
        without.wait = Some(Duration::from_secs(7));
        assert!(read(&without).starts_with("busy Some(7s)"));
    }

    #[test]
    fn a_fault_is_known_by_its_detail_or_else_by_its_code() {
        let fault = |inside: &str| {
            format!(
                "<soap:Envelope xmlns:soap=\"http://schemas.xmlsoap.org/soap/envelope/\">\
                 <soap:Body><soap:Fault>{inside}</soap:Fault></soap:Body></soap:Envelope>"
            )
        };
        assert_eq!(
            read(&reply(
                500,
                &fault(
                    "<faultcode xmlns:a=\"http://schemas.microsoft.com/exchange/services/2006/types\">a:ErrorSchemaValidation</faultcode>\
                     <faultstring>The request failed schema validation.</faultstring>"
                )
            )),
            "fault: The request failed schema validation. (ErrorSchemaValidation)"
        );
        assert_eq!(
            read(&reply(500, &fault("<faultcode>soap:Server</faultcode>"))),
            "fault: Server (HTTP 500)"
        );
        assert_eq!(
            read(&reply(500, &fault(""))),
            "fault: HTTP 500 Internal Server Error"
        );
        // A fault is a fault whatever the status says.
        assert_eq!(
            read(&reply(200, &fault("<faultstring>No.</faultstring>"))),
            "fault: No."
        );
    }

    #[test]
    fn a_failure_without_soap_is_known_by_its_status() {
        assert_eq!(
            read(&reply(502, "<html>Bad Gateway</html>")),
            "fault: HTTP 502 Bad Gateway"
        );
        assert_eq!(read(&reply(404, "")), "fault: HTTP 404 Not Found");
        assert_eq!(
            read(&reply(503, "Service Unavailable")),
            "busy None: HTTP 503 Service Unavailable"
        );
        assert_eq!(
            read(&reply(429, "")),
            "busy None: HTTP 429 Too Many Requests"
        );
    }

    fn answer(messages: &str) -> String {
        format!(
            "<s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\"><s:Body>\
             <m:GetItemResponse xmlns:m=\"http://schemas.microsoft.com/exchange/services/2006/messages\" \
             xmlns:t=\"http://schemas.microsoft.com/exchange/services/2006/types\">\
             <m:ResponseMessages>{messages}</m:ResponseMessages></m:GetItemResponse>\
             </s:Body></s:Envelope>"
        )
    }

    const WORKED: &str = "<m:GetItemResponseMessage ResponseClass=\"Success\">\
        <m:ResponseCode>NoError</m:ResponseCode><m:Items/></m:GetItemResponseMessage>";
    const GONE: &str = "<m:GetItemResponseMessage ResponseClass=\"Error\">\
        <m:MessageText>The specified object was not found in the store.</m:MessageText>\
        <m:ResponseCode>ErrorItemNotFound</m:ResponseCode>\
        <m:DescriptiveLinkKey>0</m:DescriptiveLinkKey><m:Items/></m:GetItemResponseMessage>";
    const THROTTLED: &str = "<m:GetItemResponseMessage ResponseClass=\"Error\">\
        <m:MessageText>The server cannot service this request right now.</m:MessageText>\
        <m:ResponseCode>ErrorServerBusy</m:ResponseCode>\
        <m:MessageXml><t:Value Name=\"BackOffMilliseconds\">250</t:Value></m:MessageXml>\
        </m:GetItemResponseMessage>";

    #[test]
    fn each_thing_asked_for_has_its_own_answer() {
        assert_eq!(
            read(&reply(200, &answer(&format!("{WORKED}{GONE}{WORKED}")))),
            "messages: NoError ErrorItemNotFound NoError"
        );
        assert_eq!(read(&reply(200, &answer(""))), "messages: ");
        // A success that does not repeat itself, and a failure that does
        // not say what it is.
        assert_eq!(
            read(&reply(
                200,
                &answer(
                    "<m:GetItemResponseMessage ResponseClass=\"Success\"/>\
                     <m:GetItemResponseMessage ResponseClass=\"Error\"/>\
                     <m:GetItemResponseMessage/>"
                )
            )),
            "messages: NoError  "
        );
    }

    #[test]
    fn busy_through_and_through_is_busy_and_busy_in_part_is_not() {
        assert_eq!(
            read(&reply(200, &answer(&format!("{THROTTLED}{THROTTLED}")))),
            "busy Some(250ms): The server cannot service this request right now. (ErrorServerBusy)"
        );
        // Something was done. Asking for all of it again is the caller's
        // decision, not this one's.
        assert_eq!(
            read(&reply(200, &answer(&format!("{WORKED}{THROTTLED}")))),
            "messages: NoError ErrorServerBusy"
        );
    }

    #[test]
    fn a_success_that_is_not_an_answer_from_ews_cannot_be_read() {
        for (body, why) in [
            ("<html><body>Sign in</body></html>", "<html>"),
            ("", "unreadable"),
            ("{\"ok\": true}", "unreadable"),
            (
                "<s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\"><s:Body/></s:Envelope>",
                "<Envelope>",
            ),
            (
                "<s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\"><s:Body>\
                 <GetItemResponse><ResponseMessages/></GetItemResponse></s:Body></s:Envelope>",
                "<Envelope>",
            ),
        ] {
            let read = read(&reply(200, body));
            assert!(read.starts_with("unreadable: "), "{body}: {read}");
            assert!(read.contains(why), "{body}: {read}");
        }
    }

    #[test]
    fn a_message_is_the_error_its_code_says() {
        let text = answer(&format!("{WORKED}{GONE}{THROTTLED}"));
        let document = xml::parse(&text).unwrap();
        let reply = reply(200, &text);
        let Answer::Messages(messages) = Answer::read(&reply, Ok(&document), &[]) else {
            panic!("not messages");
        };
        assert!(messages[0].ok() && messages[0].done("read the message").is_ok());
        assert!(messages[1].item_missing());
        let err = messages[1].done("read the message").err().unwrap();
        assert!(matches!(err, Error::NotFound(_)), "{err:?}");
        assert_eq!(
            err.to_string(),
            "not found: Exchange could not read the message: The specified object was not found \
             in the store. (ErrorItemNotFound)"
        );
        assert!(matches!(
            messages[2].error("read the message"),
            Error::Temporary(_)
        ));
    }

    #[test]
    fn trouble_is_put_into_the_error_the_sync_engine_acts_on() {
        let fault = |status: u16, code: &str| {
            Trouble::Fault(Fault {
                status: StatusCode::from_u16(status).unwrap(),
                code: code.to_owned(),
                text: String::new(),
            })
        };
        assert!(matches!(
            fault(500, "ErrorSchemaValidation").error("list the folders"),
            Error::Backend(_)
        ));
        assert!(matches!(
            fault(500, "ErrorInternalServerError").error("list the folders"),
            Error::Temporary(_)
        ));
        assert!(matches!(
            fault(500, "").error("list the folders"),
            Error::Temporary(_)
        ));
        assert!(matches!(
            fault(404, "").error("list the folders"),
            Error::Backend(_)
        ));
        assert!(matches!(
            fault(500, "ErrorFolderNotFound").error("list the folders"),
            Error::NotFound(_)
        ));
        assert!(matches!(
            Trouble::Unreadable("it is not UTF-8 text".to_owned()).error("list the folders"),
            Error::Backend(_)
        ));
        let err = Trouble::TooLarge(1024).error("download the message");
        assert!(err.to_string().contains("1024 bytes"), "{err}");
        assert!(matches!(
            Trouble::from(Error::Connection("gone".to_owned())).error("list the folders"),
            Error::Connection(_)
        ));
    }
}
