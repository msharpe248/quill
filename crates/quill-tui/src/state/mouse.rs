//! The mouse: a click focuses and selects, a second click on what is
//! selected opens it, the wheel scrolls what is under the pointer, the
//! edge between two panes can be dragged, and the words of the message on
//! show can be dragged over to select them (`select.rs`).
//!
//! But for the dragging, nothing here does anything the keyboard cannot:
//! a click ends in the same selection or the same command a key would
//! have reached. Where the pointer is, is worked out from the same layout
//! the views draw with.

use quill_core::types::LocalAccountId;
use ratatui::layout::{Position, Rect};

use super::{Effects, Env, Motion, Pane, State, step};
use crate::command::CommandId;
use crate::compose::Field;
use crate::layout;
use crate::layout::{Divider, Split};
use crate::setup::{AccountRequest, Preference};

/// What the mouse did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseKind {
    /// The main button went down.
    Press,
    /// The pointer moved with the main button held.
    Drag,
    /// The main button was let go.
    Release,
    WheelUp,
    WheelDown,
}

/// One thing the mouse did, and the cell it did it over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mouse {
    pub kind: MouseKind,
    pub column: u16,
    pub row: u16,
}

/// An edge between two panes that the user has hold of: from the press on
/// it to the letting go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Resizing {
    pub divider: Divider,
    /// How many columns left of the edge the pointer took hold.
    grab: u16,
    /// The column the pointer last asked the edge to be at.
    edge: u16,
    /// Whether the edge has been moved since.
    moved: bool,
}

/// How many seconds may pass between two presses on an edge for them to
/// be a double click. The clock here ticks in whole seconds.
const DOUBLE_PRESS_SECONDS: i64 = 1;

/// How many rows one notch of the wheel moves.
const WHEEL_ROWS: usize = 3;

/// Whether a click on text drawn this way, which belongs to a link, opens
/// the link: its words, and what is set beside them for finding it by (its
/// number, its line in the list of links). The warning under a link that
/// is not what it says belongs to the link too, and is only to be read.
pub(super) fn opens_on_click(style: quill_mime::html::Style) -> bool {
    style.link || style.dim
}

impl State {
    pub(super) fn on_mouse(&mut self, env: &Env, mouse: Mouse, out: &mut Effects) {
        // An edge between panes that is in hand has the mouse to itself
        // until it is let go.
        match mouse.kind {
            MouseKind::Release => {
                self.let_go_of_divider(out);
                self.let_go_in_reader(out);
                return;
            }
            MouseKind::Drag if self.resizing.is_some() => {
                self.drag_divider(env, mouse.column);
                return;
            }
            // The button went down on the message's words and has not
            // been let go: they are being selected, wherever the pointer
            // has been taken to since.
            MouseKind::Drag if self.held.is_some() => {
                self.drag_in_reader(env, mouse.column, mouse.row);
                return;
            }
            // A press with an edge still in hand: its letting go never
            // arrived. Where it was left is kept all the same. A press
            // in the reader whose letting go never arrived was no click.
            MouseKind::Press => {
                self.let_go_of_divider(out);
                self.held = None;
            }
            MouseKind::Drag | MouseKind::WheelUp | MouseKind::WheelDown => {}
        }
        let areas = layout::compute(self, self.size, env.reader_width);
        let at = Position::new(mouse.column, mouse.row);
        let over = |rect: Option<Rect>| rect.is_some_and(|rect| rect.contains(at));
        let wheel = match mouse.kind {
            MouseKind::WheelUp => Some(Motion::Prev),
            MouseKind::WheelDown => Some(Motion::Next),
            MouseKind::Press | MouseKind::Drag | MouseKind::Release => None,
        };

        // What is drawn over the panes takes the mouse first.
        if self.help.is_some() {
            match (wheel, mouse.kind) {
                (Some(motion), _) => {
                    for _ in 0..WHEEL_ROWS {
                        self.navigate(env, motion, out);
                    }
                }
                // A click beside it puts it away.
                (None, MouseKind::Press) if !over(areas.help) => self.back(env, out),
                _ => {}
            }
            return;
        }
        // A form or a question about a certificate is answered with keys:
        // a stray click must not answer it, or dismiss it.
        // The same while the form is in a sheet of the host's: nothing
        // behind it moves.
        if areas.dialog.is_some() || self.setup.is_some() || self.form_in_sheet() {
            return;
        }
        if let Some(rect) = areas.links {
            self.mouse_in_list_box(env, rect, mouse, wheel, out);
            return;
        }
        // The edge between two panes, before either of them: it is the
        // border of both.
        if mouse.kind == MouseKind::Press {
            if let Some((divider, edge)) = layout::divider_at(&areas, mouse.column, mouse.row) {
                self.press_divider(divider, edge, edge - mouse.column, out);
                return;
            }
            self.divider_pressed = None;
        }
        if self.compose.is_some() && over(areas.reader) {
            self.mouse_in_compose(areas.reader, mouse, wheel);
            return;
        }

        // The sidebar first: under 60 columns it lies over the main pane.
        if over(areas.sidebar) {
            self.mouse_in_sidebar(env, areas.sidebar, mouse, wheel, out);
        } else if areas.sidebar_overlay {
            // A click beside the overlay closes it, as esc does.
            if mouse.kind == MouseKind::Press {
                self.back(env, out);
            }
        } else if over(areas.list) {
            self.mouse_in_message_list(env, areas.list, mouse, wheel, out);
        } else if over(areas.reader) {
            self.mouse_in_reader(env, areas.reader, mouse, wheel, out);
        }
    }

