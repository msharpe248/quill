//! Changing the user's own keys in `keys.toml`, for the settings window.
//!
//! The file is theirs: a table from a command's name to a key or a list
//! of keys, with whatever comments they wrote. A change is made to the
//! text as it stands and touches only the lines of the commands it is
//! about. A file Quill could not read is never written to, and nothing is
//! handed back that Quill could not read again.

use quill_tui::command::Registry;
use quill_tui::keymap::{KeySeq, Keymap};
use quill_tui::settings::KeysChange;
use toml_edit::{Array, DocumentMut, value};

/// `text` with `change` made to it. `preset` is the keys the file is laid
/// over: a command given exactly the keys it comes with has nothing
/// written of it, so the file says only what the user changed.
pub fn apply(
    text: &str,
    change: &KeysChange,
    preset: &Keymap,
    registry: &Registry,
) -> Result<String, String> {
    // What cannot be read is not written over: there is something in it
    // the user meant, and only they know what.
    preset
        .clone()
        .with_overrides(text, registry)
        .map_err(|err| format!("keys.toml has a mistake to put right first: {err}"))?;
    let mut document: DocumentMut = text
        .parse()
        .map_err(|err| format!("keys.toml cannot be read: {err}"))?;
    match change {
        KeysChange::Set(sets) => {
            for (name, keys) in sets {
                let spec = registry
                    .by_name(name)
                    .ok_or_else(|| format!("There is no command called {name}."))?;
                let sequences = keys
                    .iter()
                    .map(|key| {
                        key.parse::<KeySeq>()
                            .map_err(|err| format!("{key} is not a key Quill can use: {err}."))
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                if sequences.as_slice() == preset.keys_for(spec.id) {
                    take_out(&mut document, name);
                } else if let [only] = keys.as_slice() {
                    document[name.as_str()] = value(only.as_str());
                } else {
                    let mut list = Array::new();
                    for key in keys {
                        list.push(key.as_str());
                    }
                    document[name.as_str()] = value(list);
                }
            }
        }
        KeysChange::Reset(names) => {
            for name in names {
                take_out(&mut document, name);
            }
        }
        KeysChange::ResetAll => {
            let names: Vec<String> = document.iter().map(|(name, _)| name.to_owned()).collect();
            for name in names {
                take_out(&mut document, &name);
            }
        }
    }
    let after = document.to_string();
    // Better to change nothing than to write keys that will not load.
    preset
        .clone()
        .with_overrides(&after, registry)
        .map_err(|err| format!("That would leave keys.toml unreadable: {err}"))?;
    Ok(after)
}

/// Takes a command's line out of the file, with the comment written right
/// above it, which is about it. What stands apart above that, with an
/// empty line between, is about the file or what follows, and stays: it
/// goes to the line that comes next, or to the foot of the file.
fn take_out(document: &mut DocumentMut, name: &str) {
    let Some((key, _)) = document.get_key_value(name) else {
        return;
    };
    let above = key
        .leaf_decor()
        .prefix()
        .and_then(|written| written.as_str())
        .unwrap_or_default();
    let apart = above
        .rfind("\n\n")
        .map(|at| above[..at + 2].to_owned())
        .unwrap_or_default();
    let next: Option<String> = document
        .iter()
        .map(|(name, _)| name.to_owned())
        .skip_while(|other| other != name)
        .nth(1);
    document.remove(name);
    if apart.is_empty() {
        return;
    }
    match next.and_then(|next| document.key_mut(&next)) {
        Some(mut next) => {
            let own = next
                .leaf_decor()
                .prefix()
                .and_then(|written| written.as_str())
                .unwrap_or_default()
                .to_owned();
            next.leaf_decor_mut().set_prefix(format!("{apart}{own}"));
        }
        None => {
            let foot = document.trailing().as_str().unwrap_or_default().to_owned();
            document.set_trailing(format!("{apart}{foot}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use quill_core::config::KeymapPreset;
    use quill_tui::command::CommandId;

    use super::*;

    fn rig() -> (Keymap, Registry) {
        (Keymap::preset(KeymapPreset::Vim), Registry::builtin())
    }

    fn set(name: &str, keys: &[&str]) -> KeysChange {
        KeysChange::Set(vec![(
            name.to_owned(),
            keys.iter().map(|key| (*key).to_owned()).collect(),
        )])
    }

    fn keys_of(text: &str, id: CommandId) -> Vec<String> {
        let (preset, registry) = rig();
        preset
            .with_overrides(text, &registry)
            .unwrap()
            .keys_for(id)
            .iter()
            .map(ToString::to_string)
            .collect()
    }

    #[test]
    fn a_key_is_written_for_its_command_and_everything_else_is_kept() {
        let (preset, registry) = rig();
        let before = "# My keys.\n\"view.zoom\" = \"Z\" # bigger\n";
        // One key is a string, several are a list, none is an empty list.
        let after = apply(before, &set("message.move", &["M"]), &preset, &registry).unwrap();
        let after = apply(
            &after,
            &set("nav.next", &["j", "down", "ctrl-n"]),
            &preset,
            &registry,
        )
        .unwrap();
        let after = apply(&after, &set("message.toggle_star", &[]), &preset, &registry).unwrap();
        assert!(
            after.starts_with("# My keys.\n\"view.zoom\" = \"Z\" # bigger\n"),
            "{after}"
        );
        assert!(after.contains("\"message.move\" = \"M\""), "{after}");
        assert!(
            after.contains("\"nav.next\" = [\"j\", \"down\", \"ctrl-n\"]"),
            "{after}"
        );
        assert!(after.contains("\"message.toggle_star\" = []"), "{after}");
        assert_eq!(keys_of(&after, CommandId::MessageMove), ["M"]);
        assert_eq!(keys_of(&after, CommandId::NavNext), ["j", "down", "ctrl-n"]);
        assert!(keys_of(&after, CommandId::MessageToggleStar).is_empty());
        assert_eq!(keys_of(&after, CommandId::ViewZoom), ["Z"]);

        // A key written again replaces what was written.
        let again = apply(&after, &set("message.move", &["g m"]), &preset, &registry).unwrap();
        assert_eq!(again.matches("message.move").count(), 1, "{again}");
        assert_eq!(keys_of(&again, CommandId::MessageMove), ["g m"]);
    }

    #[test]
    fn a_command_given_the_keys_it_comes_with_has_nothing_written_of_it() {
        let (preset, registry) = rig();
        let before = "\"message.move\" = \"M\"\n\"view.zoom\" = \"Z\"\n";
        let after = apply(before, &set("message.move", &["m"]), &preset, &registry).unwrap();
        assert_eq!(after, "\"view.zoom\" = \"Z\"\n");
        // Nor is one that was never changed.
        let after = apply(&after, &set("nav.next", &["j", "down"]), &preset, &registry).unwrap();
        assert_eq!(after, "\"view.zoom\" = \"Z\"\n");
    }

    #[test]
    fn a_command_is_put_back_and_so_are_all_of_them() {
        let (preset, registry) = rig();
        // A comment right above a key is about that key and goes with it.
        // One that stands apart, at the head of the file, stays.
        let before = "# Mine.\n\n# For filing.\n\"message.move\" = \"M\"\n\"view.zoom\" = \"Z\"\n";
        let one = apply(
            before,
            &KeysChange::Reset(vec!["message.move".to_owned()]),
            &preset,
            &registry,
        )
        .unwrap();
        assert_eq!(one, "# Mine.\n\n\"view.zoom\" = \"Z\"\n");
        assert_eq!(keys_of(&one, CommandId::MessageMove), ["m"]);
        // One that was not changed is put back to what it is.
        let same = apply(
            &one,
            &KeysChange::Reset(vec!["nav.next".to_owned()]),
            &preset,
            &registry,
        )
        .unwrap();
        assert_eq!(same, one);

        let all = apply(before, &KeysChange::ResetAll, &preset, &registry).unwrap();
        assert!(
            !all.contains("message.move") && !all.contains("view.zoom"),
            "{all}"
        );
        assert_eq!(keys_of(&all, CommandId::ViewZoom), ["z", "ctrl-z"]);
        // From no file at all, the first key makes one.
        let first = apply("", &set("message.move", &["M"]), &preset, &registry).unwrap();
        assert_eq!(first, "\"message.move\" = \"M\"\n");
    }

    #[test]
    fn what_would_not_load_is_not_written_and_a_file_with_a_mistake_is_left_alone() {
        let (preset, registry) = rig();
        let good = "\"view.zoom\" = \"Z\"\n";
        let no_command = apply(good, &set("no.such.command", &["x"]), &preset, &registry);
        assert!(no_command.unwrap_err().contains("no command called"));
        let no_key = apply(good, &set("message.move", &["ctrl-"]), &preset, &registry);
        assert!(no_key.unwrap_err().contains("not a key Quill can use"));
        // The user's file says something Quill cannot read: it is theirs
        // to put right, and is not written over.
        for mistaken in ["\"no.such.command\" = \"x\"\n", "\"view.zoom\" = \n"] {
            let refused = apply(mistaken, &set("message.move", &["M"]), &preset, &registry);
            assert!(
                refused
                    .unwrap_err()
                    .contains("a mistake to put right first"),
                "{mistaken:?}"
            );
            let refused = apply(mistaken, &KeysChange::ResetAll, &preset, &registry);
            assert!(refused.is_err(), "{mistaken:?}");
        }
    }
}
