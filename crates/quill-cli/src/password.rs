//! Reading a password from the person at the keyboard.

use std::io::{self, BufRead, IsTerminal, Write};

use rustix::termios::{LocalModes, OptionalActions, tcgetattr, tcsetattr};

/// Prompts on stderr and reads one line from stdin. At a terminal the typing
/// is not echoed. Piped input is read as it comes, so a script can supply
/// the password without it ever appearing in an argument list.
pub fn read(prompt: &str) -> io::Result<String> {
    let stdin = io::stdin();
    if !stdin.is_terminal() {
        let mut line = String::new();
        stdin.lock().read_line(&mut line)?;
        return Ok(strip_newline(line));
    }

    eprint!("{prompt}");
    io::stderr().flush()?;
    let original = tcgetattr(&stdin)?;
    let mut silent = original.clone();
    silent.local_modes.remove(LocalModes::ECHO);
    tcsetattr(&stdin, OptionalActions::Now, &silent)?;
    let mut line = String::new();
    let read = stdin.lock().read_line(&mut line);
    // Whatever happened, the terminal gets its echo back.
    let restored = tcsetattr(&stdin, OptionalActions::Now, &original);
    eprintln!();
    read?;
    restored?;
    Ok(strip_newline(line))
}

fn strip_newline(mut line: String) -> String {
    while line.ends_with('\n') || line.ends_with('\r') {
        line.pop();
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_line_ending_is_stripped() {
        assert_eq!(strip_newline("hunter2\n".to_owned()), "hunter2");
        assert_eq!(strip_newline("hunter2\r\n".to_owned()), "hunter2");
        assert_eq!(
            strip_newline("  spaces kept  \n".to_owned()),
            "  spaces kept  "
        );
        assert_eq!(strip_newline(String::new()), "");
    }
}
