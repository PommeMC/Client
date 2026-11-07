use super::*;

/// Every supported version resolves packet names and preserves the oldest
/// respawn rewrite.
#[test]
fn dispatch_tables_resolve_names_and_priorities() {
    let native = PacketTable::native();
    let count = (0..)
        .map_while(|id| native.name_of(Phase::Game, Direction::Clientbound, id))
        .count();
    let respawn_id = native
        .id(Phase::Game, Direction::Clientbound, "respawn")
        .unwrap();
    for &protocol in TRANSLATED {
        let t = Translation::for_protocol(protocol).unwrap();
        assert_eq!(t.rewrites.len(), count);
        assert!(t.rewrites.get(count).is_none());
        let expected = match protocol {
            763 => Some(Rewrite::SessionFrame(translate_respawn_763)),
            764..=765 => Some(Rewrite::SessionFrame(translate_respawn_765)),
            766..=767 => Some(Rewrite::Frame(translate_respawn_767)),
            777 => Some(Rewrite::Frame(translate_respawn_777)),
            _ => None,
        };
        assert!(
            match (t.rewrites[respawn_id as usize], expected) {
                (Some(actual), Some(expected)) => actual.same(expected),
                (None, None) => true,
                _ => false,
            },
            "respawn precedence for {protocol}"
        );
    }
}