    /// The main button went down on the edge between two panes: it is in
    /// hand until it is let go. A second press straight after one that
    /// moved nothing is a double click, and puts the edge back where
    /// Quill has it by itself.
    fn press_divider(&mut self, divider: Divider, edge: u16, grab: u16, out: &mut Effects) {
        let again = self.divider_pressed.take().is_some_and(|(pressed, at)| {
            pressed == divider && self.now - at <= DOUBLE_PRESS_SECONDS
        });
        if again {
            self.reset_divider(divider, out);
            return;
        }
        self.resizing = Some(Resizing {
            divider,
            grab,
            edge,
            moved: false,
        });
        self.divider_pressed = Some((divider, self.now));
    }

    /// The pointer moved to `column` with an edge in hand: the edge goes
    /// with it, as far as every pane's least width allows.
    fn drag_divider(&mut self, env: &Env, column: u16) {
        let Some(held) = self.resizing else {
            return;
        };
        let edge = column.saturating_add(held.grab);
        // Up and down the edge is no move at all.
        if edge == held.edge {
            return;
        }
        let Some(split) = layout::dragged(self, self.size.0, env.reader_width, held.divider, edge)
        else {
            return;
        };
        self.set_split(env, split);
        self.resizing = Some(Resizing {
            edge,
            moved: true,
            ..held
        });
        // It has been moved, so it is not half of a double click.
        self.divider_pressed = None;
    }

    /// The widths the panes are shared out by, from now and for the rest
    /// of the run.
    fn set_split(&mut self, env: &Env, split: Split) {
        if split.sidebar != self.sidebar_width {
            self.sidebar_dragged = Some(split.sidebar);
            self.sidebar_width = split.sidebar;
        }
        if split.reader != Split::of(self, env.reader_width).reader {
            self.reader_share = Some(split.reader);
        }
    }

    /// The edge in hand was let go: where it was left is remembered for
    /// the next run, if it was moved at all.
    fn let_go_of_divider(&mut self, out: &mut Effects) {
        let Some(held) = self.resizing.take() else {
            return;
        };
        if !held.moved {
            return;
        }
        if held.divider == Divider::Sidebar {
            out.accounts
                .push(AccountRequest::Remember(Preference::SidebarWidth(
                    self.sidebar_width,
                )));
        }
        // Moving the accounts' edge changes the message's share as well,
        // where the message kept its columns.
        if let Some(share) = self.reader_share {
            out.accounts
                .push(AccountRequest::Remember(Preference::ReaderWidth(Some(
                    (share * 1000.0).round() as u16,
                ))));
        }
    }

