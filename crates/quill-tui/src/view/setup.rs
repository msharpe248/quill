//! The add-an-account form and the certificate question.

use quill_core::CertificateProblem;
use quill_core::config::TlsMode;
use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Padding};

use crate::command::CommandId;
use crate::input::LineInput;
use crate::setup::{
    Kind, ServerField, Setup, SignInStage, Step, certificate_reason, grouped_fingerprint,
};
use crate::state::{Env, State, UnsubscribeHow, WhenFor};
use crate::text;

/// The width of the longest label and its gap: `Issued to: `.
const LABEL_WIDTH: usize = 11;

/// The same for the form of a Microsoft account: `Application id: `.
const MICROSOFT_LABEL_WIDTH: usize = 16;

pub fn render(frame: &mut Frame, area: Rect, state: &State, env: &Env) {
    // A certificate waiting for an answer is asked about before anything
    // else, the form included.
    if let Some(prompt) = state.certificates.front() {
        let inner = dialog(
            frame,
            area,
            &if prompt.sending {
                format!(" {}: certificate for sending ", prompt.account)
            } else {
                format!(" {}: certificate ", prompt.account)
            },
            env,
        );
        let mut rows = Rows::new(frame, inner, env);
        if prompt.sending {
            rows.text("This is the server the account sends mail through. A message is waiting in the Outbox for it.");
            rows.blank();
        }
        certificate(&mut rows, &prompt.problem, prompt.busy);
        return;
    }
    if let Some(sign_in) = &state.sign_in {
        let title = if sign_in.account.is_empty() {
            " Sign in ".to_owned()
        } else {
            format!(" Sign in to {} ", sign_in.account)
        };
        let inner = dialog(frame, area, &title, env);
        let mut rows = Rows::new(frame, inner, env);
        match &sign_in.stage {
            SignInStage::Starting => rows.text("Getting ready to sign in..."),
            SignInStage::Browser => {
                rows.text("Your browser has been opened at the provider's sign-in page.");
                rows.blank();
                rows.text(
                    "Sign in there. When the page says you can close it, come back: Quill is waiting, and takes it from there.",
                );
                rows.blank();
                rows.dim("Your password goes to the provider, never to Quill. Quill keeps only the sign-in the provider hands back, in the Keychain.");
            }
            SignInStage::Code {
                address,
                code,
                minutes,
            } => {
                rows.text("On any device, open this page and enter the code:");
                rows.blank();
                rows.pair("Page:", address);
                rows.pair("Code:", code);
                rows.blank();
                rows.text("Quill is waiting, and takes it from there.");
                rows.dim(&format!("The code is good for {minutes} minutes."));
            }
        }
        rows.blank();
        rows.dim("esc gives up");
        return;
    }
    if let Some(when) = &state.when {
        let title = match &when.what {
            WhenFor::Snooze(ids) if ids.len() > 1 => {
                format!(" Snooze {} messages until ", ids.len())
            }
            WhenFor::Snooze(_) => " Snooze until ".to_owned(),
            WhenFor::SendLater => " Send at ".to_owned(),
        };
        let inner = dialog(frame, area, &title, env);
        let mut rows = Rows::new(frame, inner, env);
        if let Some(rect) = rows.row() {
            let width = usize::from(rect.width).saturating_sub(6);
            let (shown, column) = when.input.view(width.max(4));
            let frame = &mut *rows.frame;
            frame.render_widget(
                Line::from(vec![
                    Span::styled("When: ", env.styles.dim()),
                    Span::styled(shown, env.styles.text()),
                ]),
                rect,
            );
            frame.set_cursor_position(ratatui::layout::Position::new(
                rect.x + 6 + column as u16,
                rect.y,
            ));
        }
        rows.blank();
        if let Some(problem) = &when.problem {
            rows.warning(problem);
            rows.blank();
        }
        rows.dim(&format!("Such as: {}", crate::when::EXAMPLES));
        match &when.what {
            WhenFor::Snooze(_) => {
                rows.dim("\"now\" brings a snoozed message straight back.");
            }
            WhenFor::SendLater => rows.dim(
                "It waits in the Outbox and goes at that time if Quill is running, or when it next is.",
            ),
        }
        return;
    }
    if let Some(prompt) = &state.unsubscribe {
        let inner = dialog(frame, area, " Unsubscribe ", env);
        let mut rows = Rows::new(frame, inner, env);
        rows.text("This message is from a mailing list.");
        rows.blank();
        rows.pair("From:", &prompt.sender);
        rows.blank();
        // What saying yes does, exactly; then what it does not.
        let (does, does_not, shown, yes) = match &prompt.how {
            UnsubscribeHow::OneClick { url, host } => (
                format!("Quill will tell {host} to unsubscribe you."),
                "One request goes to the address below, which the list gave for this. It says \"unsubscribe\" and nothing else: no cookies, and no page is opened.",
                url,
                "unsubscribe",
            ),
            UnsubscribeHow::Mail { url: _, address } => (
                format!("A message will be written to {address}."),
                "It is not sent for you. Look it over, then send it yourself to unsubscribe, or close it to do nothing.",
                address,
                "write the message",
            ),
            UnsubscribeHow::Page { url, host } => (
                format!("Your browser will open a page at {host}."),
                "This list has no way for Quill to unsubscribe you itself: its page says what to do next. Quill sends nothing.",
                url,
                "open the page",
            ),
        };
        rows.text(&does);
        rows.blank();
        rows.dim(does_not);
        rows.blank();
        rows.pair(
            match prompt.how {
                UnsubscribeHow::Mail { .. } => "To:",
                _ => "Address:",
            },
            shown,
        );
        rows.blank();
        let key = |id| {
            env.hint_key(id)
                .map_or_else(String::new, ToString::to_string)
        };
        let (go, stop) = (key(CommandId::NavOpen), key(CommandId::NavBack));
        rows.keys(&[(go.as_str(), yes), (stop.as_str(), "cancel")]);
        return;
    }
    let Some(setup) = &state.setup else {
        return;
    };
    let title = match setup.step {
        Step::Address => " Add an account ",
        Step::Server if setup.kind == Kind::Microsoft => " Add an account: Microsoft ",
        Step::Server => " Add an account: the server ",
        Step::Password => " Add an account: sign in ",
        Step::Certificate => " Add an account: certificate ",
    };
    let inner = dialog(frame, area, title, env);
    let mut rows = Rows::new(frame, inner, env);
    // While Quill is waiting for an answer there is nowhere to type.
    let typing = setup.busy.is_none();
    match setup.step {
        Step::Address => {
            rows.text(
                "Type the email address of the account. Quill will look for its mail server.",
            );
            rows.blank();
            rows.field("Address:", &setup.address, typing);
        }
        Step::Server if setup.kind == Kind::Microsoft => {
            if let Some(note) = &setup.note {
                rows.dim(note);
                rows.blank();
            }
            microsoft(&mut rows, setup, typing);
        }
        Step::Server => {
            if let Some(note) = &setup.note {
                rows.dim(note);
                rows.blank();
            }
            let at = setup.server_field();
            rows.choice(
                "Type:",
                setup.kind.label(),
                typing && at == ServerField::Type,
            );
            rows.field("Name:", &setup.name, typing && at == ServerField::Name);
            rows.field("Server:", &setup.host, typing && at == ServerField::Host);
            rows.field("Port:", &setup.port, typing && at == ServerField::Port);
            rows.choice(
                "Security:",
                match setup.tls {
                    TlsMode::Implicit => "TLS",
                    TlsMode::Starttls => "STARTTLS",
                    TlsMode::None => "None",
                },
                typing && at == ServerField::Security,
            );
            rows.field(
                "Sign in:",
                &setup.username,
                typing && at == ServerField::Username,
            );
            rows.blank();
            let security = |tls| match tls {
                TlsMode::Implicit => "TLS",
                TlsMode::Starttls => "STARTTLS",
                TlsMode::None => "None",
            };
            rows.field(
                "Send via:",
                &setup.smtp_host,
                typing && at == ServerField::SmtpHost,
            );
            if setup.smtp_host.is_empty() {
                rows.dim("           Empty: the account will read mail and not send it.");
            } else {
                rows.field(
                    "Port:",
                    &setup.smtp_port,
                    typing && at == ServerField::SmtpPort,
                );
                rows.choice(
                    "Security:",
                    security(setup.smtp_tls),
                    typing && at == ServerField::SmtpSecurity,
                );
            }
            if setup.tls == TlsMode::None
                || (setup.smtp_tls == TlsMode::None && !setup.smtp_host.is_empty())
            {
                rows.warning_below(
                    "With no encryption, your password and mail cross the network readable by anyone on the way.",
                );
            }
        }
        Step::Password => {
            rows.text(&format!(
                "The password for {} at {}. It is kept in your Keychain and nowhere else.",
                setup.username.text(),
                setup.host.text()
            ));
            rows.blank();
            rows.field("Password:", &setup.password, typing);
        }
        Step::Certificate => {
            if let Some(problem) = &setup.problem {
                certificate(&mut rows, problem, setup.busy.is_some());
            }
        }
    }
    status(&mut rows, setup);
}

