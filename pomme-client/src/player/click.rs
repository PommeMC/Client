//! A click in a container screen, as vanilla `AbstractContainerMenu.clicked`
//! takes it: a slot, a button and a `ContainerInput`.

use pomme_protocol::serverbound::ContainerInput;

/// One click, by what it does.
#[derive(Clone, Debug)]
pub enum ClickOperation {
    Pickup(PickupClick),
    QuickMove(QuickMoveClick),
    Swap(SwapClick),
    Clone(CloneClick),
    Throw(ThrowClick),
    QuickCraft(QuickCraftClick),
    PickupAll(PickupAllClick),
}

/// A left or right click; with no slot it is outside the menu and drops
/// what the cursor carries, which the packet sends as slot -999.
#[derive(Clone, Debug)]
pub enum PickupClick {
    Left { slot: Option<u16> },
    Right { slot: Option<u16> },
}

/// A shift-click, with either button.
#[derive(Clone, Debug)]
pub enum QuickMoveClick {
    Left { slot: u16 },
    Right { slot: u16 },
}

/// A number key or the swap-hands key over a slot.
#[derive(Clone, Debug)]
pub struct SwapClick {
    pub source_slot: u16,
    /// The hotbar slot, 0 to 8, or 40 for the offhand.
    pub target_slot: u8,
}

/// A middle click, which copies a stack in creative.
#[derive(Clone, Debug)]
pub struct CloneClick {
    pub slot: u16,
}

/// The drop key over a slot: one item, or the whole stack with control.
#[derive(Clone, Debug)]
pub enum ThrowClick {
    Single { slot: u16 },
    All { slot: u16 },
}

/// One step of a drag across slots.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QuickCraftClick {
    pub kind: QuickCraftKind,
    pub status: QuickCraftStatus,
}

/// The button a drag is made with, in `getQuickcraftType`'s order.
// TODO: creative's middle-button drag (type 2), which fills each slot, is
// not started anywhere yet.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QuickCraftKind {
    Left,
    Right,
}

/// Where in a drag a step is, in `getQuickcraftHeader`'s order.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QuickCraftStatus {
    Start,
    Add { slot: u16 },
    End,
}

/// A double click, which gathers what matches the carried stack.
#[derive(Clone, Debug)]
pub struct PickupAllClick {
    pub slot: u16,
}

impl ClickOperation {
    /// The slot the click acts on, if it has one.
    pub fn slot_num(&self) -> Option<u16> {
        match self {
            Self::Pickup(PickupClick::Left { slot } | PickupClick::Right { slot }) => *slot,
            Self::QuickMove(QuickMoveClick::Left { slot } | QuickMoveClick::Right { slot })
            | Self::Throw(ThrowClick::Single { slot } | ThrowClick::All { slot })
            | Self::Clone(CloneClick { slot })
            | Self::PickupAll(PickupAllClick { slot }) => Some(*slot),
            Self::Swap(swap) => Some(swap.source_slot),
            Self::QuickCraft(drag) => match drag.status {
                QuickCraftStatus::Add { slot } => Some(slot),
                QuickCraftStatus::Start | QuickCraftStatus::End => None,
            },
        }
    }

    /// The packet's button: the mouse button, a swap's target, or a drag's
    /// `getQuickcraftMask`.
    pub fn button_num(&self) -> u8 {
        match self {
            Self::Pickup(PickupClick::Left { .. })
            | Self::QuickMove(QuickMoveClick::Left { .. })
            | Self::Throw(ThrowClick::Single { .. })
            | Self::PickupAll(_) => 0,
            Self::Pickup(PickupClick::Right { .. })
            | Self::QuickMove(QuickMoveClick::Right { .. })
            | Self::Throw(ThrowClick::All { .. }) => 1,
            Self::Clone(_) => 2,
            Self::Swap(swap) => swap.target_slot,
            Self::QuickCraft(drag) => {
                let header = match drag.status {
                    QuickCraftStatus::Start => 0,
                    QuickCraftStatus::Add { .. } => 1,
                    QuickCraftStatus::End => 2,
                };
                header | (drag.kind as u8) << 2
            }
        }
    }

    pub fn input(&self) -> ContainerInput {
        match self {
            Self::Pickup(_) => ContainerInput::Pickup,
            Self::QuickMove(_) => ContainerInput::QuickMove,
            Self::Swap(_) => ContainerInput::Swap,
            Self::Clone(_) => ContainerInput::Clone,
            Self::Throw(_) => ContainerInput::Throw,
            Self::QuickCraft(_) => ContainerInput::QuickCraft,
            Self::PickupAll(_) => ContainerInput::PickupAll,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_drags_button_packs_its_step_and_its_mouse_button() {
        let button = |kind, status| {
            ClickOperation::QuickCraft(QuickCraftClick { kind, status }).button_num()
        };
        let add = QuickCraftStatus::Add { slot: 3 };
        assert_eq!(button(QuickCraftKind::Left, QuickCraftStatus::Start), 0);
        assert_eq!(button(QuickCraftKind::Left, add), 1);
        assert_eq!(button(QuickCraftKind::Left, QuickCraftStatus::End), 2);
        assert_eq!(button(QuickCraftKind::Right, QuickCraftStatus::Start), 4);
        assert_eq!(button(QuickCraftKind::Right, add), 5);
        assert_eq!(button(QuickCraftKind::Right, QuickCraftStatus::End), 6);
    }

    #[test]
    fn a_click_names_its_slot_button_and_input() {
        let parts = |op: ClickOperation| (op.slot_num(), op.button_num(), op.input());
        assert_eq!(
            parts(ClickOperation::Pickup(PickupClick::Right { slot: None })),
            (None, 1, ContainerInput::Pickup)
        );
        assert_eq!(
            parts(ClickOperation::Swap(SwapClick {
                source_slot: 12,
                target_slot: 40,
            })),
            (Some(12), 40, ContainerInput::Swap)
        );
        assert_eq!(
            parts(ClickOperation::Throw(ThrowClick::All { slot: 7 })),
            (Some(7), 1, ContainerInput::Throw)
        );
        assert_eq!(
            parts(ClickOperation::Clone(CloneClick { slot: 5 })),
            (Some(5), 2, ContainerInput::Clone)
        );
    }
}