    /// The divider between the terminal and the host's reading pane was
    /// dragged to where the terminal keeps `columns` columns, or with
    /// none was put back. It is the reader's edge, in the host's window:
    /// the message has the share that leaves the terminal those columns,
    /// read as a page or as text, in this run and the next.
    pub(super) fn reading_dragged(&mut self, env: &Env, columns: Option<u16>, out: &mut Effects) {
        if !self.web_reading {
            return;
        }
        let Some(columns) = columns else {
            self.reset_divider(Divider::Reader, out);
            return;
        };
        let Some(window) = self.window_columns else {
            return;
        };
        let share = layout::dragged_beside_reading(
            window,
            Split::of(self, env.reader_width),
            !self.sidebar_hidden,
            columns,
        );
        self.reader_share = Some(share);
        out.accounts
            .push(AccountRequest::Remember(Preference::ReaderWidth(Some(
                (share * 1000.0).round() as u16,
            ))));
    }

    /// Puts an edge back where Quill has it when nobody has moved it, and
    /// has the setting taken out of the config.
    fn reset_divider(&mut self, divider: Divider, out: &mut Effects) {
        self.resizing = None;
        let preference = match divider {
            Divider::Sidebar => {
                self.sidebar_dragged = Some(None);
                self.sidebar_width = None;
                Preference::SidebarWidth(None)
            }
            Divider::Reader => {
                self.reader_share = Some(quill_core::config::READER_USUAL);
                Preference::ReaderWidth(None)
            }
        };
        out.accounts.push(AccountRequest::Remember(preference));
    }

    /// A box with a list in it: links, attachments, folders, templates,
    /// spellings, the palette.
    fn mouse_in_list_box(
        &mut self,
        env: &Env,
        rect: Rect,
        mouse: Mouse,
        wheel: Option<Motion>,
        out: &mut Effects,
    ) {
        if let Some(motion) = wheel {
            self.navigate(env, motion, out);
            return;
        }
        if mouse.kind != MouseKind::Press {
            return;
        }
        if !rect.contains(Position::new(mouse.column, mouse.row)) {
            self.back(env, out);
            return;
        }
        // Folders, templates and the palette have a line to type on above
        // the list.
        let typed_rows =
            u16::from(self.palette.is_some() || self.folders.is_some() || self.templates.is_some());
        let first = rect.y + 1 + typed_rows;
        let last = rect.y + rect.height.saturating_sub(1);
        if mouse.row < first || mouse.row >= last {
            return;
        }
        let row = usize::from(mouse.row - first);
        let (count, selected, offset) = if let Some(palette) = &self.palette {
            (
                self.palette_items(env).len(),
                palette.selected,
                palette.offset,
            )
        } else if let Some(picker) = &self.folders {
            (
                self.folder_choices(picker).len(),
                picker.selected,
                picker.offset,
            )
        } else if let Some(picker) = &self.templates {
            (
                self.template_choices(env, picker).len(),
                picker.selected,
                picker.offset,
            )
        } else if let Some(picker) = &self.suggestions {
            (picker.words.len(), picker.selected, picker.offset)
        } else if let Some(picker) = &self.attachments {
            (self.reader.files().len(), picker.selected, picker.offset)
        } else if let Some(picker) = &self.links {
            (self.reader.links.len(), picker.selected, picker.offset)
        } else {
            return;
        };
        let index = offset + row;
        if index >= count {
            return;
        }
        if index == selected {
            // A click on what is chosen is `enter`.
            self.run(env, CommandId::NavOpen, out);
            return;
        }
        if let Some(palette) = &mut self.palette {
            palette.selected = index;
        } else if let Some(picker) = &mut self.folders {
            picker.selected = index;
        } else if let Some(picker) = &mut self.templates {
            picker.selected = index;
        } else if let Some(picker) = &mut self.suggestions {
            picker.selected = index;
        } else if let Some(picker) = &mut self.attachments {
            picker.selected = index;
        } else if let Some(picker) = &mut self.links {
            picker.selected = index;
            picker.typed.clear();
        }
    }

