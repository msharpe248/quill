//! The skill: what teaches an agent to use `quill`'s commands.
//!
//! It is one file, `skills/quill/SKILL.md`, in the form agents' programs
//! share (a folder named for the skill with a `SKILL.md` in it). It is
//! kept in three places that are one text: in the repository, in the
//! app's bundle beside this program, and in this program itself, so that
//! `quill skill show` always has the skill of the Quill that is asked.
//!
//! An agent's program finds a skill in a folder of its own. `quill skill
//! install` puts Quill's there as a link to the one that ships with
//! Quill, so that it is never out of step with the commands; or as a
//! copy, where there is nothing on disk to link to.

use std::path::{Path, PathBuf};

/// The skill, as this program was built with it.
pub const SKILL: &str = include_str!("../../../skills/quill/SKILL.md");

/// What the skill is called, and so what its folder is called.
pub const NAME: &str = "quill";

/// The folders agents' programs read skills from, under the user's home:
/// Claude Code's own, and the one the others share (Codex, Gemini CLI,
/// Cursor, Copilot CLI, OpenCode and Goose all read it).
pub const USUAL: [&str; 2] = [".claude/skills", ".agents/skills"];

/// The skill's folder as it ships beside this program: in the app's
/// bundle, or in the checkout a build was made from. `None` when this
/// program is by itself somewhere.
pub fn shipped() -> Option<PathBuf> {
    // Through a link on the `PATH` to where the program really is.
    let program = std::env::current_exe().ok()?.canonicalize().ok()?;
    shipped_beside(&program)
}

fn shipped_beside(program: &Path) -> Option<PathBuf> {
    let folder = program.parent()?;
    [
        // Quill.app/Contents/Helpers/quill
        folder.join("../Resources/skills").join(NAME),
        // target/release/quill, in a checkout
        folder.join("../../skills").join(NAME),
    ]
    .into_iter()
    .find(|candidate| candidate.join("SKILL.md").is_file())
    .and_then(|found| found.canonicalize().ok())
}

/// The folders to put the skill in when none is named.
pub fn usual_places(home: &Path) -> Vec<PathBuf> {
    USUAL.iter().map(|place| home.join(place)).collect()
}

/// How the skill was put in a folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Put {
    Linked { at: PathBuf, to: PathBuf },
    Copied { at: PathBuf },
}

impl std::fmt::Display for Put {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Linked { at, to } => write!(f, "{} -> {}", at.display(), to.display()),
            Self::Copied { at } => write!(f, "{} (a copy)", at.display()),
        }
    }
}

/// Whether the folder at `path` is a copy of this skill that an earlier
/// install wrote: it has a `SKILL.md` that says it is Quill's, and
/// nothing else in it.
fn is_our_copy(path: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(path) else {
        return false;
    };
    let names: Vec<_> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.file_name())
        .collect();
    names == ["SKILL.md"]
        && std::fs::read_to_string(path.join("SKILL.md"))
            .is_ok_and(|text| text.starts_with(&format!("---\nname: {NAME}\n")))
}

