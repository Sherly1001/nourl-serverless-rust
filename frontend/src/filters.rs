//! The filter modal's state, in the parameter names the server and the hash share.

use crate::datetime::{from_display, from_rfc3339, to_display, to_rfc3339};

#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub enum OwnerChoice {
    #[default]
    Any,
    Mine,
    Unowned,
    Usernames(Vec<String>),
}

#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub enum ExpiryChoice {
    #[default]
    Any,
    Never,
    Between(DateRange),
}

/// As [`crate::components::datepicker`] holds them: `yyyy/mm/dd hh:mm`, in the
/// visitor's zone.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct DateRange {
    pub from: String,
    pub to: String,
}

impl DateRange {
    fn is_empty(&self) -> bool {
        self.from.trim().is_empty() && self.to.trim().is_empty()
    }

    fn params(&self, keys: (&str, &str), offset: i32) -> Vec<(String, String)> {
        let mut pairs = Vec::new();
        if let Some(from) = instant(&self.from, offset) {
            pairs.push((keys.0.to_string(), from));
        }
        if let Some(to) = instant(&self.to, offset) {
            pairs.push((keys.1.to_string(), to));
        }
        pairs
    }

    fn read(pairs: &[(String, String)], keys: (&str, &str), offset: i32) -> Self {
        Self {
            from: first(pairs, keys.0)
                .and_then(|v| shown(v, offset))
                .unwrap_or_default(),
            to: first(pairs, keys.1)
                .and_then(|v| shown(v, offset))
                .unwrap_or_default(),
        }
    }
}

/// What the picker shows, as the instant the server reads.
fn instant(shown: &str, offset: i32) -> Option<String> {
    from_display(shown.trim()).and_then(|local| to_rfc3339(&local, offset))
}

/// The reverse. A stamp this page wrote is already local; one typed into the
/// address bar may be UTC, a different day either side of midnight.
fn shown(stamp: &str, offset: i32) -> Option<String> {
    let local = if stamp.ends_with('Z') {
        from_rfc3339(stamp, offset)?
    } else {
        let (date, time) = stamp.split_once('T')?;
        format!("{date}T{}", time.get(..5)?)
    };
    to_display(&local)
}

fn first<'a>(pairs: &'a [(String, String)], key: &str) -> Option<&'a str> {
    pairs
        .iter()
        .find(|(k, v)| k == key && !v.trim().is_empty())
        .map(|(_, v)| v.trim())
}

fn all(pairs: &[(String, String)], key: &str) -> Vec<String> {
    pairs
        .iter()
        .filter(|(k, v)| k == key && !v.trim().is_empty())
        .map(|(_, v)| v.trim().to_string())
        .collect()
}

fn number(pairs: &[(String, String)], key: &str) -> Option<i64> {
    first(pairs, key).and_then(|v| v.parse().ok())
}

pub const RANGE_BACKWARDS: &str = "The start is after the end.";
pub const HITS_BACKWARDS: &str = "The low end is above the high end.";
pub const RANGE_FUTURE: &str = "This field never holds a future date.";

