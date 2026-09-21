use std::collections::HashMap;

use mongodb::bson::{Document, doc};

use crate::error::AppError;

const DEFAULT_LIMIT: i64 = 20;
const MAX_LIMIT: i64 = 100;

/// Fields a client may sort URLs by. The old app passed unknown parameters
/// straight into the filter; per-collection, because the fields are.
const URL_SORTABLE: &[&str] = &[
    "code",
    "url",
    "hits",
    "last_hit_at",
    "expires_at",
    "created_at",
    "updated_at",
];

/// Fields a client may sort users by. `url_count` only exists after the join,
/// which is what [`UserListParams::sorts_by_join`] confines the cost of.
/// `admin_level` is derived from the chain, so there is no field to sort on.
const USER_SORTABLE: &[&str] = &["username", "created_at", "url_count"];

/// The one sortable field that does not exist until after the `$lookup`.
const JOINED_SORT_FIELD: &str = "url_count";

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

/// Parses `field,dir,…` against `allowed`. Every sort ends with `_id` so it is
/// total: without it, rows sharing a timestamp reshuffle between page loads.
fn sort_doc(
    params: &HashMap<String, String>,
    allowed: &[&str],
    default: Document,
) -> Result<Document, AppError> {
    let tiebreak = |mut sort: Document| {
        if !sort.contains_key("_id") {
            sort.insert("_id", 1);
        }
        sort
    };
    let Some(raw) = params.get("sort") else {
        return Ok(tiebreak(default));
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
    Ok(tiebreak(sort))
}

fn search_term(params: &HashMap<String, String>) -> Option<String> {
    params
        .get("q")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Case-insensitive substring match, escaped so "a.b" is not a wildcard.
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

        // Missing sorts last under `-1`, so legacy rows settle at the bottom.
        let sort = sort_doc(params, URL_SORTABLE, doc! {"updated_at": -1})?;
        let q = search_term(params);

        Ok(Self {
            limit,
            skip,
            sort,
            q,
        })
    }

    /// `owner` scopes to one user; `None` means every URL, admins only.
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

/// Its own whitelist: sharing `ListParams` would allow `?sort=hits,-1` and
/// refuse `?sort=username,1`.
pub struct UserListParams {
    pub limit: i64,
    pub skip: i64,
    pub sort: Document,
    pub q: Option<String>,
}

impl UserListParams {
    pub fn from_query(params: &HashMap<String, String>) -> Result<Self, AppError> {
        let (limit, skip) = paging(params)?;
        // A directory to look somebody up in, not a feed of recent signups.
        let sort = sort_doc(params, USER_SORTABLE, doc! {"username": 1})?;
        let q = search_term(params);
        Ok(Self {
            limit,
            skip,
            sort,
            q,
        })
    }

    /// Whether the sort needs the join first, which is the expensive shape —
    /// so callers check rather than always paying it.
    pub fn sorts_by_join(&self) -> bool {
        self.sort.contains_key(JOINED_SORT_FIELD)
    }

    /// The name someone would search by — never the provider ids or the hash.
    /// Excludes admins, who come back whole alongside this page and would
    /// otherwise be counted twice.
    pub fn filter(&self) -> Document {
        // `$ne`: an account predating the field has no `is_admin` at all.
        let mut filter = doc! {"is_admin": {"$ne": true}};
        if let Some(q) = &self.q {
            filter.extend(any_field_matches(q, &["username", "display_name", "email"]));
        }
        filter
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
        assert_eq!(parsed.sort, doc! {"updated_at": -1, "_id": 1});
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
        assert_eq!(parsed.sort, doc! {"hits": -1, "code": 1, "_id": 1});
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
        // Both branches need the escaped pattern, or `.` becomes a wildcard.
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
    fn users_default_to_alphabetical_and_share_the_paging_rules() {
        let parsed = UserListParams::from_query(&params(&[])).unwrap();
        assert_eq!(parsed.limit, 20);
        assert_eq!(parsed.skip, 0);
        // A directory, so alphabetical rather than newest first.
        assert_eq!(parsed.sort, doc! {"username": 1, "_id": 1});
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
        assert_eq!(parsed.sort, doc! {"username": 1, "_id": 1});
        // Derived from the chain, which these rows have none of.
        assert!(UserListParams::from_query(&params(&[("sort", "admin_level,1")])).is_err());
        assert!(UserListParams::from_query(&params(&[("sort", "promoted_by,1")])).is_err());
    }

    /// Ordering by the link count needs the join done before paging, and the
    /// pipeline is built differently for it, so the flag has to be right.
    #[test]
    fn only_the_link_count_forces_a_join_before_paging() {
        let by_count = UserListParams::from_query(&params(&[("sort", "url_count,-1")])).unwrap();
        assert_eq!(by_count.sort, doc! {"url_count": -1, "_id": 1});
        assert!(by_count.sorts_by_join());

        assert!(
            !UserListParams::from_query(&params(&[("sort", "username,1")]))
                .unwrap()
                .sorts_by_join()
        );
        assert!(
            !UserListParams::from_query(&params(&[]))
                .unwrap()
                .sorts_by_join()
        );
        // Even as the second key, it still decides the shape of the pipeline.
        assert!(
            UserListParams::from_query(&params(&[("sort", "username,1,url_count,-1")]))
                .unwrap()
                .sorts_by_join()
        );
    }

    /// The two whitelists must not overlap where the collections do not.
    #[test]
    fn users_and_urls_keep_separate_whitelists() {
        // Valid for URLs, meaningless for users.
        assert!(UserListParams::from_query(&params(&[("sort", "hits,-1")])).is_err());
        assert!(UserListParams::from_query(&params(&[("sort", "hash_passwd,1")])).is_err());
        // And the reverse: a user field is not sortable on URLs.
        assert!(ListParams::from_query(&params(&[("sort", "username,1")])).is_err());
        // `url_count` is a user field only — a URL does not have one.
        assert!(ListParams::from_query(&params(&[("sort", "url_count,-1")])).is_err());
    }

    #[test]
    fn users_are_searched_by_name_and_email_only() {
        let filter = UserListParams::from_query(&params(&[("q", "a.b")]))
            .unwrap()
            .filter();
        // They come back whole from `users::admins` and would count twice.
        assert_eq!(
            filter.get_document("is_admin").unwrap(),
            &doc! {"$ne": true}
        );
        let branches = filter.get_array("$or").unwrap();
        let fields: Vec<&str> = branches
            .iter()
            .map(|branch| {
                branch
                    .as_document()
                    .unwrap()
                    .keys()
                    .next()
                    .unwrap()
                    .as_str()
            })
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
        assert_eq!(parsed.filter(), doc! {"is_admin": {"$ne": true}});
    }
}
