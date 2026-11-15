//! The sync cursor: which messages have already been delivered.
//!
//! POP3 has no change stream. All a server offers is `UIDL`, the list of
//! what is in the mailbox now, so the cursor has to remember every unique
//! id already delivered and each sync compares the two lists.
//!
//! A mailbox can hold tens of thousands of messages and the cursor is
//! written to the store after every sync, so it is kept small: the ids are
//! sorted, each is written as the part that differs from the one before it
//! (servers hand out ids that run in sequence or share long prefixes), and
//! the result is compressed with DEFLATE and written as base64. Sorting
//! also makes the text canonical: the same set of ids always gives the same
//! cursor, so an unchanged mailbox gives an unchanged cursor.

use std::collections::BTreeSet;
use std::io::{Read, Write};

use base64::Engine;
use base64::engine::general_purpose::STANDARD_NO_PAD;
use flate2::Compression;
use flate2::read::DeflateDecoder;
use flate2::write::DeflateEncoder;
use quill_core::types::SyncCursor;

/// The unique ids of the messages already delivered.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Cursor {
    pub seen: BTreeSet<String>,
}

/// The most a cursor may unpack to. Fifty thousand ids of the longest kind
/// RFC 1939 allows come to under four megabytes; anything near this limit
/// is not a cursor this crate wrote.
const MAX_UNPACKED: u64 = 256 * 1024 * 1024;

impl Cursor {
    /// Version 1: `1;` then base64 (no padding) of the DEFLATE of the list.
    /// The list is, for each id in sorted order: how many leading bytes it
    /// shares with the id before it, how many bytes follow, and those
    /// bytes. Both counts are LEB128 varints.
    const VERSION: &'static str = "1";

    pub fn encode(&self) -> SyncCursor {
        let mut list = Vec::new();
        let mut previous: &[u8] = &[];
        for id in &self.seen {
            let id = id.as_bytes();
            let shared = previous
                .iter()
                .zip(id)
                .take_while(|(before, now)| before == now)
                .count();
            put_varint(&mut list, shared);
            put_varint(&mut list, id.len() - shared);
            list.extend_from_slice(&id[shared..]);
            previous = id;
        }
        let mut encoder = DeflateEncoder::new(Vec::new(), Compression::best());
        // Writing to a Vec cannot fail. If it somehow did, the cursor would
        // not decode, and a cursor that does not decode means a fresh sync.
        let packed = encoder
            .write_all(&list)
            .and_then(|()| encoder.finish())
            .unwrap_or_default();
        SyncCursor(format!(
            "{};{}",
            Self::VERSION,
            STANDARD_NO_PAD.encode(packed)
        ))
    }

    /// `None` for a cursor this version did not write, or one that has been
    /// damaged. The caller then treats every message on the server as new,
    /// which is always safe: nothing is reported as removed.
    pub fn decode(cursor: &SyncCursor) -> Option<Self> {
        let packed = cursor.0.strip_prefix(Self::VERSION)?.strip_prefix(';')?;
        let packed = STANDARD_NO_PAD.decode(packed).ok()?;
        let mut list = Vec::new();
        DeflateDecoder::new(packed.as_slice())
            .take(MAX_UNPACKED + 1)
            .read_to_end(&mut list)
            .ok()?;
        if list.len() as u64 > MAX_UNPACKED {
            return None;
        }

        let mut seen = BTreeSet::new();
        let mut previous: Vec<u8> = Vec::new();
        let mut rest = list.as_slice();
        while !rest.is_empty() {
            let shared = take_varint(&mut rest)?;
            let length = take_varint(&mut rest)?;
            let tail = rest.get(..length)?;
            rest = rest.get(length..)?;
            previous.truncate(previous.len().min(shared));
            if previous.len() != shared {
                return None;
            }
            previous.extend_from_slice(tail);
            seen.insert(String::from_utf8(previous.clone()).ok()?);
        }
        Some(Self { seen })
    }
}

