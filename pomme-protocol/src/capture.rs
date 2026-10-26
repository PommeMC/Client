//! The frame-capture format: clientbound frames as a server sent them, each
//! a phase byte, the frame's length as a big-endian u32, then the frame
//! (packet id and payload). The client's `--capture-frames` writes it and
//! `ingame-test --capture` trims it into a `wire-fixtures/` corpus.

use crate::Phase;

/// Appends one record for `frame`.
pub fn write_record(out: &mut Vec<u8>, phase: Phase, frame: &[u8]) {
    out.push(match phase {
        Phase::Login => 0,
        Phase::Configuration => 1,
        Phase::Game => 2,
        Phase::Handshake | Phase::Status => unreachable!("not captured"),
    });
    out.extend_from_slice(&(frame.len() as u32).to_be_bytes());
    out.extend_from_slice(frame);
}

/// The records in `bytes`, in order; `None` if any is cut short or has an
/// unknown phase.
pub fn records(mut bytes: &[u8]) -> Option<Vec<(Phase, &[u8])>> {
    let mut all = Vec::new();
    while let [phase, rest @ ..] = bytes {
        let phase = match phase {
            0 => Phase::Login,
            1 => Phase::Configuration,
            2 => Phase::Game,
            _ => return None,
        };
        let len = u32::from_be_bytes(rest.get(..4)?.try_into().ok()?) as usize;
        let frame = rest.get(4..4 + len)?;
        all.push((phase, frame));
        bytes = &rest[4 + len..];
    }
    Some(all)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_round_trip() {
        let mut bytes = Vec::new();
        write_record(&mut bytes, Phase::Configuration, &[3, 1]);
        write_record(&mut bytes, Phase::Game, &[]);
        assert_eq!(bytes, [1, 0, 0, 0, 2, 3, 1, 2, 0, 0, 0, 0]);
        assert_eq!(
            records(&bytes),
            Some(vec![
                (Phase::Configuration, &[3u8, 1][..]),
                (Phase::Game, &[][..])
            ])
        );
        assert_eq!(records(&bytes[..6]), None);
    }
}