/// The fields of an account that signs in with Microsoft: no server to
/// name, since Microsoft's is known, and the application to sign in as.
fn microsoft(rows: &mut Rows<'_, '_>, setup: &Setup, typing: bool) {
    let at = setup.server_field();
    rows.label = MICROSOFT_LABEL_WIDTH;
    rows.choice(
        "Type:",
        setup.kind.label(),
        typing && at == ServerField::Type,
    );
    rows.field("Name:", &setup.name, typing && at == ServerField::Name);
    rows.pair("Address:", setup.address.text());
    rows.field(
        "Application id:",
        &setup.application,
        typing && at == ServerField::Application,
    );
    rows.field(
        "Tenant:",
        &setup.tenant,
        typing && at == ServerField::Tenant,
    );
    rows.blank();
    rows.dim(
        "Register Quill with Microsoft once to get the application id: see docs/oauth.md. Tenant may be left empty.",
    );
}

fn status(rows: &mut Rows<'_, '_>, setup: &Setup) {
    if let Some(busy) = &setup.busy {
        rows.blank();
        rows.dim(busy);
    } else if let Some(error) = &setup.error {
        rows.warning_below(error);
    }
}

fn dialog(frame: &mut Frame, area: Rect, title: &str, env: &Env) -> Rect {
    let styles = &env.styles;
    let block = Block::bordered()
        .border_type(BorderType::Thick)
        .border_style(styles.border(true))
        .padding(Padding::horizontal(1))
        .title(Line::styled(
            text::truncate(title, usize::from(area.width.saturating_sub(2))),
            styles.border(true),
        ));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    inner
}

