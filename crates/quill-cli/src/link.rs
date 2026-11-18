//! Putting `quill` on the user's `PATH`: Install Command Line Tool.
//!
//! The command line is inside the app (`Quill.app/Contents/Helpers/quill`),
//! which is not a place a shell looks. This makes a link to it, called
//! `quill`, in a folder a shell does look in, so that `quill search ...`
//! works at a prompt, for a script and for an agent's skill. It is a link,
//! so it is always the installed Quill's own command line.
//!
//! Nothing here asks for an administrator's password. The link goes in the
//! first usual folder the user can write to themselves, and when that is
//! one a shell does not look in until told to, what is said afterwards
//! says how to tell it.

use std::path::{Path, PathBuf};

/// What the link is called.
pub const NAME: &str = "quill";

/// Names the one folder to link into, in place of the usual ones: for a
/// run that must not touch the folders everyone's programs are in.
pub const DIR_VARIABLE: &str = "QUILL_LINK_DIR";

/// A folder the link may go in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Place {
    pub dir: PathBuf,
    /// Make the folder if it is not there. The system's own folders are
    /// not made: only an administrator can, and nothing here is one.
    pub make: bool,
    /// A shell looks here without being told to.
    pub on_path: bool,
}

/// Where the link may go, the best place first: the folder that was
/// named, if one was; otherwise the folders programs are usually linked
/// into on a Mac, then one of the user's own.
pub fn places(given: Option<PathBuf>, home: Option<&Path>, path: Option<&str>) -> Vec<Place> {
    let looked_in =
        |dir: &Path| path.is_some_and(|path| std::env::split_paths(path).any(|entry| entry == dir));
    if let Some(dir) = given {
        let on_path = looked_in(&dir);
        return vec![Place {
            dir,
            make: true,
            on_path,
        }];
    }
    let mut places = vec![
        // On every Mac's `PATH` from the start.
        Place {
            dir: PathBuf::from("/usr/local/bin"),
            make: false,
            on_path: true,
        },
        // Homebrew's, on an Apple silicon Mac: on the `PATH` of anyone
        // who has set Homebrew up, which is how the folder comes to be.
        Place {
            dir: PathBuf::from("/opt/homebrew/bin"),
            make: false,
            on_path: true,
        },
    ];
    if let Some(home) = home {
        let dir = home.join(".local/bin");
        let on_path = looked_in(&dir);
        places.push(Place {
            dir,
            make: true,
            on_path,
        });
    }
    places
}

/// What was done.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Linked {
    /// The link.
    pub at: PathBuf,
    /// It was there already, pointing at this Quill.
    pub already: bool,
    /// A shell looks in its folder without being told to.
    pub on_path: bool,
}

impl Linked {
    /// What to tell the user, in a line short enough for a status bar.
    /// A folder under `home` is written as the user would write it.
    pub fn said(&self, home: Option<&Path>) -> String {
        let under_home =
            |path: &Path, as_home: &str| match home.and_then(|home| path.strip_prefix(home).ok()) {
                Some(rest) => format!("{as_home}/{}", rest.display()),
                None => path.display().to_string(),
            };
        let at = under_home(&self.at, "~");
        match (self.already, self.on_path) {
            (true, true) => format!("quill is already installed, at {at}."),
            (false, true) => {
                format!("Installed at {at}: in a new terminal window, quill is this Quill.")
            }
            // Where a shell does not look until it is told to: how to
            // tell it comes first, since it is what there is to do.
            (_, false) => {
                let folder = under_home(self.at.parent().unwrap_or(Path::new("")), "$HOME");
                format!(
                    "Linked at {at}. To use it, add to ~/.zprofile: export PATH=\"{folder}:$PATH\""
                )
            }
        }
    }
}

/// Why a program is no place to link to: it is where it will not stay.
fn passing(program: &Path) -> Option<&'static str> {
    let text = program.to_string_lossy();
    if text.starts_with("/Volumes/") {
        Some(
            "Quill is running from its disk image. Drag it to Applications and open it from \
             there first",
        )
    } else if text.contains("/AppTranslocation/") {
        Some(
            "macOS is running Quill from a temporary place. Move Quill to Applications, open \
             it from there, and try again",
        )
    } else {
        None
    }
}

/// Whether the link at `at` is one this made, now or for a Quill that was
/// somewhere else: it points at a program called `quill`, or at nothing.
fn is_ours(at: &Path) -> bool {
    match std::fs::read_link(at) {
        Ok(target) => target.file_name().is_some_and(|name| name == NAME) || !at.exists(),
        Err(_) => false,
    }
}

