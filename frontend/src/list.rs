//! Paging, searching and sorting shared by the table pages.

use std::time::Duration;

use leptos::prelude::*;

pub const PAGE_SIZE: u64 = 20;
/// Long enough that typing a word is one request, short enough to feel live.
pub const SEARCH_DEBOUNCE: Duration = Duration::from_millis(300);
/// Far enough up that rows arrive before the scrollbar reaches the end.
pub const LOAD_MORE_MARGIN: f64 = 200.0;
/// Placeholder rows while the first page of a query is on its way.
pub const GHOST_ROWS: usize = 6;
/// A response beating this draws no skeletons, so a fast query cannot flash.
pub const GHOST_DELAY: Duration = Duration::from_millis(200);

/// A column the server will sort by; each endpoint whitelists its own, and
/// anything else comes back 400.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sort {
    pub field: &'static str,
    pub desc: bool,
}

/// Ascending, descending, then no explicit sort — three clicks return to where
/// you started. `None` means whatever the server sorts by, which is why the
/// signal holds an `Option`: a default here would swallow the third click.
pub fn cycled(current: Option<Sort>, field: &'static str) -> Option<Sort> {
    match current {
        Some(sort) if sort.field == field => {
            if sort.desc {
                None
            } else {
                Some(Sort { field, desc: true })
            }
        }
        _ => Some(Sort { field, desc: false }),
    }
}

/// What the URL table offers as a sort, for reading one back out of a hash.
pub const URL_SORT_FIELDS: &[&str] = &[
    "code",
    "url",
    "hits",
    "last_hit_at",
    "expires_at",
    "created_at",
    "updated_at",
];

pub const USER_SORT_FIELDS: &[&str] = &["username", "created_at", "url_count"];