/// The certificate the server presented, in its own words, and the three
/// things that can be done about it.
fn certificate(rows: &mut Rows<'_, '_>, problem: &CertificateProblem, busy: bool) {
    rows.text(&format!(
        "{} presented a certificate that was not accepted. Nothing has been sent to it.",
        problem.host
    ));
    rows.warning(&certificate_reason(&problem.reason));
    rows.blank();
    if let Some(details) = &problem.details {
        rows.pair("Issued to:", &details.subject);
        rows.pair("Issued by:", &details.issuer);
        let date = |seconds| text::long_date(seconds, &rows.env.tz);
        rows.pair(
            "Valid:",
            &format!(
                "{} to {}",
                date(details.not_before),
                date(details.not_after)
            ),
        );
    }
    match &problem.fingerprint {
        Some(fingerprint) => {
            let grouped = grouped_fingerprint(fingerprint);
            // Two rows of sixteen pairs, to compare with the server's own.
            let (first, second) = grouped.split_at(grouped.len().min(48));
            rows.pair("SHA-256:", first.trim_end_matches(':'));
            if !second.is_empty() {
                rows.pair("", second);
            }
            rows.blank();
            if busy {
                rows.dim("Working…");
            } else {
                rows.keys(&[
                    ("r", "reject"),
                    ("o", "trust once"),
                    ("p", "always trust this certificate"),
                ]);
            }
        }
        None => {
            rows.blank();
            rows.dim("The certificate could not be read, so it cannot be accepted.");
            rows.keys(&[("r", "reject")]);
        }
    }
}

/// Lays text out a row at a time inside the dialog, and stops at its end.
struct Rows<'f, 'a> {
    frame: &'f mut Frame<'a>,
    area: Rect,
    y: u16,
    env: &'f Env,
    /// How wide the column of labels is.
    label: usize,
}