/// Links `program`, this Quill's command line, into the first of `places`
/// that can be written to, as `quill`.
///
/// What is there already under that name is replaced only if it is a link
/// to a `quill`, or to nothing: this Quill's, or one that has moved.
/// Anything else is somebody's program, and stops the install, since a
/// link further along the `PATH` would never be the one a shell found.
pub fn install(program: &Path, places: &[Place]) -> Result<Linked, String> {
    if let Some(why) = passing(program) {
        return Err(why.to_owned());
    }
    let mut denied = Vec::new();
    for place in places {
        if !place.dir.is_dir() {
            if !place.make {
                continue;
            }
            if let Err(err) = std::fs::create_dir_all(&place.dir) {
                denied.push(format!("{} ({err})", place.dir.display()));
                continue;
            }
        }
        let at = place.dir.join(NAME);
        if std::fs::symlink_metadata(&at).is_ok() {
            if std::fs::read_link(&at).is_ok_and(|target| target == program) {
                return Ok(Linked {
                    at,
                    already: true,
                    on_path: place.on_path,
                });
            }
            if !is_ours(&at) {
                return Err(format!(
                    "{} is there already and is not a link to Quill. Move it away first",
                    at.display()
                ));
            }
            match std::fs::remove_file(&at) {
                Ok(()) => {}
                Err(err) if err.kind() == std::io::ErrorKind::PermissionDenied => {
                    denied.push(format!("{} ({err})", place.dir.display()));
                    continue;
                }
                Err(err) => return Err(format!("{}: {err}", at.display())),
            }
        }
        match std::os::unix::fs::symlink(program, &at) {
            Ok(()) => {
                return Ok(Linked {
                    at,
                    already: false,
                    on_path: place.on_path,
                });
            }
            // Not the user's to write in: the next folder is tried.
            Err(err) if err.kind() == std::io::ErrorKind::PermissionDenied => {
                denied.push(format!("{} ({err})", place.dir.display()));
            }
            Err(err) => return Err(format!("{}: {err}", at.display())),
        }
    }
    Err(if denied.is_empty() {
        "There is no folder to put the link in".to_owned()
    } else {
        format!(
            "No folder could be written to: {}. Name one with `quill link --to DIR`",
            denied.join(", ")
        )
    })
}

