use std::collections::HashMap;

use mongodb::bson::{Document, doc};

use crate::error::AppError;

const DEFAULT_LIMIT: i64 = 20;
const MAX_LIMIT: i64 = 100;

/// Fields a client may sort URLs by. The old app passed unknown query
/// parameters straight into the Mongo filter; whitelisting replaces that
/// deliberately, and a whitelist is per-collection because the fields are.
const URL_SORTABLE: &[&str] = &[
    "code",
    "url",
    "hits",
    "last_hit_at",
    "expires_at",
    "created_at",
    "updated_at",
];

/// Fields a client may sort users by.
///
/// `url_count` is deliberately absent: it only exists after the `$lookup`, so
/// sorting by it would mean joining every account in the collection before
/// paging rather than joining the twenty rows on the page.
const USER_SORTABLE: &[&str] = &["username", "created_at", "is_admin"];

pub struct ListParams {
    pub limit: i64,
    pub skip: i64,
    pub sort: Document,
    pub q: Option<String>,
}

fn number(raw: Option<&String>, field: &str) -> Result<Option<i64>, AppError> {
    match raw {
        None => Ok(None),
        Some(value) => value
            .parse::<i64>()
            .map(Some)
            .map_err(|_| AppError::validation(format!("{field} must be a number"))),
    }
}

/// `limit` and `skip` mean the same thing for every collection.
fn paging(params: &HashMap<String, String>) -> Result<(i64, i64), AppError> {
    let limit = number(params.get("limit"), "limit")?
        .unwrap_or(DEFAULT_LIMIT)
        .clamp(1, MAX_LIMIT);
    let skip = number(params.get("skip"), "skip")?.unwrap_or(0);
    if skip < 0 {
        return Err(AppError::validation("skip must not be negative"));
    }
    Ok((limit, skip))
}

/// Parses `field,dir,field,dir…` against `allowed`, falling back to `default`.
fn sort_doc(
    params: &HashMap<String, String>,
    allowed: &[&str],
    default: Document,
) -> Result<Document, AppError> {
    let Some(raw) = params.get("sort") else {
        return Ok(default);
    };
    let fields: Vec<&str> = raw.split(',').collect();
    if fields.is_empty() || !fields.len().is_multiple_of(2) {
        return Err(AppError::validation(
            "sort must be pairs of field and direction, e.g. hits,-1",
        ));
    }
    let mut sort = Document::new();
    for pair in fields.chunks(2) {
        let (field, dir) = (pair[0], pair[1]);
        if !allowed.contains(&field) {
            return Err(AppError::validation(format!("cannot sort by '{field}'")));
        }
        let dir: i32 = dir
            .parse()
            .ok()
            .filter(|d| *d == 1 || *d == -1)
            .ok_or_else(|| AppError::validation("sort direction must be 1 or -1"))?;
        sort.insert(field, dir);
    }
    Ok(sort)
}

