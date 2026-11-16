//! The mouse: a click focuses and selects, a second click on what is
//! selected opens it, the wheel scrolls what is under the pointer.
//!
//! Nothing here does anything the keyboard cannot: a click ends in the same
//! selection or the same command a key would have reached. Where the
//! pointer is, is worked out from the same layout the views draw with.

use ratatui::layout::{Position, Rect};

use super::{Effects, Env, HostRequest, Motion, Notice, Pane, State, step};
use crate::command::CommandId;
use crate::compose::Field;
use crate::{layout, text};

/// What the mouse did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseKind {
    /// The main button went down.
    Press,
    /// The pointer moved with the main button held.
    Drag,
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

/// How many rows one notch of the wheel moves.
const WHEEL_ROWS: usize = 3;

impl State {
    pub(super) fn on_mouse(&mut self, env: &Env, mouse: Mouse, out: &mut Effects) {
        let areas = layout::compute(self, self.size, env.reader_width);
        let at = Position::new(mouse.column, mouse.row);
        let over = |rect: Option<Rect>| rect.is_some_and(|rect| rect.contains(at));
        let wheel = match mouse.kind {
            MouseKind::WheelUp => Some(Motion::Prev),
            MouseKind::WheelDown => Some(Motion::Next),
            MouseKind::Press | MouseKind::Drag => None,
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
        if areas.dialog.is_some() || self.setup.is_some() {
            return;
        }
        if let Some(rect) = areas.links {
            self.mouse_in_list_box(env, rect, mouse, wheel, out);
            return;
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
        let (Some(rect), Some(compose)) = (rect, &mut self.compose) else {
            return;
        };
        // Being sent, or in Vim: there is nothing here to point at.
        if compose.sending || compose.away.is_some() {
            return;
        }
        let body = layout::compose_body(rect, compose);
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
        let header = self.reader_header_rows() as u16;
        let fields = self.reader_field_rows() as u16;
        let top = rect.y + 1;
        // The line naming the files opens their list.
        if !self.reader.files().is_empty() && mouse.row == top + fields - 1 {
            self.run(env, CommandId::MessageAttachments, out);
            return;
        }
        // A link in the text opens, as it would from the list of links.
        let picture = self.reader_image_rows(env, rect) as u16;
        let body_top = top + header + 1 + if picture > 0 { picture + 1 } else { 0 };
        let Some(row) = mouse.row.checked_sub(body_top) else {
            return;
        };
        let Some(column) = mouse.column.checked_sub(rect.x + 2) else {
            return;
        };
        let Some(line) = self.reader.lines.get(self.reader.scroll + usize::from(row)) else {
            return;
        };
        let mut start = 0;
        let mut link = None;
        for span in &line.spans {
            let end = start + text::width(&span.text);
            if (start..end).contains(&usize::from(column)) {
                link = span.link.filter(|_| span.style.link);
                break;
            }
            start = end;
        }
        let Some(url) = link.and_then(|index| self.reader.links.get(index)) else {
            return;
        };
        if quill_mime::links::is_openable(url) {
            out.host.push(HostRequest::OpenUrl(url.clone()));
        } else {
            self.notices.push_back(Notice::error(
                "Quill only opens web (http, https) and mail (mailto) links. This is not one.",
            ));
        }
    }
}
