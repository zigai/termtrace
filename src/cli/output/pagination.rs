use super::{OutputPage, PageArgs};

pub(in crate::cli) fn paginate<T>(items: Vec<T>, page: &PageArgs) -> (Vec<T>, OutputPage, bool) {
    let total = items.len();
    let start = page.offset.min(total);
    let end = if page.all { total } else { start.saturating_add(page.limit).min(total) };
    let returned = end.saturating_sub(start);
    let partial = start != 0 || end < total;
    let next_offset = (end < total).then_some(end);
    let selected = items
        .into_iter()
        .skip(start)
        .take(returned)
        .collect::<Vec<_>>();

    return (
        selected,
        OutputPage {
            offset: start,
            limit: (!page.all).then_some(page.limit),
            total: Some(total),
            returned,
            truncated: end < total,
            next_offset,
        },
        partial,
    );
}
