use std::collections::HashMap;

use mongodb::bson::{Document, doc};

use crate::error::AppError;

const DEFAULT_LIMIT: i64 = 20;
const MAX_LIMIT: i64 = 100;

/// Fields a client may sort by. The old app passed unknown query parameters
/// straight into the Mongo filter; whitelisting replaces that deliberately.
const SORTABLE: &[&str] = &[
    "code",
    "url",
    "hits",
    "last_hit_at",
    "expires_at",
    "created_at",
];

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

impl ListParams {
    pub fn from_query(params: &HashMap<String, String>) -> Result<Self, AppError> {
        let limit = number(params.get("limit"), "limit")?
            .unwrap_or(DEFAULT_LIMIT)
            .clamp(1, MAX_LIMIT);
        let skip = number(params.get("skip"), "skip")?.unwrap_or(0);
        if skip < 0 {
            return Err(AppError::validation("skip must not be negative"));
        }

        let sort = match params.get("sort") {
            None => doc! {"created_at": -1},
            Some(raw) => {
                let fields: Vec<&str> = raw.split(',').collect();
                if fields.is_empty() || !fields.len().is_multiple_of(2) {
                    return Err(AppError::validation(
                        "sort must be pairs of field and direction, e.g. hits,-1",
                    ));
                }
                let mut sort = Document::new();
                for pair in fields.chunks(2) {
                    let (field, dir) = (pair[0], pair[1]);
                    if !SORTABLE.contains(&field) {
                        return Err(AppError::validation(format!("cannot sort by '{field}'")));
                    }
                    let dir: i32 = dir
                        .parse()
                        .ok()
                        .filter(|d| *d == 1 || *d == -1)
                        .ok_or_else(|| AppError::validation("sort direction must be 1 or -1"))?;
                    sort.insert(field, dir);
                }
                sort
            }
        };

        let q = params
            .get("q")
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());

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
            // Escaped so a search for "a.b" cannot become a wildcard, and so a
            // pathological pattern cannot be smuggled in.
            let pattern = regex::escape(q);
            filter.insert(
                "$or",
                vec![
                    doc! {"code": {"$regex": &pattern, "$options": "i"}},
                    doc! {"url": {"$regex": &pattern, "$options": "i"}},
                ],
            );
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
        assert_eq!(parsed.sort, doc! {"created_at": -1});
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
}