impl DateRange {
    /// What is wrong with the range, if anything. `now` is the visitor's clock
    /// for the fields that record something that has already happened.
    pub fn problem(&self, now: Option<&str>) -> Option<&'static str> {
        let from = from_display(self.from.trim());
        let to = from_display(self.to.trim());
        if let (Some(from), Some(to)) = (&from, &to)
            && from > to
        {
            return Some(RANGE_BACKWARDS);
        }
        match now {
            Some(now) if from.iter().chain(to.iter()).any(|held| held.as_str() > now) => {
                Some(RANGE_FUTURE)
            }
            _ => None,
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Filters {
    pub owner: OwnerChoice,
    pub expiry: ExpiryChoice,
    pub hits_min: String,
    pub hits_max: String,
    pub last_hit: DateRange,
    pub updated: DateRange,
    pub created: DateRange,
    pub codes: Vec<String>,
    pub urls: Vec<String>,
}

impl Filters {
    pub fn active(&self) -> usize {
        [
            self.owner != OwnerChoice::Any,
            self.expiry != ExpiryChoice::Any,
            !self.hits_min.trim().is_empty() || !self.hits_max.trim().is_empty(),
            !self.last_hit.is_empty(),
            !self.updated.is_empty(),
            !self.created.is_empty(),
            !self.codes.is_empty(),
            !self.urls.is_empty(),
        ]
        .into_iter()
        .filter(|active| *active)
        .count()
    }

    pub fn to_params(&self, offset: i32) -> Vec<(String, String)> {
        let mut pairs = Vec::new();
        match &self.owner {
            OwnerChoice::Any => {}
            OwnerChoice::Mine => pairs.push(("mine".into(), "true".into())),
            OwnerChoice::Unowned => pairs.push(("owner_state".into(), "unowned".into())),
            OwnerChoice::Usernames(names) => {
                pairs.extend(names.iter().map(|name| ("owner".to_string(), name.clone())));
            }
        }
        match &self.expiry {
            ExpiryChoice::Any => {}
            ExpiryChoice::Never => pairs.push(("expiry".into(), "never".into())),
            ExpiryChoice::Between(range) => {
                pairs.extend(range.params(("expires_from", "expires_to"), offset));
            }
        }
        for (key, value) in [("hits_min", &self.hits_min), ("hits_max", &self.hits_max)] {
            if !value.trim().is_empty() {
                pairs.push((key.to_string(), value.trim().to_string()));
            }
        }
        pairs.extend(
            self.last_hit
                .params(("last_hit_from", "last_hit_to"), offset),
        );
        pairs.extend(self.updated.params(("updated_from", "updated_to"), offset));
        pairs.extend(self.created.params(("created_from", "created_to"), offset));
        pairs.extend(self.codes.iter().map(|v| ("code".to_string(), v.clone())));
        pairs.extend(self.urls.iter().map(|v| ("url".to_string(), v.clone())));
        pairs
    }

    /// Whether any range refuses to be applied. Expiry may reach into the
    /// future; the three that record what has happened may not.
    pub fn problem(&self, now: &str) -> Option<&'static str> {
        let expiry = match &self.expiry {
            ExpiryChoice::Between(range) => range.problem(None),
            _ => None,
        };
        expiry
            .or_else(|| self.hits_problem())
            .or_else(|| self.last_hit.problem(Some(now)))
            .or_else(|| self.updated.problem(Some(now)))
            .or_else(|| self.created.problem(Some(now)))
    }

    pub fn hits_problem(&self) -> Option<&'static str> {
        let min = self.hits_min.trim().parse::<i64>().ok();
        let max = self.hits_max.trim().parse::<i64>().ok();
        match (min, max) {
            (Some(min), Some(max)) if min > max => Some(HITS_BACKWARDS),
            _ => None,
        }
    }