fn put_varint(out: &mut Vec<u8>, mut value: usize) {
    while value >= 0x80 {
        out.push((value & 0x7f) as u8 | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

fn take_varint(input: &mut &[u8]) -> Option<usize> {
    let mut value = 0usize;
    for shift in (0..usize::BITS).step_by(7) {
        let (byte, rest) = input.split_first()?;
        *input = rest;
        value |= usize::from(byte & 0x7f).checked_shl(shift)?;
        if byte & 0x80 == 0 {
            return Some(value);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cursor<'a>(ids: impl IntoIterator<Item = &'a str>) -> Cursor {
        Cursor {
            seen: ids.into_iter().map(str::to_owned).collect(),
        }
    }

    /// A small deterministic generator, so the tests need no random crate
    /// and fail the same way every time.
    struct Lcg(u64);

    impl Lcg {
        fn next(&mut self) -> u64 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            self.0 >> 11
        }
    }

    #[test]
    fn a_cursor_round_trips() {
        for ids in [
            vec![],
            vec!["a"],
            vec!["0000000169185d3a", "0000000269185d3a", "0000000a69185d3a"],
            vec!["abc", "abcdef", "abd", "b", "zzzzzzzz"],
            vec!["odd id with spaces", "naïve-ünïcödé", "%;:,\"'\\"],
        ] {
            let cursor = cursor(ids);
            let encoded = cursor.encode();
            assert!(encoded.0.starts_with("1;"), "{}", encoded.0);
            assert_eq!(Cursor::decode(&encoded), Some(cursor));
        }
    }

    #[test]
    fn a_long_id_round_trips() {
        // Longer than a one-byte length can say.
        let long = "x".repeat(300);
        let longer = format!("{long}y");
        let cursor = cursor([long.as_str(), longer.as_str()]);
        assert_eq!(Cursor::decode(&cursor.encode()), Some(cursor));
    }

    #[test]
    fn the_same_ids_always_give_the_same_cursor() {
        let forwards = cursor(["b", "a", "c"]);
        let backwards = cursor(["c", "b", "a"]);
        assert_eq!(forwards.encode(), backwards.encode());
        assert_eq!(forwards.encode(), forwards.encode());
    }

    #[test]
    fn a_cursor_from_elsewhere_is_ignored_not_trusted() {
        let good = cursor(["0000000169185d3a", "0000000269185d3a"]).encode().0;
        let truncated = &good[..good.len() - 3];
        let wrong_version = good.replacen("1;", "2;", 1);
        let no_separator = good.replacen("1;", "1", 1);
        for other in [
            "",
            "1",
            "fixture-v1",
            // An IMAP cursor.
            "1;7;1:3,9",
            "1;not base64!",
            // Base64 of bytes that are not DEFLATE.
            "1;/////w",
            truncated,
            wrong_version.as_str(),
            no_separator.as_str(),
        ] {
            assert_eq!(
                Cursor::decode(&SyncCursor(other.to_owned())),
                None,
                "{other}"
            );
        }
        // An empty list is a cursor: a mailbox that was empty.
        assert_eq!(Cursor::decode(&cursor([]).encode()), Some(cursor([])));
    }

    #[test]
    fn a_list_that_contradicts_itself_is_refused() {
        let pack = |list: &[u8]| {
            let mut encoder = DeflateEncoder::new(Vec::new(), Compression::best());
            encoder.write_all(list).unwrap();
            SyncCursor(format!(
                "1;{}",
                STANDARD_NO_PAD.encode(encoder.finish().unwrap())
            ))
        };
        // Shares three bytes with an id that has only one.
        assert_eq!(Cursor::decode(&pack(&[0, 1, b'a', 3, 1, b'b'])), None);
        // Promises more bytes than there are.
        assert_eq!(Cursor::decode(&pack(&[0, 9, b'a'])), None);
        // A count with no end.
        assert_eq!(Cursor::decode(&pack(&[0x80; 12])), None);
        // Not text.
        assert_eq!(Cursor::decode(&pack(&[0, 2, 0xff, 0xfe])), None);
        // And one that is sound.
        assert_eq!(
            Cursor::decode(&pack(&[0, 1, b'a', 1, 1, b'b'])),
            Some(cursor(["a", "ab"]))
        );
    }

    #[test]
    fn fifty_thousand_sequential_ids_stay_small() {
        // What Dovecot hands out: the UID and the UIDVALIDITY in hex.
        let ids: Vec<String> = (1..=50_000u32)
            .map(|uid| format!("{uid:08x}69185d3a"))
            .collect();
        let cursor = cursor(ids.iter().map(String::as_str));
        let encoded = cursor.encode();
        let plain: usize = ids.iter().map(|id| id.len() + 1).sum();
        eprintln!(
            "50,000 sequential ids: {} bytes of cursor for {plain} bytes of ids",
            encoded.0.len()
        );
        assert!(encoded.0.len() < 10_000, "{}", encoded.0.len());
        assert_eq!(Cursor::decode(&encoded), Some(cursor));
    }

    #[test]
    fn fifty_thousand_ids_with_gaps_stay_small() {
        // A lived-in mailbox: most of what ever arrived has been deleted.
        let mut random = Lcg(7);
        let mut uid = 0u64;
        let ids: Vec<String> = (0..50_000)
            .map(|_| {
                uid += 1 + random.next() % 6;
                format!("{uid:08x}69185d3a")
            })
            .collect();
        let cursor = cursor(ids.iter().map(String::as_str));
        let encoded = cursor.encode();
        eprintln!("50,000 ids with gaps: {} bytes of cursor", encoded.0.len());
        assert!(encoded.0.len() < 100_000, "{}", encoded.0.len());
        assert_eq!(Cursor::decode(&encoded), Some(cursor));
    }

    #[test]
    fn fifty_thousand_random_ids_cost_no_more_than_the_ids_themselves() {
        // The worst case: ids with nothing in common, such as an MD5 of
        // each message in hex. Nothing can be squeezed out but the hex.
        let mut random = Lcg(11);
        let ids: Vec<String> = (0..50_000)
            .map(|_| format!("{:016x}{:016x}", random.next() << 11, random.next() << 11))
            .collect();
        let cursor = cursor(ids.iter().map(String::as_str));
        let encoded = cursor.encode();
        let plain: usize = ids.iter().map(|id| id.len() + 1).sum();
        eprintln!(
            "50,000 random ids: {} bytes of cursor for {plain} bytes of ids",
            encoded.0.len()
        );
        assert!(encoded.0.len() < plain, "{} >= {plain}", encoded.0.len());
        assert_eq!(Cursor::decode(&encoded), Some(cursor));
    }
}
