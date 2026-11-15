//! The add-an-account form and the certificate question.

use quill_core::CertificateProblem;
use quill_core::config::TlsMode;
use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Padding};

use crate::input::LineInput;
use crate::setup::{ServerField, Setup, Step, certificate_reason, grouped_fingerprint};
use crate::state::{Env, State};
use crate::text;

/// The width of the longest label and its gap: `Issued to: `.
const LABEL_WIDTH: usize = 11;

pub fn render(frame: &mut Frame, area: Rect, state: &State, env: &Env) {
    // A certificate waiting for an answer is asked about before anything
    // else, the form included.
    if let Some(prompt) = state.certificates.front() {
        let inner = dialog(
            frame,
            area,
            &format!(" {}: certificate ", prompt.account),
            env,
        );
        let mut rows = Rows::new(frame, inner, env);
        certificate(&mut rows, &prompt.problem, prompt.busy);
        return;
    }
    let Some(setup) = &state.setup else {
        return;
    };
    let title = match setup.step {
        Step::Address => " Add an account ",
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
        Step::Server => {
            if let Some(note) = &setup.note {
                rows.dim(note);
                rows.blank();
            }
            let at = setup.server_field();
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
            if setup.tls == TlsMode::None {
                rows.blank();
                rows.warning(
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

fn status(rows: &mut Rows<'_, '_>, setup: &Setup) {
    if let Some(busy) = &setup.busy {
        rows.blank();
        rows.dim(busy);
    } else if let Some(error) = &setup.error {
        rows.blank();
        rows.warning(error);
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
}

impl<'f, 'a> Rows<'f, 'a> {
    fn new(frame: &'f mut Frame<'a>, area: Rect, env: &'f Env) -> Self {
        Self {
            frame,
            area,
            y: area.y,
            env,
        }
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

    /// A label and a value that is only shown.
    fn pair(&mut self, label: &str, value: &str) {
        let Some(rect) = self.row() else { return };
        let room = usize::from(self.area.width).saturating_sub(LABEL_WIDTH);
        let line = Line::from(vec![
            Span::styled(text::fit(label, LABEL_WIDTH), self.env.styles.dim()),
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
        let room = usize::from(self.area.width).saturating_sub(LABEL_WIDTH);
        let (shown, column) = input.view(room);
        let label_style = if focused {
            self.env.styles.key()
        } else {
            self.env.styles.dim()
        };
        // Underlined to its full width, so an empty field is still a field.
        let field_style = self.env.styles.text().add_modifier(Modifier::UNDERLINED);
        let line = Line::from(vec![
            Span::styled(text::fit(label, LABEL_WIDTH), label_style),
            Span::styled(text::fit(&shown, room), field_style),
        ]);
        self.frame.render_widget(line, rect);
        if focused {
            self.frame.set_cursor_position(Position::new(
                rect.x + (LABEL_WIDTH + column) as u16,
                rect.y,
            ));
        }
    }

    /// A label and one of a few values, changed with the arrow keys.
    fn choice(&mut self, label: &str, value: &str, focused: bool) {
        let Some(rect) = self.row() else { return };
        let styles = &self.env.styles;
        let mut spans = vec![Span::styled(
            text::fit(label, LABEL_WIDTH),
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
