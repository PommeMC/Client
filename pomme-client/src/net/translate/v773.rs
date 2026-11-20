use pomme_protocol::codec::Reader;

use super::*;

/// How long a game tick is, in the milliseconds old borders count in.
const MILLIS_PER_TICK: u64 = 50;
const TICKS_PER_SECOND: u32 = 20;

impl Ids773 {
    pub(super) fn rewrite(&self, id: u32) -> Option<FrameRewrite> {
        Some(match id {
            i if i == self.initialize_border_id => translate_initialize_border,
            i if i == self.set_border_lerp_size_id => translate_set_border_lerp_size,
            i if i == self.set_border_warning_delay_id => translate_set_border_warning_delay,
            _ => return None,
        })
    }
}

/// Copies `doubles` doubles and rewrites the var-long after them from
/// milliseconds to game ticks, leaving the reader past it.
fn copy_lerp(out: &mut Vec<u8>, r: &mut Reader, doubles: usize) -> Option<()> {
    for _ in 0..doubles {
        out.extend_from_slice(&r.f64().ok()?.to_be_bytes());
    }
    wire::write_varlong(out, r.varlong().ok()? / MILLIS_PER_TICK);
    Some(())
}

/// Copies a var-int warning time, rewritten from seconds to game ticks.
fn copy_warning_time(out: &mut Vec<u8>, r: &mut Reader) -> Option<()> {
    wire::write_varint(out, r.varint().ok()?.saturating_mul(TICKS_PER_SECOND));
    Some(())
}

/// The frame for `id` whose payload `write` builds from the old one.
fn rewrite(
    id: u32,
    payload: &[u8],
    write: impl FnOnce(&mut Vec<u8>, &mut Reader) -> Option<()>,
) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(payload.len() + 4);
    wire::write_varint(&mut out, id);
    write(&mut out, &mut Reader::new(payload))?;
    Some(out)
}

/// Rewrites `initialize_border` (`centerX, centerZ, oldSize, newSize,
/// lerpTime, absoluteMaxSize, warningBlocks, warningTime`): its lerp time
/// from milliseconds and its warning time from seconds, both to the game
/// ticks 1.21.11 made them.
fn translate_initialize_border(id: u32, payload: &[u8]) -> Option<Vec<u8>> {
    rewrite(id, payload, |out, r| {
        copy_lerp(out, r, 4)?;
        for _ in 0..2 {
            wire::write_varint(out, r.varint().ok()?);
        }
        copy_warning_time(out, r)
    })
}

/// Rewrites `set_border_lerp_size` (`oldSize, newSize, lerpTime`).
fn translate_set_border_lerp_size(id: u32, payload: &[u8]) -> Option<Vec<u8>> {
    rewrite(id, payload, |out, r| copy_lerp(out, r, 2))
}

/// Rewrites `set_border_warning_delay` (`warningDelay`).
fn translate_set_border_warning_delay(id: u32, payload: &[u8]) -> Option<Vec<u8>> {
    rewrite(id, payload, copy_warning_time)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doubles(values: &[f64]) -> Vec<u8> {
        values.iter().flat_map(|v| v.to_be_bytes()).collect()
    }

    #[test]
    fn a_lerp_in_milliseconds_becomes_ticks() {
        // 10 000 ms is the var-long 90 4E; 200 ticks is C8 01.
        let mut payload = doubles(&[45.0, 25.0]);
        payload.extend([0x90, 0x4E]);
        let mut expected = vec![89];
        expected.extend(doubles(&[45.0, 25.0]));
        expected.extend([0xC8, 0x01]);
        assert_eq!(translate_set_border_lerp_size(89, &payload), Some(expected));
    }

    #[test]
    fn initialize_converts_its_lerp_and_warning_times_only() {
        let mut payload = doubles(&[1.0, 2.0, 60.0, 20.0]);
        // 10 000 ms, a maximum of 100, 5 warning blocks, 15 warning seconds.
        payload.extend([0x90, 0x4E, 100, 5, 15]);
        let mut expected = vec![43];
        expected.extend(doubles(&[1.0, 2.0, 60.0, 20.0]));
        // 200 ticks, then 300 ticks of warning as AC 02.
        expected.extend([0xC8, 0x01, 100, 5, 0xAC, 0x02]);
        assert_eq!(translate_initialize_border(43, &payload), Some(expected));
    }

    #[test]
    fn a_warning_delay_in_seconds_becomes_ticks() {
        assert_eq!(
            translate_set_border_warning_delay(91, &[15]),
            Some(vec![91, 0xAC, 0x02])
        );
        assert_eq!(translate_set_border_warning_delay(91, &[]), None);
    }
}