/// The field must be one of `allowed`, whose entries outlive the parse.
pub fn parse_sort(raw: &str, allowed: &[&'static str]) -> Option<Sort> {
    let (field, direction) = raw.split_once(',')?;
    let field = allowed.iter().copied().find(|known| *known == field)?;
    match direction {
        "1" => Some(Sort { field, desc: false }),
        "-1" => Some(Sort { field, desc: true }),
        _ => None,
    }
}

pub fn sort_param(sort: Sort) -> String {
    format!("{},{}", sort.field, if sort.desc { -1 } else { 1 })
}

/// One page as query parameters; an unset sort is left out so the server
/// applies its own. Pairs, not a finished string: `gloo_net` assembles the URL
/// itself and would leave a trailing `&` on every request.
pub fn list_params(page: u64, search: &str, sort: Option<Sort>) -> Vec<(String, String)> {
    let mut params = vec![
        ("limit".to_string(), PAGE_SIZE.to_string()),
        ("skip".to_string(), (page * PAGE_SIZE).to_string()),
    ];
    if let Some(sort) = sort {
        params.push(("sort".to_string(), sort_param(sort)));
    }
    if !search.trim().is_empty() {
        params.push(("q".to_string(), search.trim().to_string()));
    }
    params
}

/// True only when there is something to select and all of it is selected — an
/// empty table must not show a ticked "select all". Shared by both table pages,
/// which key their selection by whatever identifies a row: a code, or an id.
pub fn all_selected(ids: &[String], selected: &std::collections::HashSet<String>) -> bool {
    !ids.is_empty() && ids.iter().all(|id| selected.contains(id))
}

/// `2026-08-01T18:34:37.937Z` reads better as `2026-08-01 18:34`. Anything that
/// is not the expected shape is shown as-is rather than mangled.
pub fn short_datetime(raw: Option<&String>) -> String {
    let Some(value) = raw else {
        return "—".into();
    };
    match value.split_once('T') {
        Some((date, time)) if time.len() >= 5 => format!("{date} {}", &time[..5]),
        _ => value.clone(),
    }
}

/// A column heading that cycles the sort when clicked.
#[component]
pub fn SortHeader(
    field: &'static str,
    label: &'static str,
    sort: RwSignal<Option<Sort>>,
    /// Fixed width for the column, so it does not jump when the cell swaps
    /// between text, an edit input and a loading skeleton.
    #[prop(default = "")]
    width: &'static str,
) -> impl IntoView {
    let icon = move || match sort.get() {
        Some(current) if current.field == field && current.desc => {
            "icon-[tabler--sort-descending] text-primary"
        }
        Some(current) if current.field == field => "icon-[tabler--sort-ascending] text-primary",
        _ => "icon-[tabler--arrows-sort] opacity-30",
    };

    view! {
        <th class=format!("p-0 {width}")>
            <button
                class="flex overflow-hidden gap-1 items-center py-3 px-3 w-full text-xs font-semibold tracking-wide uppercase hover:bg-base-200"
                on:click=move |_| sort.update(|s| *s = cycled(*s, field))
            >
                <span class="truncate">{label}</span>
                <span class=move || format!("{} size-4", icon())></span>
            </button>
        </th>
    }
}

/// One placeholder row, shown while the first page of a new query loads.
#[component]
pub fn GhostRow(columns: usize) -> impl IntoView {
    view! {
        <tr>
            {(0..columns)
                .map(|_| {
                    view! {
                        <td>
                            <span class="block w-full h-4 rounded animate-pulse bg-base-content/10"></span>
                        </td>
                    }
                })
                .collect_view()}
        </tr>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BY_CODE: Sort = Sort {
        field: "code",
        desc: false,
    };

    fn params(page: u64, search: &str, sort: Option<Sort>) -> Vec<(String, String)> {
        list_params(page, search, sort)
    }

    #[test]
    fn a_sort_survives_the_address_bar_only_as_a_column_that_exists() {
        assert_eq!(parse_sort("code,1", URL_SORT_FIELDS), Some(BY_CODE));
        assert_eq!(
            parse_sort("hits,-1", URL_SORT_FIELDS),
            Some(Sort {
                field: "hits",
                desc: true
            })
        );
        assert_eq!(parse_sort("hash_passwd,1", URL_SORT_FIELDS), None);
        assert_eq!(parse_sort("hits,1", USER_SORT_FIELDS), None);
        assert_eq!(parse_sort("code,sideways", URL_SORT_FIELDS), None);
        assert_eq!(parse_sort("code", URL_SORT_FIELDS), None);
        assert_eq!(sort_param(BY_CODE), "code,1");
    }

    #[test]
    fn the_query_carries_paging_sort_and_only_a_real_search() {
        assert_eq!(
            params(0, "", None),
            [
                ("limit".to_string(), "20".to_string()),
                ("skip".to_string(), "0".to_string())
            ]
        );
        // Whitespace is not a search.
        assert_eq!(
            params(2, "   ", None),
            [
                ("limit".to_string(), "20".to_string()),
                ("skip".to_string(), "40".to_string())
            ]
        );
        assert_eq!(
            params(0, "", Some(BY_CODE)),
            [
                ("limit".to_string(), "20".to_string()),
                ("skip".to_string(), "0".to_string()),
                ("sort".to_string(), "code,1".to_string())
            ]
        );
        assert_eq!(
            params(
                1,
                " needle ",
                Some(Sort {
                    field: "hits",
                    desc: true
                })
            ),
            [
                ("limit".to_string(), "20".to_string()),
                ("skip".to_string(), "20".to_string()),
                ("sort".to_string(), "hits,-1".to_string()),
                ("q".to_string(), "needle".to_string())
            ]
        );
    }

    /// Three clicks on one column return to the starting state.
    #[test]
    fn a_column_cycles_ascending_descending_then_off() {
        let first = cycled(None, "code");
        assert_eq!(first, Some(BY_CODE));
        let second = cycled(first, "code");
        assert_eq!(
            second,
            Some(Sort {
                field: "code",
                desc: true
            })
        );
        assert_eq!(cycled(second, "code"), None, "the third click clears it");
    }

    #[test]
    fn a_different_column_starts_ascending_rather_than_inheriting() {
        let descending = Some(Sort {
            field: "code",
            desc: true,
        });
        assert_eq!(
            cycled(descending, "hits"),
            Some(Sort {
                field: "hits",
                desc: false
            })
        );
    }

    #[test]
    fn timestamps_are_shortened_to_the_minute() {
        assert_eq!(
            short_datetime(Some(&"2026-08-01T18:34:37.937Z".to_string())),
            "2026-08-01 18:34"
        );
        assert_eq!(short_datetime(None), "—");
        // Anything unexpected is shown rather than mangled.
        assert_eq!(short_datetime(Some(&"whenever".to_string())), "whenever");
    }
}