fn search_term(params: &HashMap<String, String>) -> Option<String> {
    params
        .get("q")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Case-insensitive substring match across `fields`.
///
/// Escaped so a search for "a.b" cannot become a wildcard, and so a
/// pathological pattern cannot be smuggled in.
fn any_field_matches(q: &str, fields: &[&str]) -> Document {
    let pattern = regex::escape(q);
    let branches: Vec<Document> = fields
        .iter()
        .map(|field| doc! {*field: {"$regex": &pattern, "$options": "i"}})
        .collect();
    doc! {"$or": branches}
}

impl ListParams {
    pub fn from_query(params: &HashMap<String, String>) -> Result<Self, AppError> {
        let (limit, skip) = paging(params)?;

        // Most recently touched first. Links written before `updated_at`
        // existed have none, and Mongo sorts a missing field last under `-1`,
        // so legacy rows settle at the bottom rather than the top.
        let sort = sort_doc(params, URL_SORTABLE, doc! {"updated_at": -1})?;
        let q = search_term(params);

        Ok(Self {
            limit,
            skip,
            sort,
            q,
        })
    }

    /// Mongo filter for this page. `owner` scopes to one user; `None` means
    /// every URL (admins only).
    pub fn filter(&self, owner: Option<&str>) -> Document {
        let mut filter = Document::new();
        if let Some(owner) = owner {
            filter.insert("owner", owner);
        }
        if let Some(q) = &self.q {
            filter.extend(any_field_matches(q, &["code", "url"]));
        }
        filter
    }
}

/// The users list has its own whitelist and its own searchable fields —
/// sharing `ListParams` would let `?sort=hits,-1` through (a field users do
/// not have) while refusing `?sort=username,1` (the obvious one).
pub struct UserListParams {
    pub limit: i64,
    pub skip: i64,
    pub sort: Document,
    pub q: Option<String>,
}

impl UserListParams {
    pub fn from_query(params: &HashMap<String, String>) -> Result<Self, AppError> {
        let (limit, skip) = paging(params)?;
        let sort = sort_doc(params, USER_SORTABLE, doc! {"created_at": -1})?;
        let q = search_term(params);
        Ok(Self {
            limit,
            skip,
            sort,
            q,
        })
    }

    /// Matches the name someone would search by. Not the provider ids, which
    /// are secrets, and not `hash_passwd` for the obvious reason.
    pub fn filter(&self) -> Document {
        match &self.q {
            Some(q) => any_field_matches(q, &["username", "display_name", "email"]),
            None => Document::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn params(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn defaults_when_nothing_is_supplied() {
        let parsed = ListParams::from_query(&params(&[])).unwrap();
        assert_eq!(parsed.limit, 20);
        assert_eq!(parsed.skip, 0);
        assert_eq!(parsed.sort, doc! {"updated_at": -1});
        assert!(parsed.q.is_none());
    }

    #[test]
    fn limit_is_capped_and_floored() {
        assert_eq!(
            ListParams::from_query(&params(&[("limit", "500")]))
                .unwrap()
                .limit,
            100
        );
        assert_eq!(
            ListParams::from_query(&params(&[("limit", "0")]))
                .unwrap()
                .limit,
            1
        );
        assert!(ListParams::from_query(&params(&[("limit", "abc")])).is_err());
        assert!(ListParams::from_query(&params(&[("skip", "-1")])).is_err());
    }

    #[test]
    fn sort_accepts_whitelisted_fields_only() {
        let parsed = ListParams::from_query(&params(&[("sort", "hits,-1,code,1")])).unwrap();
        assert_eq!(parsed.sort, doc! {"hits": -1, "code": 1});
        // Anything not on the whitelist is refused rather than passed to Mongo.
        assert!(ListParams::from_query(&params(&[("sort", "hash_passwd,1")])).is_err());
        assert!(ListParams::from_query(&params(&[("sort", "code")])).is_err());
        assert!(ListParams::from_query(&params(&[("sort", "code,sideways")])).is_err());
    }

    #[test]
    fn search_escapes_regex_metacharacters() {
        let filter = ListParams::from_query(&params(&[("q", "a.b*c")]))
            .unwrap()
            .filter(None);
        // Both branches of the $or must carry the escaped pattern, or `.` and
        // `*` would silently become wildcards.
        let branches = filter.get_array("$or").unwrap();
        assert_eq!(branches.len(), 2);
        for (branch, field) in branches.iter().zip(["code", "url"]) {
            let pattern = branch
                .as_document()
                .unwrap()
                .get_document(field)
                .unwrap()
                .get_str("$regex")
                .unwrap();
            assert_eq!(pattern, r"a\.b\*c");
        }
    }

    #[test]
    fn search_and_owner_scope_apply_together() {
        let filter = ListParams::from_query(&params(&[("q", "x")]))
            .unwrap()
            .filter(Some("u1"));
        assert_eq!(filter.get_str("owner").unwrap(), "u1");
        assert!(
            filter.contains_key("$or"),
            "searching must not drop the owner scope"
        );
    }

    #[test]
    fn filter_scopes_to_the_owner_when_given() {
        let parsed = ListParams::from_query(&params(&[])).unwrap();
        assert_eq!(parsed.filter(Some("u1")), doc! {"owner": "u1"});
        assert_eq!(parsed.filter(None), doc! {});
    }

    #[test]
    fn users_default_to_newest_first_and_share_the_paging_rules() {
        let parsed = UserListParams::from_query(&params(&[])).unwrap();
        assert_eq!(parsed.limit, 20);
        assert_eq!(parsed.skip, 0);
        // Not `updated_at`: accounts do not have one.
        assert_eq!(parsed.sort, doc! {"created_at": -1});
        assert!(parsed.q.is_none());

        assert_eq!(
            UserListParams::from_query(&params(&[("limit", "500")]))
                .unwrap()
                .limit,
            100
        );
        assert!(UserListParams::from_query(&params(&[("skip", "-1")])).is_err());
    }

    /// The point of a separate type: the two whitelists must not overlap where
    /// the collections do not.
    #[test]
    fn users_and_urls_do_not_share_a_sort_whitelist() {
        let parsed = UserListParams::from_query(&params(&[("sort", "username,1")])).unwrap();
        assert_eq!(parsed.sort, doc! {"username": 1});

        // Valid for URLs, meaningless for users.
        assert!(UserListParams::from_query(&params(&[("sort", "hits,-1")])).is_err());
        // Only exists after the `$lookup`, so sorting by it would join the
        // whole collection before paging.
        assert!(UserListParams::from_query(&params(&[("sort", "url_count,-1")])).is_err());
        assert!(UserListParams::from_query(&params(&[("sort", "hash_passwd,1")])).is_err());

        // And the reverse: a user field is not sortable on URLs.
        assert!(ListParams::from_query(&params(&[("sort", "username,1")])).is_err());
    }

    #[test]
    fn users_are_searched_by_name_and_email_only() {
        let filter = UserListParams::from_query(&params(&[("q", "a.b")]))
            .unwrap()
            .filter();
        let branches = filter.get_array("$or").unwrap();
        let fields: Vec<&str> = branches
            .iter()
            .map(|branch| branch.as_document().unwrap().keys().next().unwrap().as_str())
            .collect();
        assert_eq!(fields, ["username", "display_name", "email"]);
        // Never the provider ids or the hash, however tempting as a lookup.
        assert!(!fields.contains(&"github_id"));
        assert!(!fields.contains(&"hash_passwd"));

        let pattern = branches[0]
            .as_document()
            .unwrap()
            .get_document("username")
            .unwrap()
            .get_str("$regex")
            .unwrap();
        assert_eq!(pattern, r"a\.b");
    }

    /// An unsearched list must match everything, not nothing.
    #[test]
    fn users_without_a_search_are_unfiltered() {
        let parsed = UserListParams::from_query(&params(&[("q", "   ")])).unwrap();
        assert!(parsed.q.is_none(), "whitespace is not a search");
        assert_eq!(parsed.filter(), doc! {});
    }
}
