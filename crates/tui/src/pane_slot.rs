//! The pane beside the transcript, which shows Activity or the plan's
//! Timeline. Focus and full screen belong to the slot, not to a view, so
//! Ctrl+A, z and Esc mean the same in both and only one thing can be
//! focused. Each view keeps its own state, which survives a swap.

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum SlotView {
    #[default]
    Activity,
    Timeline,
}

#[derive(Debug, Default)]
pub(crate) struct PaneSlot {
    /// The view in front: the one drawn when the slot shows one view, and
    /// the one that gets keys and the focus color when it shows both.
    pub(crate) view: SlotView,
    pub(crate) focused: bool,
    /// Only while focused; unfocusing leaves full screen.
    pub(crate) fullscreen: bool,
}

impl PaneSlot {
    /// Brings the other view forward. Focus and full screen stay, so a
    /// focused user keeps browsing, now in the other view.
    pub(crate) fn swap(&mut self) {
        self.view = match self.view {
            SlotView::Activity => SlotView::Timeline,
            SlotView::Timeline => SlotView::Activity,
        };
    }

    pub(crate) fn toggle_focus(&mut self) {
        self.focused = !self.focused;
        if !self.focused {
            self.fullscreen = false;
        }
    }

    pub(crate) fn toggle_fullscreen(&mut self) {
        self.fullscreen = self.focused && !self.fullscreen;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn swapping_keeps_focus_and_unfocusing_leaves_full_screen() {
        let mut slot = PaneSlot::default();
        slot.toggle_fullscreen();
        assert!(!slot.fullscreen, "full screen needs focus");
        slot.toggle_focus();
        slot.toggle_fullscreen();
        slot.swap();
        assert_eq!(slot.view, SlotView::Timeline);
        assert!(slot.focused && slot.fullscreen);
        slot.toggle_focus();
        assert!(!slot.fullscreen);
    }
}
