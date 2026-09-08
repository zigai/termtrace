use super::{OutputPage, PageArgs};

pub(super) fn pagination_projection(value: &serde_json::Value) -> Option<serde_json::Value> {
    let source = value.as_object()?;

    let mut projection = serde_json::Map::new();
    for key in [
        "kind",
        "type",
        "page",
        "output_page",
        "next_offset",
        "next_cursor",
    ] {
        if let Some(value) = source.get(key).cloned() {
            projection.insert(key.to_owned(), value);
        }
    }

    if !projection.contains_key("page")
        && !projection.contains_key("output_page")
        && let Some(results) = source.get("results").and_then(serde_json::Value::as_array)
        && let Some(page) = results
            .iter()
            .rev()
            .find_map(pagination_projection)
            .and_then(|value| return value.as_object().cloned())
    {
        for key in ["page", "output_page", "next_offset", "next_cursor"] {
            if let Some(value) = page.get(key).cloned() {
                projection.insert(key.to_owned(), value);
            }
        }
    }

    if !projection.contains_key("page")
        && !projection.contains_key("output_page")
        && !projection.contains_key("next_offset")
        && !projection.contains_key("next_cursor")
    {
        return None;
    }

    return Some(serde_json::Value::Object(projection));
}

pub(super) fn raw_pagination_projection(output: &[u8]) -> Option<serde_json::Value> {
    let line = str::from_utf8(output)
        .ok()?
        .lines()
        .rev()
        .find_map(|line| return line.strip_prefix("page: "))?;

    let page = serde_json::from_str::<serde_json::Value>(line).ok()?;

    return Some(serde_json::json!({ "page": page }));
}

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
