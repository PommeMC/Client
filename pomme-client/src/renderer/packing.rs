//! Shelf packing for texture atlases.
//!
//! A fixed atlas size silently truncates the moment someone adds a sprite, and
//! a dropped sprite renders as nothing at all, so callers size the atlas from
//! its contents: [`fit_atlas_size`] seeds from the padded area and doubles
//! until everything fits, the way vanilla's `Stitcher.expand` does.
//
// TODO: `chunk::atlas::pack` and the font atlas in `pipelines::menu_overlay`
// are two more hand-rolled shelf packers that predate this module; the font one
// still drops glyphs on overflow. Both should move over here.

use std::cmp::Reverse;

/// Per-rect pixel origin inside the atlas, or `None` if it did not fit.
pub(crate) type Placements = Vec<Option<(u32, u32)>>;

/// Shelf packing strands the unused height at the tail of every shelf, so seed
/// the atlas from ~1.4x the raw area. Same factor as `chunk::atlas`.
const SHELF_SLACK: f64 = 1.4;

/// A rect's footprint once its gutter is counted. Vanilla bakes the pad into
/// the cell the same way and on all four sides (`Stitcher.registerSprite`), so
/// even rects against the atlas boundary keep their moat.
fn padded(size: u32, pad: u32) -> u32 {
    size.saturating_add(2 * pad)
}

/// Smallest power-of-two square atlas that holds every rect with a `pad` texel
/// gutter around each, plus where each one landed and whether they all fit.
///
/// Placements come back in the order `sizes` was given, whatever order the
/// packer chose internally. Pure and total: the seed is clamped to `max_size`
/// up front, so the loop runs at most `log2(max_size)` times. The caller
/// decides how loudly to complain about a rect that did not fit, which is what
/// lets that path be tested without tripping the caller's `debug_assert!`.
pub(crate) fn fit_atlas_size(
    sizes: &[(u32, u32)],
    pad: u32,
    max_size: u32,
) -> (u32, Placements, bool) {
    // Shelf packing gives every rect in a shelf the height of the tallest one
    // in it, so a short rect landing after a tall one strands the difference.
    // Height then width descending is vanilla's order
    // (`Stitcher.HOLDER_COMPARATOR`); its third key, the sprite name, buys a
    // reproducibility that sorting a fixed input order stably already gives.
    let mut order: Vec<usize> = (0..sizes.len()).collect();
    order.sort_by_key(|&i| (Reverse(sizes[i].1), Reverse(sizes[i].0)));
    let tallest_first: Vec<(u32, u32)> = order.iter().map(|&i| sizes[i]).collect();

    let total_area: u64 = sizes
        .iter()
        .map(|&(w, h)| u64::from(padded(w, pad)) * u64::from(padded(h, pad)))
        .sum();

    let seed = ((total_area as f64) * SHELF_SLACK).sqrt().ceil();
    // Clamp before `next_power_of_two`, which panics on overflow in debug.
    let mut atlas_size = if seed >= f64::from(max_size) {
        max_size
    } else {
        (seed as u32).max(1).next_power_of_two().min(max_size)
    };

    loop {
        let (packed, all_fit) = shelf_pack(&tallest_first, atlas_size, pad);
        if all_fit || atlas_size >= max_size {
            let mut placements = vec![None; sizes.len()];
            for (&i, placement) in order.iter().zip(packed) {
                placements[i] = placement;
            }
            return (atlas_size, placements, all_fit);
        }
        atlas_size *= 2;
    }
}