/// Puts the skill in `skills`, a folder an agent's program reads skills
/// from, as `skills/quill`: a link to `shipped` when there is one and a
/// copy was not asked for, otherwise a copy of the text this program was
/// built with.
///
/// What is there already is replaced only if an install put it there: a
/// link, or a copy of this skill. Anything else is somebody's, and is
/// left alone with the reason said.
pub fn install(skills: &Path, shipped: Option<&Path>, copy: bool) -> Result<Put, String> {
    let at = skills.join(NAME);
    let failed = |what: &str, err: std::io::Error| format!("{}: {what}: {err}", at.display());
    std::fs::create_dir_all(skills)
        .map_err(|err| format!("{}: cannot be made: {err}", skills.display()))?;
    match std::fs::symlink_metadata(&at) {
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => return Err(failed("cannot be looked at", err)),
        // A link is an install's: this one's, or one that points where
        // Quill used to be.
        Ok(found) if found.file_type().is_symlink() => {
            std::fs::remove_file(&at)
                .map_err(|err| failed("the old link cannot be removed", err))?;
        }
        Ok(found) if found.is_dir() && is_our_copy(&at) => {
            std::fs::remove_dir_all(&at)
                .map_err(|err| failed("the old copy cannot be removed", err))?;
        }
        Ok(_) => {
            return Err(format!(
                "{} is there already and is not Quill's skill. Move it away first",
                at.display()
            ));
        }
    }
    match shipped.filter(|_| !copy) {
        Some(to) => {
            std::os::unix::fs::symlink(to, &at).map_err(|err| failed("cannot be linked", err))?;
            Ok(Put::Linked {
                at,
                to: to.to_path_buf(),
            })
        }
        None => {
            std::fs::create_dir(&at).map_err(|err| failed("cannot be made", err))?;
            std::fs::write(at.join("SKILL.md"), SKILL)
                .map_err(|err| failed("cannot be written", err))?;
            Ok(Put::Copied { at })
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use quill_tui::agent::tools::TOOLS;

    use super::*;

    /// The lines between the two `---` at the top, as `key: value`.
    fn frontmatter() -> Vec<(String, String)> {
        let rest = SKILL
            .strip_prefix("---\n")
            .expect("the skill starts with ---");
        let (head, _) = rest.split_once("\n---\n").expect("and closes it");
        head.lines()
            .map(|line| {
                let (key, value) = line.split_once(": ").expect(line);
                (key.to_owned(), value.to_owned())
            })
            .collect()
    }

    #[test]
    fn the_skill_is_in_the_form_agents_share() {
        let fields = frontmatter();
        let get = |key: &str| {
            fields
                .iter()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value.as_str())
        };
        // Its name is its folder's, in the letters a name may have.
        assert_eq!(get("name"), Some(NAME));
        assert!(
            NAME.len() <= 64
                && NAME
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        );
        let description = get("description").unwrap();
        assert!(!description.is_empty() && description.chars().count() <= 1024);
        // What it does, and when to reach for it.
        assert!(description.contains("email") && description.contains("Use it when"));
        assert!(get("compatibility").unwrap().chars().count() <= 500);
        // Only fields every agent's program takes.
        for (key, _) in &fields {
            assert!(
                [
                    "name",
                    "description",
                    "compatibility",
                    "license",
                    "metadata"
                ]
                .contains(&key.as_str()),
                "{key}"
            );
        }
        // No command is approved ahead of the user: what may be run is
        // theirs to say.
        assert!(!SKILL.contains("allowed-tools"));
    }

    #[test]
    fn every_command_there_is_is_in_the_skill() {
        for tool in TOOLS {
            let command = format!("quill {}", tool.name.replace('_', " "));
            assert!(
                SKILL.contains(&command),
                "the skill never shows `{command}`"
            );
        }
        // And every way of being refused.
        for code in [
            "switched_off",
            "over_limit",
            "not_found",
            "invalid",
            "cannot_undo",
            "not_running",
            "failed",
        ] {
            assert!(SKILL.contains(&format!("`{code}`")), "{code}");
        }
        // The rules that matter most are said.
        for said in ["untrusted", "--dry-run", "Do not look for another way"] {
            assert!(SKILL.contains(said), "{said}");
        }
    }

    #[test]
    fn the_skill_is_linked_to_the_one_that_ships_and_the_link_is_kept_current() {
        let dir = tempfile::tempdir().unwrap();
        let shipped = dir.path().join("Quill.app/Contents/Resources/skills/quill");
        std::fs::create_dir_all(&shipped).unwrap();
        std::fs::write(shipped.join("SKILL.md"), SKILL).unwrap();
        let skills = dir.path().join("home/.claude/skills");

        let put = install(&skills, Some(&shipped), false).unwrap();
        let at = skills.join("quill");
        assert_eq!(
            put,
            Put::Linked {
                at: at.clone(),
                to: shipped.clone()
            }
        );
        assert!(
            std::fs::symlink_metadata(&at)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        // Read through the link, it is the skill.
        assert_eq!(std::fs::read_to_string(at.join("SKILL.md")).unwrap(), SKILL);

        // Quill moves: the link is made again, where it is now.
        let moved = dir.path().join("elsewhere/skills/quill");
        std::fs::create_dir_all(&moved).unwrap();
        std::fs::write(moved.join("SKILL.md"), SKILL).unwrap();
        std::fs::remove_dir_all(dir.path().join("Quill.app")).unwrap();
        install(&skills, Some(&moved), false).unwrap();
        assert_eq!(std::fs::read_link(&at).unwrap(), moved);
        // What the link pointed at was never touched.
        assert!(moved.join("SKILL.md").is_file());
    }

    #[test]
    fn with_nothing_to_link_to_or_when_asked_the_skill_is_copied() {
        let dir = tempfile::tempdir().unwrap();
        let skills = dir.path().join(".agents/skills");
        let put = install(&skills, None, false).unwrap();
        let at = skills.join("quill");
        assert_eq!(put, Put::Copied { at: at.clone() });
        assert!(
            at.is_dir()
                && !std::fs::symlink_metadata(&at)
                    .unwrap()
                    .file_type()
                    .is_symlink()
        );
        assert_eq!(std::fs::read_to_string(at.join("SKILL.md")).unwrap(), SKILL);

        // A copy is brought up to date by installing again, and may
        // become a link, or a link a copy.
        std::fs::write(at.join("SKILL.md"), format!("---\nname: {NAME}\nold")).unwrap();
        install(&skills, None, true).unwrap();
        assert_eq!(std::fs::read_to_string(at.join("SKILL.md")).unwrap(), SKILL);
        let shipped = dir.path().join("shipped/quill");
        std::fs::create_dir_all(&shipped).unwrap();
        std::fs::write(shipped.join("SKILL.md"), SKILL).unwrap();
        assert!(matches!(
            install(&skills, Some(&shipped), false).unwrap(),
            Put::Linked { .. }
        ));
        assert!(matches!(
            install(&skills, Some(&shipped), true).unwrap(),
            Put::Copied { .. }
        ));
        // The copy took nothing from what ships.
        assert!(shipped.join("SKILL.md").is_file());
    }

    #[test]
    fn what_is_not_quills_is_never_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let skills = dir.path().join("skills");
        // A skill of the same name that someone wrote, with a file of
        // their own beside it.
        let theirs = skills.join("quill");
        std::fs::create_dir_all(&theirs).unwrap();
        std::fs::write(theirs.join("SKILL.md"), format!("---\nname: {NAME}\nmine")).unwrap();
        std::fs::write(theirs.join("notes.md"), "mine").unwrap();
        let refused = install(&skills, None, true).unwrap_err();
        assert!(refused.contains("not Quill's skill"), "{refused}");
        assert_eq!(
            std::fs::read_to_string(theirs.join("notes.md")).unwrap(),
            "mine"
        );
        // Or one that only shares the name, or a plain file.
        std::fs::remove_file(theirs.join("notes.md")).unwrap();
        std::fs::write(theirs.join("SKILL.md"), "---\nname: quill-pens\n---\n").unwrap();
        assert!(install(&skills, None, true).is_err());
        std::fs::remove_dir_all(&theirs).unwrap();
        std::fs::write(&theirs, "a file").unwrap();
        assert!(install(&skills, None, false).is_err());
        assert_eq!(std::fs::read_to_string(&theirs).unwrap(), "a file");
    }

    #[test]
    fn the_skill_that_ships_is_found_beside_the_program() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        // In the app's bundle.
        let helpers = root.join("Quill.app/Contents/Helpers");
        let bundled = root.join("Quill.app/Contents/Resources/skills/quill");
        std::fs::create_dir_all(&helpers).unwrap();
        std::fs::create_dir_all(&bundled).unwrap();
        assert_eq!(shipped_beside(&helpers.join("quill")), None);
        std::fs::write(bundled.join("SKILL.md"), SKILL).unwrap();
        assert_eq!(shipped_beside(&helpers.join("quill")), Some(bundled));
        // In a checkout.
        let release = root.join("repo/target/release");
        let checked_out = root.join("repo/skills/quill");
        std::fs::create_dir_all(&release).unwrap();
        std::fs::create_dir_all(&checked_out).unwrap();
        std::fs::write(checked_out.join("SKILL.md"), SKILL).unwrap();
        assert_eq!(shipped_beside(&release.join("quill")), Some(checked_out));
        // By itself somewhere, there is none.
        let alone = root.join("bin");
        std::fs::create_dir_all(&alone).unwrap();
        assert_eq!(shipped_beside(&alone.join("quill")), None);
        assert_eq!(
            usual_places(Path::new("/Users/robin")),
            [
                Path::new("/Users/robin/.claude/skills"),
                Path::new("/Users/robin/.agents/skills")
            ]
        );
    }
}