    fn mouse_in_compose(&mut self, rect: Option<Rect>, mouse: Mouse, wheel: Option<Motion>) {
        let accounts: Vec<LocalAccountId> =
            self.accounts.iter().map(|account| account.id).collect();
        let (Some(rect), Some(compose)) = (rect, &mut self.compose) else {
            return;
        };
        // Being sent, or all of it in an editor: there is nothing here to
        // point at.
        if compose.sending || !compose.fields_in_reach() {
            return;
        }
        // The list of accounts is open under the From field: a click on
        // one of them sends the message from it, and a click anywhere
        // else puts the list away and is that click and no more.
        if let Some(choice) = compose.choosing {
            if mouse.kind == MouseKind::Press {
                let listed = mouse
                    .row
                    .checked_sub(rect.y + 2)
                    .map(usize::from)
                    .filter(|row| *row < choice.count);
                match listed {
                    Some(index) => compose.choose_account(index, &accounts),
                    None => compose.choosing = None,
                }
            }
            return;
        }
        let body = layout::compose_body(rect, compose);
        // An editor has the text and the fields are still here: a click
        // on the text is the editor's (the host sees to its keys), and
        // the wheel has nothing here to turn.
        let text_away = compose.away.is_some();
        if text_away && wheel.is_some() {
            return;
        }
        if text_away
            && mouse.kind == MouseKind::Press
            && body.contains(Position::new(mouse.column, mouse.row))
        {
            compose.go_to_body();
            return;
        }
        if text_away && mouse.kind != MouseKind::Press {
            return;
        }
        if let Some(motion) = wheel {
            let rows = WHEEL_ROWS as isize;
            compose
                .body
                .scroll(if motion == Motion::Next { rows } else { -rows });
            return;
        }
        let in_body = body.contains(Position::new(mouse.column, mouse.row));
        match mouse.kind {
            MouseKind::Press if in_body => {
                compose.field = Field::Body;
                compose.completion = None;
                compose.attaching = None;
                compose
                    .body
                    .click(mouse.column - body.x, mouse.row - body.y);
            }
            // Dragging on past the edge of the body keeps selecting to it.
            MouseKind::Drag if compose.field == Field::Body => {
                let column = mouse
                    .column
                    .clamp(body.x, body.x + body.width.saturating_sub(1));
                let row = mouse
                    .row
                    .clamp(body.y, body.y + body.height.saturating_sub(1));
                compose.body.drag(column - body.x, row - body.y);
            }
            MouseKind::Press => {
                // A header field, by the row it is drawn on.
                let mut fields = vec![Field::From, Field::To, Field::Cc];
                if compose.field == Field::Bcc || !compose.bcc.is_empty() {
                    fields.push(Field::Bcc);
                }
                fields.push(Field::Subject);
                let Some(row) = mouse.row.checked_sub(rect.y + 1) else {
                    return;
                };
                if let Some(field) = fields.get(usize::from(row)) {
                    compose.field = *field;
                    compose.completion = None;
                    compose.attaching = None;
                    // A click on From opens its list, as a click on a
                    // pop-up menu does.
                    if *field == Field::From {
                        compose.toggle_accounts(&accounts);
                    }
                }
            }
            _ => {}
        }
    }

    fn mouse_in_sidebar(
        &mut self,
        env: &Env,
        rect: Option<Rect>,
        mouse: Mouse,
        wheel: Option<Motion>,
        out: &mut Effects,
    ) {
        let Some(rect) = rect else {
            return;
        };
        if let Some(motion) = wheel {
            // Its entries are mailboxes: moving among them changes what
            // the list shows, so the pane takes the focus as for a key.
            self.set_focus(Pane::Sidebar);
            self.navigate(env, motion, out);
            return;
        }
        if mouse.kind != MouseKind::Press {
            return;
        }
        self.set_focus(Pane::Sidebar);
        let Some(row) = mouse.row.checked_sub(rect.y + 1) else {
            return;
        };
        if mouse.row >= rect.y + rect.height.saturating_sub(1) {
            return;
        }
        // Among the rows on show: a closed account's folders are not there
        // to be clicked.
        let Some(index) = self
            .sidebar
            .shown()
            .get(self.sidebar.offset + usize::from(row))
            .copied()
        else {
            return;
        };
        let Some(entry) = self.sidebar.entries.get(index) else {
            return;
        };
        // An account's marker, and the margin left of its name, open and
        // close it. The rest of its row is the account, to go to.
        if let super::SidebarEntry::Account { id, .. } = entry
            && mouse.column <= rect.x + layout::SIDEBAR_MARKER_CELLS
        {
            let account = *id;
            self.toggle_account(env, account, out);
            return;
        }
        // A heading is not somewhere to go.
        if !entry.is_place() {
            return;
        }
        if index == self.sidebar.selected {
            // A click on the mailbox on show goes to its mail.
            self.run(env, CommandId::NavOpen, out);
        } else {
            self.go_to_entry(env, index, out);
        }
    }