/// Shelf-packs `sizes` into a square `atlas_size` atlas in the order given,
/// returning each origin already offset by `pad` so the caller blits straight
/// to it, and whether every rect was placed.
fn shelf_pack(sizes: &[(u32, u32)], atlas_size: u32, pad: u32) -> (Placements, bool) {
    let mut placements = Vec::with_capacity(sizes.len());
    let mut all_fit = true;
    let mut shelf_x = 0u32;
    let mut shelf_y = 0u32;
    let mut shelf_h = 0u32;

    for &(w, h) in sizes {
        let (cell_w, cell_h) = (padded(w, pad), padded(h, pad));

        // Bigger than the whole atlas in some axis, so no amount of shelf
        // juggling places it. Leave the cursor alone rather than wrapping, or
        // it would strand the shelf its neighbours are still filling.
        if cell_w > atlas_size || cell_h > atlas_size {
            all_fit = false;
            placements.push(None);
            continue;
        }
        if shelf_x + cell_w > atlas_size {
            shelf_y += shelf_h;
            shelf_x = 0;
            shelf_h = 0;
        }
        if shelf_y + cell_h > atlas_size {
            all_fit = false;
            placements.push(None);
            continue;
        }

        placements.push(Some((shelf_x + pad, shelf_y + pad)));
        shelf_x += cell_w;
        shelf_h = shelf_h.max(cell_h);
    }

    (placements, all_fit)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The GUI sprite atlas's settings, used unless a test exercises the knobs.
    const PAD: u32 = 1;
    const MAX: u32 = 4096;

    /// Every caller's invariant: padded cells stay inside the atlas and never
    /// overlap. `blit_image` silently drops out-of-bounds writes, so breaking
    /// this shows up as a mysteriously blank sprite rather than a panic.
    fn assert_sound(sizes: &[(u32, u32)], placements: &[Option<(u32, u32)>], size: u32, pad: u32) {
        let cells: Vec<(u32, u32, u32, u32)> = sizes
            .iter()
            .zip(placements)
            .filter_map(|(&(w, h), placement)| {
                let (x, y) = (*placement)?;
                assert!(x >= pad && y >= pad, "rect at ({x}, {y}) lost its gutter");
                Some((x - pad, y - pad, padded(w, pad), padded(h, pad)))
            })
            .collect();

        for &(x, y, w, h) in &cells {
            assert!(
                x + w <= size && y + h <= size,
                "cell ({x}, {y}, {w}, {h}) escapes the {size}px atlas"
            );
        }
        for (i, &a) in cells.iter().enumerate() {
            for &b in &cells[i + 1..] {
                let overlaps =
                    a.0 < b.0 + b.2 && b.0 < a.0 + a.2 && a.1 < b.1 + b.3 && b.1 < a.1 + a.3;
                assert!(!overlaps, "padded cells {a:?} and {b:?} overlap");
            }
        }
    }

    /// The shapes a fixed 1024px atlas dropped: the three oversized container
    /// backgrounds and all 28 creative tabs.
    #[test]
    fn dropped_sprite_shapes_all_get_placed() {
        let mut sizes = vec![(176, 167), (176, 166), (176, 166)];
        sizes.extend(std::iter::repeat_n((26, 32), 28));

        let (size, placements, all_fit) = fit_atlas_size(&sizes, PAD, MAX);

        assert!(all_fit);
        assert_eq!(placements.len(), sizes.len());
        assert_eq!(size, 512, "area seed should land on 512 for this set");
        assert_sound(&sizes, &placements, size, PAD);
    }

    /// The regression: a set too big for the seeded size grows the atlas rather
    /// than losing rects off the end.
    #[test]
    fn overflowing_set_grows_instead_of_dropping() {
        let sizes = vec![(176, 166); 40];

        // Proof the fixture genuinely overflows the size that used to be
        // hardcoded, so it cannot quietly stop testing anything.
        let (fixed, fixed_fit) = shelf_pack(&sizes, 1024, PAD);
        assert!(!fixed_fit);
        assert_eq!(fixed.iter().filter(|p| p.is_none()).count(), 10);

        let (size, placements, all_fit) = fit_atlas_size(&sizes, PAD, MAX);

        assert!(all_fit);
        assert!(placements.iter().all(Option::is_some));
        assert_eq!(size, 2048);
        assert_sound(&sizes, &placements, size, PAD);
    }

    /// Tall thin rects defeat the area estimate, so the driver has to actually
    /// double rather than trust its seed.
    #[test]
    fn grow_to_fit_doubles_past_a_bad_seed() {
        let sizes = vec![(1, 500); 12];

        let (size, placements, all_fit) = fit_atlas_size(&sizes, PAD, MAX);

        assert!(all_fit);
        assert_eq!(size, 512, "seed of 256 cannot clear a 500px rect");
        assert_sound(&sizes, &placements, size, PAD);
    }

    #[test]
    fn oversized_rect_is_reported_not_fitting() {
        let (size, placements, all_fit) = fit_atlas_size(&[(100, 100)], PAD, 64);

        assert_eq!(size, 64);
        assert!(!all_fit);
        assert_eq!(placements, vec![None]);
    }

    /// An unplaceable rect must not strand the shelf its neighbours are on.
    #[test]
    fn oversized_rect_does_not_evict_its_neighbours() {
        let (placements, all_fit) = shelf_pack(&[(16, 16), (999, 16), (16, 16)], 64, PAD);

        assert!(!all_fit);
        assert_eq!(placements[0], Some((1, 1)));
        assert_eq!(placements[1], None);
        assert_eq!(placements[2], Some((19, 1)));
    }

    /// Neighbours sit `2 * pad` apart and neither touches the atlas edge.
    #[test]
    fn adjacent_rects_keep_a_gutter() {
        let (placements, all_fit) = shelf_pack(&[(16, 16), (16, 16)], 64, PAD);

        assert!(all_fit);
        assert_eq!(placements[0], Some((1, 1)));
        assert_eq!(placements[1], Some((19, 1)));
        let gap = placements[1].unwrap().0 - (placements[0].unwrap().0 + 16);
        assert_eq!(gap, 2 * PAD);
    }

    /// Placements track the caller's order, not the tallest-first order the
    /// packer works in.
    #[test]
    fn placements_come_back_in_input_order() {
        let sizes = [(64, 8), (16, 64), (32, 32)];

        let (size, placements, all_fit) = fit_atlas_size(&sizes, PAD, MAX);

        assert!(all_fit);
        // Tallest first, so the 64px-tall rect owns the top-left cell.
        assert_eq!(placements[1], Some((1, 1)));
        assert_sound(&sizes, &placements, size, PAD);
    }

    /// The gutter is a policy knob, not a correctness crutch.
    #[test]
    fn packs_soundly_without_padding() {
        let sizes: Vec<(u32, u32)> = (1..=40).map(|i| (i * 3, i * 2)).collect();

        let (size, placements, all_fit) = fit_atlas_size(&sizes, 0, MAX);

        assert!(all_fit);
        assert_sound(&sizes, &placements, size, 0);
    }

    /// Sound across a spread of shapes no hand-written fixture would cover.
    #[test]
    fn random_rect_sets_pack_soundly() {
        let mut rng = fastrand::Rng::with_seed(0x5EED);
        for _ in 0..64 {
            let sizes: Vec<(u32, u32)> = (0..rng.u32(1..=200))
                .map(|_| (rng.u32(1..=128), rng.u32(1..=128)))
                .collect();

            let (size, placements, all_fit) = fit_atlas_size(&sizes, PAD, MAX);

            assert!(all_fit, "{} rects should fit under {MAX}", sizes.len());
            assert_eq!(placements.len(), sizes.len());
            assert!(size.is_power_of_two());
            assert_sound(&sizes, &placements, size, PAD);
        }
    }

    #[test]
    fn empty_input_yields_a_valid_atlas() {
        let (size, placements, all_fit) = fit_atlas_size(&[], PAD, MAX);

        assert!(all_fit);
        assert!(placements.is_empty());
        assert!(size >= 1 && size.is_power_of_two());
    }
}
