//! A server for the unit tests that says exactly what its script says.
//!
//! It is for what a real server will not do on request: refuse a search,
//! leave out a response code, change its mind between two commands, or stop
//! answering. It fails the test if it is sent anything the script did not
//! expect, so a script is also a statement of every command that may be
//! sent.

use std::time::Duration;

use async_imap::{Client, Session};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream};
use tokio::task::JoinHandle;

use crate::wait::Bounded;

/// A signed-in session with a server that follows `script`: for each line
/// it expects (without the tag), what it answers, with `{tag}` standing for
/// the tag of the command in progress. An empty answer is a server that has
/// stopped talking. Join the handle after dropping the session to check the
/// whole script was played.
pub(crate) async fn scripted(
    script: &[(&str, &str)],
) -> (Session<Bounded<DuplexStream>>, JoinHandle<()>) {
    scripted_within(script, Duration::from_secs(10)).await
}

/// As [`scripted`], with a limit of its own on the server's silence.
pub(crate) async fn scripted_within(
    script: &[(&str, &str)],
    patience: Duration,
) -> (Session<Bounded<DuplexStream>>, JoinHandle<()>) {
    let (ours, theirs) = tokio::io::duplex(64 * 1024);
    let mut steps = vec![(
        "LOGIN \"robin\" \"secret\"".to_owned(),
        "{tag} OK\r\n".to_owned(),
    )];
    steps.extend(
        script
            .iter()
            .map(|(expect, answer)| ((*expect).to_owned(), (*answer).to_owned())),
    );
    let server = tokio::spawn(async move {
        let (read, mut write) = tokio::io::split(theirs);
        let mut lines = BufReader::new(read);
        let mut tag = String::new();
        // After a go-ahead the next line continues a command and has no
        // tag of its own.
        let mut continuing = false;
        for (expect, answer) in steps {
            let mut line = Vec::new();
            lines.read_until(b'\n', &mut line).await.unwrap();
            let line = String::from_utf8(line).unwrap();
            assert!(!line.is_empty(), "the client hung up before {expect:?}");
            let line = line.trim_end_matches("\r\n");
            let said = if continuing {
                line
            } else {
                let (theirs, rest) = line.split_once(' ').unwrap();
                tag = theirs.to_owned();
                rest
            };
            assert_eq!(said, expect);
            // A go-ahead need not come first: a server may have other things
            // to say before it.
            continuing = answer.starts_with('+') || answer.contains("\r\n+");
            write
                .write_all(answer.replace("{tag}", &tag).as_bytes())
                .await
                .unwrap();
        }
        let mut extra = Vec::new();
        lines.read_until(b'\n', &mut extra).await.unwrap();
        assert_eq!(String::from_utf8_lossy(&extra), "", "after the script");
    });
    let session = Client::new(Bounded::new(ours, patience))
        .login("robin", "secret")
        .await
        .map_err(|(err, _)| err)
        .unwrap();
    (session, server)
}

/// Several stretches of script, end to end.
pub(crate) fn steps<'a>(parts: &[&[(&'a str, &'a str)]]) -> Vec<(&'a str, &'a str)> {
    parts.iter().flat_map(|part| part.iter().copied()).collect()
}
