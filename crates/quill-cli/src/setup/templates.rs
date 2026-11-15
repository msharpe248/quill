//! Reads the user's templates: the `.md` and `.txt` files in the
//! `templates` folder beside `config.toml`.
//!
//! A file's name without its ending is the template's name. Like the rest
//! of what is loaded at startup, nothing here is fatal: a file that cannot
//! be used is left out and said so in one notice.
//!
//! The folder is the user's own, but it is still read with limits, so that
//! a stray large file or a folder of thousands cannot slow the start or
//! fill the picker.

use std::io::{ErrorKind, Read};
use std::path::Path;

use quill_tui::state::Notice;
use quill_tui::template::Template;

/// A template larger than this is left out. Sixty-four kibibytes is a
/// long letter many times over.
pub const MAX_FILE_BYTES: u64 = 64 * 1024;

/// The most templates that are loaded: the first of them by name.
pub const MAX_TEMPLATES: usize = 200;

/// The most files named in the notice about those left out.
const MAX_NAMED: usize = 5;

/// The templates in `dir`, in order of name. A folder that is not there
/// is no templates and nothing to report.
pub fn load(dir: &Path, notices: &mut Vec<Notice>) -> Vec<Template> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == ErrorKind::NotFound => return Vec::new(),
        Err(err) => {
            notices.push(Notice::error(format!(
                "cannot read {}: {err}. No templates are loaded.",
                dir.display()
            )));
            return Vec::new();
        }
    };
    // Names first, so that which files are used does not depend on the
    // order the disk happens to list them in.
    let mut names: Vec<String> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| stem(name).is_some())
        .collect();
    names.sort_by_key(|name| (name.to_lowercase(), name.clone()));

    let mut left_out: Vec<String> = Vec::new();
    let extra = names.len().saturating_sub(MAX_TEMPLATES);
    names.truncate(MAX_TEMPLATES);
    let mut templates = Vec::new();
    for name in &names {
        let Some(stem) = stem(name) else { continue };
        match read(&dir.join(name)) {
            Ok(text) => {
                let template = Template::parse(stem, &text);
                // A name with nothing in it that can be shown cannot be
                // chosen from a list.
                if template.name.is_empty() {
                    left_out.push(format!("{} (its name cannot be shown)", shown(name)));
                } else {
                    templates.push(template);
                }
            }
            Err(why) => left_out.push(format!("{} ({why})", shown(name))),
        }
    }
    if !left_out.is_empty() {
        let more = left_out.len().saturating_sub(MAX_NAMED);
        left_out.truncate(MAX_NAMED);
        let mut said = format!("Templates left out: {}", left_out.join(", "));
        if more > 0 {
            said.push_str(&format!(", and {more} more"));
        }
        said.push('.');
        notices.push(Notice::error(said));
    }
    if extra > 0 {
        notices.push(Notice::error(format!(
            "There are {} templates. The first {MAX_TEMPLATES} by name are used.",
            MAX_TEMPLATES + extra
        )));
    }
    templates
}

/// The name of a template file without its ending, or `None` for a file
/// that is not one: another kind of file, or a hidden one.
fn stem(file_name: &str) -> Option<&str> {
    if file_name.starts_with('.') {
        return None;
    }
    let (stem, ending) = file_name.rsplit_once('.')?;
    let is_template = ending.eq_ignore_ascii_case("md") || ending.eq_ignore_ascii_case("txt");
    (is_template && !stem.is_empty()).then_some(stem)
}

/// A file's name as a notice may say it: one line, nothing that could do
/// anything to the terminal, and not too long.
fn shown(file_name: &str) -> String {
    quill_tui::text::truncate(&quill_tui::text::sanitize_line(file_name), 40)
}

