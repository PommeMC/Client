use super::*;
/// Each editable identity changes only its corresponding persisted leaf.
#[test]
fn option_ids_change_one_field() {
    /// Counts changed persisted leaves, including nested chat settings.
    fn changed(a: &serde_json::Value, b: &serde_json::Value) -> usize {
        match (a.as_object(), b.as_object()) {
            (Some(a), Some(b)) => a.iter().map(|(k, v)| changed(v, &b[k])).sum(),
            _ => usize::from(a != b),
        }
    }
    for &id in OptionId::ALL {
        let mut options = GameOptions::default();
        let before = serde_json::to_value(&options).unwrap();
        if let Some(value) = id.get(&options) {
            let next = match id.kind() {
                OptionKind::Toggle => 1.0 - value,
                OptionKind::Cycle { .. } => (value + 1.0) % 3.0,
                OptionKind::Slider { min, max, .. } => {
                    if value == min {
                        max
                    } else {
                        min
                    }
                }
                OptionKind::Action => unreachable!(),
            };
            assert!(id.set(&mut options, next), "{id:?}");
            assert_eq!(
                changed(&before, &serde_json::to_value(&options).unwrap()),
                1,
                "{id:?}"
            );
        }
        assert!(!id.label_key().is_empty());
    }
}