/// Installs this Quill's own command line, where the environment says or
/// in the usual places, and says what to tell the user.
pub fn install_this(given: Option<PathBuf>) -> Result<String, String> {
    let program = std::env::current_exe()
        .and_then(|program| program.canonicalize())
        .map_err(|err| format!("Quill cannot tell where it is: {err}"))?;
    let given = given.or_else(|| std::env::var_os(DIR_VARIABLE).map(PathBuf::from));
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let path = std::env::var("PATH").ok();
    let linked = install(&program, &places(given, home.as_deref(), path.as_deref()))?;
    Ok(linked.said(home.as_deref()))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    struct Rig {
        dir: tempfile::TempDir,
        program: PathBuf,
    }

    impl Rig {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let helpers = dir.path().join("Quill.app/Contents/Helpers");
            std::fs::create_dir_all(&helpers).unwrap();
            let program = helpers.join("quill");
            std::fs::write(&program, "the program").unwrap();
            Self { dir, program }
        }

        fn place(&self, name: &str, make: bool, on_path: bool) -> Place {
            Place {
                dir: self.dir.path().join(name),
                make,
                on_path,
            }
        }
    }

    #[test]
    fn the_link_goes_in_the_first_folder_there_is_and_says_where() {
        let rig = Rig::new();
        // The system's folder is not there and is not made; Homebrew's is.
        let system = rig.place("usr-local-bin", false, true);
        let brew = rig.place("homebrew-bin", false, true);
        std::fs::create_dir_all(&brew.dir).unwrap();
        let own = rig.place("home/.local/bin", true, false);
        let places = [system.clone(), brew.clone(), own];

        let linked = install(&rig.program, &places).unwrap();
        assert_eq!(linked.at, brew.dir.join("quill"));
        assert!(!linked.already && linked.on_path);
        assert_eq!(std::fs::read_link(&linked.at).unwrap(), rig.program);
        assert_eq!(std::fs::read_to_string(&linked.at).unwrap(), "the program");
        assert!(!system.dir.exists());
        assert!(linked.said(None).starts_with("Installed at "));

        // Asked again, there is nothing to do, and it says so.
        let again = install(&rig.program, &places).unwrap();
        assert!(again.already);
        assert!(again.said(None).starts_with("quill is already installed"));
    }

    #[test]
    fn a_folder_that_cannot_be_written_to_is_passed_over_for_the_users_own() {
        let rig = Rig::new();
        let locked = rig.place("locked", false, true);
        std::fs::create_dir_all(&locked.dir).unwrap();
        std::fs::set_permissions(&locked.dir, std::fs::Permissions::from_mode(0o555)).unwrap();
        let own = rig.place("home/.local/bin", true, false);

        let linked = install(&rig.program, &[locked.clone(), own.clone()]).unwrap();
        assert_eq!(linked.at, own.dir.join("quill"));
        // A folder a shell is not looking in yet: how to tell it is said.
        let said = linked.said(None);
        assert!(
            said.contains("~/.zprofile") && said.contains("export PATH="),
            "{said}"
        );
        assert!(said.contains(&own.dir.display().to_string()));
        // Written as the user would write it, and short enough to be read
        // in a status bar.
        let home = rig.dir.path().join("home");
        assert_eq!(
            linked.said(Some(&home)),
            "Linked at ~/.local/bin/quill. To use it, add to ~/.zprofile: export \
             PATH=\"$HOME/.local/bin:$PATH\""
        );

        // With nowhere at all, the folders that were tried are named.
        std::fs::remove_file(&linked.at).unwrap();
        let refused = install(&rig.program, std::slice::from_ref(&locked)).unwrap_err();
        assert!(
            refused.contains("locked") && refused.contains("--to"),
            "{refused}"
        );
        std::fs::set_permissions(&locked.dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[test]
    fn a_link_to_a_quill_that_moved_is_made_again_and_nothing_else_is_replaced() {
        let rig = Rig::new();
        let bin = rig.place("bin", true, true);
        std::fs::create_dir_all(&bin.dir).unwrap();
        let at = bin.dir.join("quill");

        // A link left by a Quill that is gone.
        std::os::unix::fs::symlink(rig.dir.path().join("old/Helpers/quill"), &at).unwrap();
        let linked = install(&rig.program, std::slice::from_ref(&bin)).unwrap();
        assert!(!linked.already);
        assert_eq!(std::fs::read_link(&at).unwrap(), rig.program);

        // Somebody's own program of the same name stops it, and is left.
        std::fs::remove_file(&at).unwrap();
        std::fs::write(&at, "another quill").unwrap();
        let later = rig.place("later", true, true);
        let refused = install(&rig.program, &[bin.clone(), later.clone()]).unwrap_err();
        assert!(refused.contains("not a link to Quill"), "{refused}");
        assert_eq!(std::fs::read_to_string(&at).unwrap(), "another quill");
        // And no link is made further along, where a shell would never
        // find it first.
        assert!(!later.dir.join("quill").exists());

        // So does a link to some other program.
        std::fs::remove_file(&at).unwrap();
        let other = rig.dir.path().join("pen");
        std::fs::write(&other, "pen").unwrap();
        std::os::unix::fs::symlink(&other, &at).unwrap();
        assert!(install(&rig.program, std::slice::from_ref(&bin)).is_err());
        assert_eq!(std::fs::read_link(&at).unwrap(), other);
    }

    #[test]
    fn a_quill_that_will_not_stay_where_it_is_is_not_linked_to() {
        let rig = Rig::new();
        let bin = rig.place("bin", true, true);
        for program in [
            "/Volumes/Quill/Quill.app/Contents/Helpers/quill",
            "/private/var/folders/x/T/AppTranslocation/A1/d/Quill.app/Contents/Helpers/quill",
        ] {
            let refused = install(Path::new(program), std::slice::from_ref(&bin)).unwrap_err();
            assert!(refused.contains("Applications"), "{refused}");
        }
        assert!(!bin.dir.exists());
    }

    #[test]
    fn the_places_are_the_usual_ones_unless_one_is_named() {
        let home = Path::new("/Users/robin");
        let usual = places(None, Some(home), Some("/usr/bin:/bin"));
        let dirs: Vec<&Path> = usual.iter().map(|place| place.dir.as_path()).collect();
        assert_eq!(
            dirs,
            [
                Path::new("/usr/local/bin"),
                Path::new("/opt/homebrew/bin"),
                Path::new("/Users/robin/.local/bin")
            ]
        );
        // Only the user's own folder is made, and it is on the `PATH`
        // only if the `PATH` says so.
        assert_eq!(
            usual.iter().map(|place| place.make).collect::<Vec<_>>(),
            [false, false, true]
        );
        assert!(!usual[2].on_path);
        let with = places(None, Some(home), Some("/usr/bin:/Users/robin/.local/bin"));
        assert!(with[2].on_path);

        let named = places(
            Some(PathBuf::from("/tmp/bin")),
            Some(home),
            Some("/tmp/bin"),
        );
        assert_eq!(
            named,
            [Place {
                dir: PathBuf::from("/tmp/bin"),
                make: true,
                on_path: true
            }]
        );
    }
}