impl<'f, 'a> Rows<'f, 'a> {
    fn new(frame: &'f mut Frame<'a>, area: Rect, env: &'f Env) -> Self {
        Self {
            frame,
            area,
            y: area.y,
            env,
            label: LABEL_WIDTH,
        }
    }

    /// How many rows are still free.
    fn left(&self) -> usize {
        usize::from((self.area.y + self.area.height).saturating_sub(self.y))
    }

    /// How many rows `words` would take.
    fn lines(&self, words: &str) -> usize {
        text::wrap(&text::sanitize_line(words), usize::from(self.area.width)).len()
    }

    fn row(&mut self) -> Option<Rect> {
        (self.y < self.area.y + self.area.height).then(|| {
            let rect = Rect::new(self.area.x, self.y, self.area.width, 1);
            self.y += 1;
            rect
        })
    }

    fn blank(&mut self) {
        self.row();
    }

    fn wrapped(&mut self, words: &str, style: ratatui::style::Style) {
        let words = text::sanitize_line(words);
        for line in text::wrap(&words, usize::from(self.area.width)) {
            let Some(rect) = self.row() else { return };
            self.frame.render_widget(Line::styled(line, style), rect);
        }
    }

    fn text(&mut self, words: &str) {
        self.wrapped(words, self.env.styles.text());
    }

    fn dim(&mut self, words: &str) {
        self.wrapped(words, self.env.styles.dim());
    }

    fn warning(&mut self, words: &str) {
        self.wrapped(words, self.env.styles.error());
    }

    /// A warning under the fields, after a gap. With the form nearly full
    /// the gap goes first: the words are worth more than the row above
    /// them.
    fn warning_below(&mut self, words: &str) {
        if self.left() > self.lines(words) {
            self.blank();
        }
        self.warning(words);
    }

    /// A label and a value that is only shown.
    fn pair(&mut self, label: &str, value: &str) {
        let Some(rect) = self.row() else { return };
        let room = usize::from(self.area.width).saturating_sub(self.label);
        let line = Line::from(vec![
            Span::styled(text::fit(label, self.label), self.env.styles.dim()),
            Span::styled(
                text::truncate(&text::sanitize_line(value), room),
                self.env.styles.text(),
            ),
        ]);
        self.frame.render_widget(line, rect);
    }

    /// A label and a text field. The focused one holds the cursor.
    fn field(&mut self, label: &str, input: &LineInput, focused: bool) {
        let Some(rect) = self.row() else { return };
        let room = usize::from(self.area.width).saturating_sub(self.label);
        let (shown, column) = input.view(room);
        let label_style = if focused {
            self.env.styles.key()
        } else {
            self.env.styles.dim()
        };
        // Underlined to its full width, so an empty field is still a field.
        let field_style = self.env.styles.text().add_modifier(Modifier::UNDERLINED);
        let line = Line::from(vec![
            Span::styled(text::fit(label, self.label), label_style),
            Span::styled(text::fit(&shown, room), field_style),
        ]);
        self.frame.render_widget(line, rect);
        if focused {
            self.frame
                .set_cursor_position(Position::new(rect.x + (self.label + column) as u16, rect.y));
        }
    }

    /// A label and one of a few values, changed with the arrow keys.
    fn choice(&mut self, label: &str, value: &str, focused: bool) {
        let Some(rect) = self.row() else { return };
        let styles = &self.env.styles;
        let mut spans = vec![Span::styled(
            text::fit(label, self.label),
            if focused { styles.key() } else { styles.dim() },
        )];
        if focused {
            spans.push(Span::styled("‹ ", styles.dim()));
            spans.push(Span::styled(
                value.to_owned(),
                styles.text().add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::styled(" ›  ←/→ to change", styles.dim()));
        } else {
            spans.push(Span::styled(value.to_owned(), styles.text()));
        }
        self.frame.render_widget(Line::from(spans), rect);
    }

    /// The letters that answer a question, with what each does.
    fn keys(&mut self, keys: &[(&str, &str)]) {
        let Some(rect) = self.row() else { return };
        let mut spans = Vec::new();
        for (key, label) in keys {
            spans.push(Span::styled((*key).to_owned(), self.env.styles.key()));
            spans.push(Span::styled(format!(" {label}   "), self.env.styles.dim()));
        }
        self.frame.render_widget(Line::from(spans), rect);
    }
}
