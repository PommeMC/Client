//! Component values pomme reads itself, each ported from its vanilla
//! `STREAM_CODEC`.

use pomme_protocol::codec::{DecodeResult, Reader};
use pomme_protocol::item::{self as wire, ItemCodec};
use pomme_protocol::wire::write_varint;
use pomme_registry::builtin::DataComponentKind;

use crate::ItemStack;

/// A data component's value, read from and written to its wire bytes.
pub trait Component: Sized {
    const KIND: DataComponentKind;

    fn read(r: &mut Reader) -> DecodeResult<Self>;
    fn write(&self, buf: &mut Vec<u8>);

    /// The value as a patch holds it.
    fn to_bytes(&self) -> Box<[u8]> {
        let mut buf = Vec::new();
        self.write(&mut buf);
        buf.into()
    }
}

/// `ItemStackTemplate.STREAM_CODEC`: a stack that is never empty.
fn read_template(r: &mut Reader) -> DecodeResult<ItemStack> {
    let template = ItemCodec::native().read_template(r)?;
    Ok(ItemStack::from_wire(&template))
}

/// Writes each stack as a template after their count; an empty stack has no
/// template and is left out.
fn write_templates(buf: &mut Vec<u8>, items: &[ItemStack]) {
    let templates: Vec<_> = items.iter().filter_map(ItemStack::to_wire).collect();
    write_varint(buf, templates.len() as u32);
    for template in &templates {
        wire::write_template(buf, template);
    }
}

/// Vanilla `BundleContents`: the stacks in a bundle, newest first.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BundleContents {
    pub items: Vec<ItemStack>,
}

impl Component for BundleContents {
    const KIND: DataComponentKind = DataComponentKind::BundleContents;

    fn read(r: &mut Reader) -> DecodeResult<Self> {
        let items = r.list(usize::MAX, read_template)?;
        Ok(Self { items })
    }

    fn write(&self, buf: &mut Vec<u8>) {
        write_templates(buf, &self.items);
    }
}

/// Vanilla `ChargedProjectiles`: what a crossbow is loaded with.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChargedProjectiles {
    pub items: Vec<ItemStack>,
}

impl ChargedProjectiles {
    /// `ByteBufCodecs.list(1024)`.
    const MAX_ITEMS: usize = 1024;
}

impl Component for ChargedProjectiles {
    const KIND: DataComponentKind = DataComponentKind::ChargedProjectiles;

    fn read(r: &mut Reader) -> DecodeResult<Self> {
        let items = r.list(Self::MAX_ITEMS, read_template)?;
        Ok(Self { items })
    }

    fn write(&self, buf: &mut Vec<u8>) {
        write_templates(buf, &self.items);
    }
}

#[cfg(test)]
mod tests {
    use pomme_registry::Registry;
    use pomme_registry::builtin::ItemKind;

    use super::*;

    fn read<T: Component>(bytes: &[u8]) -> DecodeResult<T> {
        let mut r = Reader::new(bytes);
        let value = T::read(&mut r)?;
        r.ensure_eof()?;
        Ok(value)
    }

    /// Two templates: item, count, then an empty patch's two zero counts.
    fn two_stacks() -> Vec<u8> {
        let (stone, arrow) = (ItemKind::Stone.to_u32(), ItemKind::Arrow.to_u32());
        let mut bytes = vec![2];
        for (item, count) in [(stone, 3), (arrow, 1)] {
            write_varint(&mut bytes, item);
            bytes.extend([count, 0, 0]);
        }
        bytes
    }

    #[test]
    fn bundle_contents_are_a_counted_list_of_templates() {
        let bytes = two_stacks();
        let contents: BundleContents = read(&bytes).unwrap();
        assert_eq!(
            contents.items,
            [
                ItemStack::new(ItemKind::Stone, 3),
                ItemStack::new(ItemKind::Arrow, 1)
            ]
        );
        assert_eq!(*contents.to_bytes(), bytes[..]);
        assert_eq!(
            read::<BundleContents>(&[0]).unwrap(),
            BundleContents::default()
        );
    }

    #[test]
    fn an_empty_stack_is_left_out_of_what_is_written() {
        let contents = BundleContents {
            items: vec![ItemStack::Empty, ItemStack::new(ItemKind::Stone, 1)],
        };
        let written: BundleContents = read(&contents.to_bytes()).unwrap();
        assert_eq!(written.items, [ItemStack::new(ItemKind::Stone, 1)]);
    }

    #[test]
    fn charged_projectiles_share_the_layout_and_cap_the_count() {
        let bytes = two_stacks();
        let charged: ChargedProjectiles = read(&bytes).unwrap();
        assert_eq!(charged.items.len(), 2);
        assert_eq!(*charged.to_bytes(), bytes[..]);
        // 1025 as a varint.
        assert!(read::<ChargedProjectiles>(&[0x81, 0x08]).is_err());
    }
}