    fn mouse_in_message_list(
        &mut self,
        env: &Env,
        rect: Option<Rect>,
        mouse: Mouse,
        wheel: Option<Motion>,
        out: &mut Effects,
    ) {
        let Some(rect) = rect else {
            return;
        };
        // Typing in the search box ends when the list is reached for.
        if let Some(search) = &mut self.search
            && mouse.kind == MouseKind::Press
            && !self.list.rows.is_empty()
        {
            search.typing = false;
        }
        if let Some(motion) = wheel {
            // The list under the pointer moves, wherever the focus is.
            let target = (0..WHEEL_ROWS).fold(self.list.selected, |selected, _| {
                step(selected, self.list.rows.len(), motion, 1)
            });
            if target != self.list.selected {
                self.list.selected = target;
                self.sync_reader(out);
            }
            return;
        }
        if mouse.kind != MouseKind::Press {
            return;
        }
        let had_focus = self.focus == Pane::List;
        self.set_focus(Pane::List);
        let Some(row) = mouse.row.checked_sub(rect.y + 1) else {
            return;
        };
        if mouse.row >= rect.y + rect.height.saturating_sub(1) {
            return;
        }
        let index = self.list.offset + usize::from(row);
        if index >= self.list.rows.len() {
            return;
        }
        if index == self.list.selected && had_focus {
            // A click on the message under the cursor opens it.
            self.run(env, CommandId::NavOpen, out);
        } else if index != self.list.selected {
            self.list.selected = index;
            // Clicked, the message is gone to; the wheel only passes by.
            self.follow.went = true;
            self.sync_reader(out);
        }
    }

    fn mouse_in_reader(
        &mut self,
        env: &Env,
        rect: Option<Rect>,
        mouse: Mouse,
        wheel: Option<Motion>,
        out: &mut Effects,
    ) {
        let Some(rect) = rect else {
            return;
        };
        if let Some(motion) = wheel {
            // Clamped to the end of the body by `relayout`.
            self.reader.scroll = match motion {
                Motion::Next => self.reader.scroll.saturating_add(WHEEL_ROWS),
                _ => self.reader.scroll.saturating_sub(WHEEL_ROWS),
            };
            return;
        }
        if mouse.kind != MouseKind::Press || self.reader.row.is_none() {
            return;
        }
        self.set_focus(Pane::Reader);
        let fields = self.reader_field_rows() as u16;
        let top = rect.y + 1;
        // The line naming the files opens their list.
        if !self.reader.files().is_empty() && mouse.row == top + fields - 1 {
            self.reader.selection = None;
            self.run(env, CommandId::MessageAttachments, out);
            return;
        }
        // Anywhere else on the message it is the start of a click or of
        // a selection: a link opens when the button is let go where it
        // went down, and not before.
        self.press_in_reader(env, rect, mouse.column, mouse.row);
    }
}

#[cfg(test)]
mod tests {
    use quill_mime::html::Style;

    use super::opens_on_click;

    #[test]
    fn a_links_words_number_and_listing_open_it_and_its_warning_does_not() {
        let words = Style {
            link: true,
            ..Style::default()
        };
        // The number after the words, and the line in the list of links.
        let aside = Style {
            dim: true,
            ..Style::default()
        };
        // The warning under a link that is not what it says.
        let warning = Style {
            bold: true,
            ..Style::default()
        };
        assert!(opens_on_click(words));
        assert!(opens_on_click(aside));
        assert!(!opens_on_click(warning));
        assert!(!opens_on_click(Style::default()));
    }
}
