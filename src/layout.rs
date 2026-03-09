use crate::pane::PaneId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitAxis {
    Horizontal,
    Vertical,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LayoutNode {
    Pane(PaneId),
    Split {
        axis: SplitAxis,
        first: Box<LayoutNode>,
        second: Box<LayoutNode>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayoutTree {
    pub root: LayoutNode,
    pub active: PaneId,
}

impl LayoutTree {
    pub fn new(root: PaneId) -> Self {
        Self {
            root: LayoutNode::Pane(root),
            active: root,
        }
    }

    pub fn split_active(&mut self, axis: SplitAxis, new_pane: PaneId) {
        let active = self.active;
        self.root = self.root.clone().split(active, axis, new_pane);
        self.active = new_pane;
    }

    pub fn panes(&self) -> Vec<PaneId> {
        let mut panes = Vec::new();
        self.root.collect_panes(&mut panes);
        panes
    }
}

impl LayoutNode {
    fn split(self, target: PaneId, axis: SplitAxis, new_pane: PaneId) -> Self {
        match self {
            LayoutNode::Pane(pane) if pane == target => LayoutNode::Split {
                axis,
                first: Box::new(LayoutNode::Pane(pane)),
                second: Box::new(LayoutNode::Pane(new_pane)),
            },
            LayoutNode::Pane(pane) => LayoutNode::Pane(pane),
            LayoutNode::Split {
                axis: current_axis,
                first,
                second,
            } => LayoutNode::Split {
                axis: current_axis,
                first: Box::new(first.split(target, axis, new_pane)),
                second,
            },
        }
    }

    fn collect_panes(&self, panes: &mut Vec<PaneId>) {
        match self {
            LayoutNode::Pane(pane) => panes.push(*pane),
            LayoutNode::Split { first, second, .. } => {
                first.collect_panes(panes);
                second.collect_panes(panes);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splitting_active_pane_adds_new_leaf() {
        let mut tree = LayoutTree::new(PaneId(1));
        tree.split_active(SplitAxis::Horizontal, PaneId(2));

        assert_eq!(tree.active, PaneId(2));
        assert_eq!(tree.panes(), vec![PaneId(1), PaneId(2)]);
    }
}
