//! Play order of a presentation's slides: natural group order, or an arrangement's
//! group sequence (groups may repeat).

use doc::Document;

/// One position in the play order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SlideRef {
    /// Index into `Document::groups()`.
    pub group: usize,
    /// Index into that group's slides.
    pub slide: usize,
    /// Index into `Document::slides()` (all slides in natural order).
    pub flat: usize,
    pub enabled: bool,
}

/// Slides in play order. `arrangement` is an arrangement UUID; `None` or an unknown
/// UUID gives the natural order.
pub fn slide_order(doc: &Document, arrangement: Option<&str>) -> Vec<SlideRef> {
    let groups = doc.groups();
    let mut starts = Vec::with_capacity(groups.len());
    let mut n = 0;
    for g in groups {
        starts.push(n);
        n += g.slides().len();
    }
    let sequence: Vec<usize> = arrangement
        .and_then(|id| {
            doc.arrangements()
                .into_iter()
                .find(|a| a.uuid.eq_ignore_ascii_case(id))
        })
        .map(|a| a.groups)
        .unwrap_or_else(|| (0..groups.len()).collect());
    sequence
        .into_iter()
        .filter_map(|gi| groups.get(gi).map(|g| (gi, g)))
        .flat_map(|(gi, g)| {
            let start = starts[gi];
            g.slides().iter().enumerate().map(move |(si, s)| SlideRef {
                group: gi,
                slide: si,
                flat: start + si,
                enabled: s.enabled,
            })
        })
        .collect()
}

/// Next enabled position after `from` (or the first enabled one when `from` is `None`).
pub fn next_enabled(order: &[SlideRef], from: Option<usize>) -> Option<usize> {
    let start = from.map_or(0, |f| f + 1);
    (start..order.len()).find(|&i| order[i].enabled)
}

/// Previous enabled position before `from`.
pub fn prev_enabled(order: &[SlideRef], from: usize) -> Option<usize> {
    (0..from.min(order.len())).rev().find(|&i| order[i].enabled)
}

/// Where `slide` sits in `order`, choosing the occurrence nearest to `near`.
pub fn find_nearest(order: &[SlideRef], slide: SlideRef, near: usize) -> Option<usize> {
    order
        .iter()
        .enumerate()
        .filter(|(_, r)| r.flat == slide.flat)
        .min_by_key(|(i, _)| i.abs_diff(near))
        .map(|(i, _)| i)
}
