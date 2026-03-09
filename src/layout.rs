use std::collections::BTreeMap;

use crate::pane::{PaneId, Rect};

const DEFAULT_RATIO: u16 = 500;
const MIN_RATIO: u16 = 100;
const MAX_RATIO: u16 = 900;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SplitAxis {
    Horizontal,
    Vertical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum LayoutNode {
    Pane(PaneId),
    Split {
        axis: SplitAxis,
        ratio: u16,
        first: Box<LayoutNode>,
        second: Box<LayoutNode>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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
        self.root = self
            .root
            .clone()
            .split(active, axis, DEFAULT_RATIO, new_pane);
        self.active = new_pane;
    }

    pub fn panes(&self) -> Vec<PaneId> {
        let mut panes = Vec::new();
        self.root.collect_panes(&mut panes);
        panes
    }

    pub fn pane_rects(&self, area: Rect) -> BTreeMap<PaneId, Rect> {
        let mut rects = BTreeMap::new();
        self.root.collect_rects(area, &mut rects);
        rects
    }

    pub fn select_direction(&mut self, direction: Direction, area: Rect) -> Option<PaneId> {
        let rects = self.pane_rects(area);
        let active_rect = *rects.get(&self.active)?;
        let active_center = center(active_rect);

        let next = rects
            .into_iter()
            .filter(|(pane, _)| *pane != self.active)
            .filter_map(|(pane, rect)| {
                let candidate = center(rect);
                let score = match direction {
                    Direction::Left if rect.right() <= active_rect.x => Some((
                        active_rect.x.saturating_sub(rect.right()),
                        axis_distance(active_center.1, candidate.1),
                    )),
                    Direction::Right if rect.x >= active_rect.right() => Some((
                        rect.x.saturating_sub(active_rect.right()),
                        axis_distance(active_center.1, candidate.1),
                    )),
                    Direction::Up if rect.bottom() <= active_rect.y => Some((
                        active_rect.y.saturating_sub(rect.bottom()),
                        axis_distance(active_center.0, candidate.0),
                    )),
                    Direction::Down if rect.y >= active_rect.bottom() => Some((
                        rect.y.saturating_sub(active_rect.bottom()),
                        axis_distance(active_center.0, candidate.0),
                    )),
                    _ => None,
                }?;
                Some((score, pane))
            })
            .min_by_key(|(score, _)| *score)
            .map(|(_, pane)| pane);

        if let Some(next) = next {
            self.active = next;
        }
        next
    }

    pub fn resize_active(&mut self, direction: Direction, amount: u16) -> bool {
        self.root.resize(self.active, direction, amount)
    }

    pub fn remove_active(&mut self) -> Option<PaneId> {
        let active = self.active;
        let mut fallback = None;
        self.root = self.root.clone().remove(active, &mut fallback)?;
        self.active = fallback?;
        Some(self.active)
    }

    pub fn remove_pane(&mut self, pane: PaneId) -> Option<PaneId> {
        let mut fallback = None;
        self.root = self.root.clone().remove(pane, &mut fallback)?;
        if self.active == pane {
            self.active = fallback?;
        }
        Some(self.active)
    }
}

impl LayoutNode {
    fn split(self, target: PaneId, axis: SplitAxis, ratio: u16, new_pane: PaneId) -> Self {
        match self {
            LayoutNode::Pane(pane) if pane == target => LayoutNode::Split {
                axis,
                ratio,
                first: Box::new(LayoutNode::Pane(pane)),
                second: Box::new(LayoutNode::Pane(new_pane)),
            },
            LayoutNode::Pane(pane) => LayoutNode::Pane(pane),
            LayoutNode::Split {
                axis: current_axis,
                ratio: current_ratio,
                first,
                second,
            } => {
                if first.contains(target) {
                    LayoutNode::Split {
                        axis: current_axis,
                        ratio: current_ratio,
                        first: Box::new(first.split(target, axis, ratio, new_pane)),
                        second,
                    }
                } else if second.contains(target) {
                    LayoutNode::Split {
                        axis: current_axis,
                        ratio: current_ratio,
                        first,
                        second: Box::new(second.split(target, axis, ratio, new_pane)),
                    }
                } else {
                    LayoutNode::Split {
                        axis: current_axis,
                        ratio: current_ratio,
                        first,
                        second,
                    }
                }
            }
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

    fn collect_rects(&self, area: Rect, rects: &mut BTreeMap<PaneId, Rect>) {
        match self {
            LayoutNode::Pane(pane) => {
                rects.insert(*pane, area);
            }
            LayoutNode::Split {
                axis,
                ratio,
                first,
                second,
            } => {
                let (first_rect, second_rect) = split_rect(area, *axis, *ratio);
                first.collect_rects(first_rect, rects);
                second.collect_rects(second_rect, rects);
            }
        }
    }

    fn contains(&self, target: PaneId) -> bool {
        match self {
            LayoutNode::Pane(pane) => *pane == target,
            LayoutNode::Split { first, second, .. } => {
                first.contains(target) || second.contains(target)
            }
        }
    }

    fn resize(&mut self, target: PaneId, direction: Direction, amount: u16) -> bool {
        match self {
            LayoutNode::Pane(_) => false,
            LayoutNode::Split {
                axis,
                ratio,
                first,
                second,
            } => {
                let first_contains = first.contains(target);
                let second_contains = second.contains(target);
                if !first_contains && !second_contains {
                    return false;
                }

                let desired_axis = match direction {
                    Direction::Left | Direction::Right => SplitAxis::Vertical,
                    Direction::Up | Direction::Down => SplitAxis::Horizontal,
                };

                if *axis == desired_axis {
                    let delta = amount.min(100);
                    let grow_first = matches!(
                        (direction, first_contains, second_contains),
                        (Direction::Left, true, false)
                            | (Direction::Up, true, false)
                            | (Direction::Right, false, true)
                            | (Direction::Down, false, true)
                    );
                    if grow_first {
                        *ratio = (*ratio + delta).clamp(MIN_RATIO, MAX_RATIO);
                    } else {
                        *ratio = ratio.saturating_sub(delta).clamp(MIN_RATIO, MAX_RATIO);
                    }
                    true
                } else if first_contains {
                    first.resize(target, direction, amount)
                } else {
                    second.resize(target, direction, amount)
                }
            }
        }
    }

    fn remove(self, target: PaneId, fallback: &mut Option<PaneId>) -> Option<Self> {
        match self {
            LayoutNode::Pane(pane) => {
                if pane == target {
                    None
                } else {
                    *fallback = Some(pane);
                    Some(LayoutNode::Pane(pane))
                }
            }
            LayoutNode::Split {
                axis,
                ratio,
                first,
                second,
            } => {
                if first.contains(target) {
                    match first.remove(target, fallback) {
                        Some(first) => Some(LayoutNode::Split {
                            axis,
                            ratio,
                            first: Box::new(first),
                            second,
                        }),
                        None => {
                            let replacement = *second;
                            if let Some(pane) = replacement.first_pane() {
                                *fallback = Some(pane);
                            }
                            Some(replacement)
                        }
                    }
                } else if second.contains(target) {
                    match second.remove(target, fallback) {
                        Some(second) => Some(LayoutNode::Split {
                            axis,
                            ratio,
                            first,
                            second: Box::new(second),
                        }),
                        None => {
                            let replacement = *first;
                            if let Some(pane) = replacement.first_pane() {
                                *fallback = Some(pane);
                            }
                            Some(replacement)
                        }
                    }
                } else {
                    Some(LayoutNode::Split {
                        axis,
                        ratio,
                        first,
                        second,
                    })
                }
            }
        }
    }

    fn first_pane(&self) -> Option<PaneId> {
        match self {
            LayoutNode::Pane(pane) => Some(*pane),
            LayoutNode::Split { first, .. } => first.first_pane(),
        }
    }
}

fn split_rect(area: Rect, axis: SplitAxis, ratio: u16) -> (Rect, Rect) {
    match axis {
        SplitAxis::Vertical => {
            let usable = area.width.saturating_sub(1);
            let first_width = ((u32::from(usable) * u32::from(ratio)) / 1000).max(1) as u16;
            let second_width = usable.saturating_sub(first_width);
            (
                Rect {
                    x: area.x,
                    y: area.y,
                    width: first_width,
                    height: area.height,
                },
                Rect {
                    x: area.x.saturating_add(first_width).saturating_add(1),
                    y: area.y,
                    width: second_width,
                    height: area.height,
                },
            )
        }
        SplitAxis::Horizontal => {
            let usable = area.height.saturating_sub(1);
            let first_height = ((u32::from(usable) * u32::from(ratio)) / 1000).max(1) as u16;
            let second_height = usable.saturating_sub(first_height);
            (
                Rect {
                    x: area.x,
                    y: area.y,
                    width: area.width,
                    height: first_height,
                },
                Rect {
                    x: area.x,
                    y: area.y.saturating_add(first_height).saturating_add(1),
                    width: area.width,
                    height: second_height,
                },
            )
        }
    }
}

fn center(rect: Rect) -> (u16, u16) {
    (
        rect.y.saturating_add(rect.height / 2),
        rect.x.saturating_add(rect.width / 2),
    )
}

fn axis_distance(a: u16, b: u16) -> u16 {
    a.abs_diff(b)
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

    #[test]
    fn pane_rects_account_for_split_gap() {
        let mut tree = LayoutTree::new(PaneId(1));
        tree.split_active(SplitAxis::Vertical, PaneId(2));

        let rects = tree.pane_rects(Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 20,
        });

        assert_eq!(rects[&PaneId(1)].width + rects[&PaneId(2)].width, 79);
        assert_eq!(rects[&PaneId(2)].x, rects[&PaneId(1)].width + 1);
    }

    #[test]
    fn directional_selection_prefers_adjacent_pane() {
        let mut tree = LayoutTree::new(PaneId(1));
        tree.split_active(SplitAxis::Vertical, PaneId(2));
        tree.active = PaneId(1);

        let selected = tree.select_direction(
            Direction::Right,
            Rect {
                x: 0,
                y: 0,
                width: 80,
                height: 20,
            },
        );

        assert_eq!(selected, Some(PaneId(2)));
        assert_eq!(tree.active, PaneId(2));
    }

    #[test]
    fn removing_active_collapses_split() {
        let mut tree = LayoutTree::new(PaneId(1));
        tree.split_active(SplitAxis::Vertical, PaneId(2));

        let active = tree.remove_active();

        assert_eq!(active, Some(PaneId(1)));
        assert_eq!(tree.panes(), vec![PaneId(1)]);
    }

    #[test]
    fn splitting_nested_second_branch_preserves_mixed_axis_layout() {
        let mut tree = LayoutTree::new(PaneId(1));
        tree.split_active(SplitAxis::Vertical, PaneId(2));
        tree.active = PaneId(2);

        tree.split_active(SplitAxis::Horizontal, PaneId(3));

        let rects = tree.pane_rects(Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 20,
        });

        assert_eq!(tree.panes(), vec![PaneId(1), PaneId(2), PaneId(3)]);
        assert_eq!(rects[&PaneId(1)].x, 0);
        assert_eq!(rects[&PaneId(2)].x, rects[&PaneId(3)].x);
        assert!(rects[&PaneId(2)].y < rects[&PaneId(3)].y);
    }
}