/// The file's text, or why it is not used, in a few words.
fn read(path: &Path) -> Result<String, String> {
    let unreadable = |err: std::io::Error| format!("cannot be read: {err}");
    let too_large = || format!("is larger than {} KiB", MAX_FILE_BYTES / 1024);
    // Looked at before it is opened: opening a pipe would wait for
    // whoever writes to it, and a folder is not text.
    let metadata = std::fs::metadata(path).map_err(unreadable)?;
    if !metadata.is_file() {
        return Err("is not a file".to_owned());
    }
    if metadata.len() > MAX_FILE_BYTES {
        return Err(too_large());
    }
    // Read to the limit and no further, whatever the size was a moment ago.
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .map_err(unreadable)?
        .take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(unreadable)?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(too_large());
    }
    String::from_utf8(bytes).map_err(|_| "is not text (UTF-8)".to_owned())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use quill_tui::state::NoticeKind;
    use tempfile::TempDir;

    use super::*;

    fn folder() -> (TempDir, std::path::PathBuf) {
        let dir = TempDir::new().unwrap();
        let templates = dir.path().join("config").join("templates");
        fs::create_dir_all(&templates).unwrap();
        (dir, templates)
    }

    #[test]
    fn no_folder_is_no_templates_and_nothing_to_say() {
        let dir = TempDir::new().unwrap();
        let mut notices = Vec::new();
        assert!(load(&dir.path().join("templates"), &mut notices).is_empty());
        assert!(notices.is_empty());
        // And none is made by looking.
        assert!(!dir.path().join("templates").exists());
    }

    #[test]
    fn md_and_txt_files_are_templates_named_after_their_files() {
        let (_dir, templates) = folder();
        fs::write(
            templates.join("Meeting follow-up.md"),
            "Subject: Following up\n\nHi {{first_name}},\n\nThanks for today.\n",
        )
        .unwrap();
        fs::write(templates.join("thanks.TXT"), "Thanks,\nRobin\n").unwrap();
        // Not templates: other kinds of file, hidden files, a folder, and
        // a file with nothing before its ending.
        fs::write(templates.join("notes.docx"), "x").unwrap();
        fs::write(templates.join("README"), "x").unwrap();
        fs::write(templates.join(".hidden.md"), "x").unwrap();
        fs::write(templates.join("draft.md~"), "x").unwrap();
        fs::create_dir(templates.join("more.md")).unwrap();
        fs::create_dir(templates.join("archive")).unwrap();
        fs::write(templates.join("archive").join("old.md"), "x").unwrap();

        let mut notices = Vec::new();
        let loaded = load(&templates, &mut notices);
        let names: Vec<&str> = loaded.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["Meeting follow-up", "thanks"]);
        assert_eq!(loaded[0].subject.as_deref(), Some("Following up"));
        assert_eq!(loaded[0].body, "Hi {{first_name}},\n\nThanks for today.");
        assert_eq!(loaded[1].subject, None);
        assert_eq!(loaded[1].body, "Thanks,\nRobin");
        // The folder named like a template is said to be left out; the
        // files that are simply something else are not mentioned.
        assert_eq!(notices.len(), 1, "{notices:?}");
        assert_eq!(
            notices[0].text,
            "Templates left out: more.md (is not a file)."
        );
    }

    #[test]
    fn a_file_that_is_too_large_or_not_text_is_left_out_and_said() {
        let (_dir, templates) = folder();
        fs::write(templates.join("a-fine.md"), "Fine.").unwrap();
        // Exactly at the limit is in; a byte over is out.
        fs::write(
            templates.join("b-limit.md"),
            "x".repeat(MAX_FILE_BYTES as usize),
        )
        .unwrap();
        fs::write(
            templates.join("c-large.md"),
            "x".repeat(MAX_FILE_BYTES as usize + 1),
        )
        .unwrap();
        fs::write(templates.join("d-binary.txt"), [0xff, 0xfe, 0x00, 0x41]).unwrap();

        let mut notices = Vec::new();
        let loaded = load(&templates, &mut notices);
        let names: Vec<&str> = loaded.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["a-fine", "b-limit"]);
        assert_eq!(notices.len(), 1);
        assert_eq!(notices[0].kind, NoticeKind::Error);
        assert_eq!(
            notices[0].text,
            "Templates left out: c-large.md (is larger than 64 KiB), d-binary.txt (is not text (UTF-8))."
        );
    }

    #[test]
    fn names_and_text_are_made_safe_to_show() {
        let (_dir, templates) = folder();
        fs::write(
            templates.join("es\u{1b}[31mcape.md"),
            "Subject: a\u{1b}]0;title\u{7}b\n\nOne\u{1b}[2J\ttwo\u{0}\r\nthree\r\n",
        )
        .unwrap();
        // A name that is nothing but a control character has nothing to
        // show in a list.
        fs::write(templates.join("\u{7}.md"), "x").unwrap();
        let mut notices = Vec::new();
        let loaded = load(&templates, &mut notices);
        assert_eq!(loaded.len(), 1);
        let template = &loaded[0];
        assert_eq!(template.name, "escape");
        let all = format!(
            "{}{}{}",
            template.name,
            template.subject.as_deref().unwrap(),
            template.body
        );
        assert!(
            !all.contains(|c: char| c.is_control() && c != '\n'),
            "{all:?}"
        );
        assert!(template.body.ends_with("two\nthree"));
        assert_eq!(notices.len(), 1);
        assert!(!notices[0].text.contains('\u{7}'));
        assert!(
            notices[0].text.contains("(its name cannot be shown)"),
            "{}",
            notices[0].text
        );
    }

    #[test]
    fn only_so_many_templates_are_loaded_the_first_by_name() {
        let (_dir, templates) = folder();
        for number in 0..MAX_TEMPLATES + 7 {
            fs::write(templates.join(format!("t{number:04}.md")), "text").unwrap();
        }
        let mut notices = Vec::new();
        let loaded = load(&templates, &mut notices);
        assert_eq!(loaded.len(), MAX_TEMPLATES);
        assert_eq!(loaded[0].name, "t0000");
        assert_eq!(loaded[MAX_TEMPLATES - 1].name, "t0199");
        assert_eq!(notices.len(), 1);
        assert_eq!(
            notices[0].text,
            "There are 207 templates. The first 200 by name are used."
        );
    }

    #[test]
    fn many_unusable_files_are_summed_up() {
        let (_dir, templates) = folder();
        for number in 0..9 {
            fs::write(templates.join(format!("bad{number}.md")), [0xff]).unwrap();
        }
        let mut notices = Vec::new();
        assert!(load(&templates, &mut notices).is_empty());
        assert_eq!(notices.len(), 1);
        assert!(
            notices[0].text.ends_with(", and 4 more."),
            "{}",
            notices[0].text
        );
        assert!(!notices[0].text.contains('\n'));
    }

    #[cfg(unix)]
    #[test]
    fn a_pipe_named_like_a_template_is_not_waited_on() {
        let (_dir, templates) = folder();
        let pipe = templates.join("pipe.md");
        let made = std::process::Command::new("mkfifo").arg(&pipe).status();
        if !made.is_ok_and(|status| status.success()) {
            return;
        }
        fs::write(templates.join("real.md"), "Real.").unwrap();
        // Opening a pipe to read waits for someone to write to it, and
        // nobody will: were the loader to open it, this would never end.
        let mut notices = Vec::new();
        let loaded = load(&templates, &mut notices);
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].name, "real");
        assert!(notices[0].text.contains("pipe.md (is not a file)"));
    }
}
