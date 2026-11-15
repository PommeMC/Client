//! `update_tags` (`ClientboundUpdateTagsPacket`) as vanilla 26.2 lays it out:
//! each registry's tags and the ids of their elements.

use std::collections::HashMap;

use crate::codec::{DecodeResult, Reader};

/// Registry key to tag name to element ids. A key sent twice keeps its last
/// value, as vanilla's `readMap` does.
pub type Tags = HashMap<String, HashMap<String, Vec<i32>>>;

/// Decodes packet `name`, or `None` if it isn't `update_tags`.
pub fn read(name: &str, r: &mut Reader) -> Option<DecodeResult<Tags>> {
    (name == "update_tags").then(|| {
        // ClientboundUpdateTagsPacket.java:27, `NetworkPayload.read`
        map(r, |r| map(r, |r| r.list(usize::MAX, Reader::var_i32)))
    })
}

/// `FriendlyByteBuf.readMap` keyed by identifiers.
fn map<V>(
    r: &mut Reader,
    mut value: impl FnMut(&mut Reader) -> DecodeResult<V>,
) -> DecodeResult<HashMap<String, V>> {
    let n = r.len(usize::MAX)?;
    let mut out = HashMap::with_capacity(n);
    for _ in 0..n {
        let key = r.identifier()?.to_owned();
        out.insert(key, value(r)?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vanilla_fixtures::case;

    #[test]
    fn tags_read_as_vanilla_writes_them() {
        let bytes = case("update_tags/two_registries");
        let mut r = Reader::new(&bytes);
        let tags = read("update_tags", &mut r).unwrap().unwrap();
        r.ensure_eof().unwrap();
        assert_eq!(
            tags,
            HashMap::from([
                (
                    "minecraft:block".to_owned(),
                    HashMap::from([
                        ("minecraft:logs".to_owned(), vec![1, 300]),
                        ("minecraft:empty".to_owned(), vec![]),
                    ]),
                ),
                (
                    "minecraft:dialog".to_owned(),
                    HashMap::from([("minecraft:pause_screen_additions".to_owned(), vec![2])]),
                ),
            ])
        );
    }

    #[test]
    fn other_packets_are_not_read() {
        assert!(read("set_camera", &mut Reader::new(&[])).is_none());
    }
}