    /// Anything unreadable is dropped: the address bar is editable by hand.
    pub fn from_pairs(pairs: &[(String, String)], offset: i32) -> Self {
        let named = all(pairs, "owner");
        let owner = match (first(pairs, "owner_state"), first(pairs, "mine")) {
            (Some("unowned"), _) => OwnerChoice::Unowned,
            (_, Some("true")) => OwnerChoice::Mine,
            _ if !named.is_empty() => OwnerChoice::Usernames(named),
            _ => OwnerChoice::Any,
        };
        let expiry = match first(pairs, "expiry") {
            Some("never") => ExpiryChoice::Never,
            _ => {
                let range = DateRange::read(pairs, ("expires_from", "expires_to"), offset);
                if range.is_empty() {
                    ExpiryChoice::Any
                } else {
                    ExpiryChoice::Between(range)
                }
            }
        };
        Self {
            owner,
            expiry,
            hits_min: number(pairs, "hits_min")
                .map(|n| n.to_string())
                .unwrap_or_default(),
            hits_max: number(pairs, "hits_max")
                .map(|n| n.to_string())
                .unwrap_or_default(),
            last_hit: DateRange::read(pairs, ("last_hit_from", "last_hit_to"), offset),
            updated: DateRange::read(pairs, ("updated_from", "updated_to"), offset),
            created: DateRange::read(pairs, ("created_from", "created_to"), offset),
            codes: all(pairs, "code"),
            urls: all(pairs, "url"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKYO: i32 = 9 * 60;

    fn pairs(of: &[(&str, &str)]) -> Vec<(String, String)> {
        of.iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn a_range_carries_the_zone_the_picker_wrote_it_in() {
        let filters = Filters {
            created: DateRange {
                from: "2026/09/01 00:00".into(),
                to: "2026/09/01 23:59".into(),
            },
            ..Filters::default()
        };
        assert_eq!(
            filters.to_params(TOKYO),
            pairs(&[
                ("created_from", "2026-09-01T00:00:00+09:00"),
                ("created_to", "2026-09-01T23:59:00+09:00"),
            ])
        );
    }

    #[test]
    fn every_item_survives_a_round_trip() {
        let filters = Filters {
            owner: OwnerChoice::Usernames(vec!["ann".into(), "bob".into()]),
            expiry: ExpiryChoice::Between(DateRange {
                from: "2026/01/02 08:30".into(),
                to: String::new(),
            }),
            hits_min: "0".into(),
            hits_max: "9".into(),
            last_hit: DateRange::default(),
            updated: DateRange::default(),
            created: DateRange {
                from: "2026/03/04 00:00".into(),
                to: "2026/05/06 17:45".into(),
            },
            codes: vec!["promo".into()],
            urls: vec!["example.com".into(), "other".into()],
        };
        let round = Filters::from_pairs(&filters.to_params(TOKYO), TOKYO);
        assert_eq!(round, filters);
        assert_eq!(round.active(), 6);
    }

    #[test]
    fn a_backwards_range_is_refused_and_a_future_one_only_where_it_cannot_happen() {
        let backwards = DateRange {
            from: "2026/09/02 00:00".into(),
            to: "2026/09/01 00:00".into(),
        };
        assert_eq!(backwards.problem(None), Some(RANGE_BACKWARDS));

        let ahead = DateRange {
            from: "2026/09/23 00:00".into(),
            to: String::new(),
        };
        assert_eq!(ahead.problem(Some("2026-09-22T10:00")), Some(RANGE_FUTURE));
        // An expiry is allowed to be ahead, and passes `None` for that reason.
        assert_eq!(ahead.problem(None), None);

        let filters = Filters {
            expiry: ExpiryChoice::Between(ahead.clone()),
            created: ahead,
            ..Filters::default()
        };
        assert_eq!(filters.problem("2026-09-22T10:00"), Some(RANGE_FUTURE));
        assert_eq!(Filters::default().problem("2026-09-22T10:00"), None);

        let hits = Filters {
            hits_min: "9".into(),
            hits_max: "2".into(),
            ..Filters::default()
        };
        assert_eq!(hits.hits_problem(), Some(HITS_BACKWARDS));
        assert_eq!(hits.problem("2026-09-22T10:00"), Some(HITS_BACKWARDS));
        // A single bound is a range with one open end, not a backwards one.
        assert_eq!(
            Filters {
                hits_min: "9".into(),
                ..Filters::default()
            }
            .hits_problem(),
            None
        );
    }

    #[test]
    fn the_owner_item_carries_mine_as_the_scope_the_server_already_knows() {
        let filters = Filters {
            owner: OwnerChoice::Mine,
            ..Filters::default()
        };
        assert_eq!(filters.to_params(TOKYO), pairs(&[("mine", "true")]));
        assert_eq!(
            Filters::from_pairs(&filters.to_params(TOKYO), TOKYO).owner,
            OwnerChoice::Mine
        );
        assert_eq!(filters.active(), 1);
    }

    #[test]
    fn the_unowned_choice_outranks_names_left_in_a_disabled_control() {
        let read = Filters::from_pairs(
            &pairs(&[("owner", "ann"), ("owner_state", "unowned")]),
            TOKYO,
        );
        assert_eq!(read.owner, OwnerChoice::Unowned);
    }

    /// A UTC midnight is the previous day in Tokyo.
    #[test]
    fn a_utc_stamp_is_read_as_the_local_time_it_is() {
        let read = Filters::from_pairs(
            &pairs(&[("created_from", "2026-09-01T00:00:00.000Z")]),
            TOKYO,
        );
        assert_eq!(read.created.from, "2026/09/01 09:00");
        let read = Filters::from_pairs(
            &pairs(&[("created_from", "2026-08-31T20:00:00.000Z")]),
            TOKYO,
        );
        assert_eq!(read.created.from, "2026/09/01 05:00");
    }

    #[test]
    fn unreadable_values_are_dropped_rather_than_kept() {
        let read = Filters::from_pairs(
            &pairs(&[
                ("hits_min", "lots"),
                ("created_from", "yesterday"),
                ("owner", "  "),
                ("nonsense", "1"),
            ]),
            TOKYO,
        );
        assert_eq!(read, Filters::default());
        assert_eq!(read.active(), 0);
        assert!(read.to_params(TOKYO).is_empty());
    }
}
