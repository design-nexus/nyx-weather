//! Each severe-weather alert once.
//!
//! NWS sends one hazard as several alerts: a product has a segment per period (a Freeze
//! Warning tonight and another tomorrow night, each its own alert with the same text),
//! an update can sit beside the message it updates, and a point can be in a zone and a
//! county warned for the same event.

use crate::model::{AlertPeriod, WeatherAlert};
use std::cmp::Reverse;
use std::collections::HashSet;

fn ts(iso: Option<&str>) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(iso?).ok().map(|t| t.timestamp())
}

impl WeatherAlert {
    /// When it's in force, falling back to the message's own times.
    pub fn in_force(&self) -> Vec<AlertPeriod> {
        if self.periods.is_empty() {
            vec![AlertPeriod { onset: self.effective.clone(), ends: self.expires.clone() }]
        } else {
            self.periods.clone()
        }
    }

    /// Everything that names this alert: its message ids, the messages it updates and
    /// its VTEC events. A later alert sharing any of these is the same one.
    pub fn keys(&self) -> Vec<String> {
        let mut keys = vec![self.id.clone()];
        keys.extend(self.merged_ids.iter().cloned());
        keys.extend(self.references.iter().cloned());
        keys.extend(self.vtec.iter().map(|v| format!("vtec:{v}")));
        keys.sort();
        keys.dedup();
        keys
    }

    fn starts(&self) -> Option<i64> {
        self.in_force().iter().filter_map(|p| ts(p.onset.as_deref())).min()
    }

    /// The alert at `now_unix` without the periods already over; None once all are.
    pub(crate) fn advanced_to(mut self, now_unix: i64) -> Option<Self> {
        let live = |iso: Option<&str>| ts(iso).is_none_or(|e| e > now_unix);
        if self.periods.is_empty() {
            return live(self.expires.as_deref()).then_some(self);
        }
        self.periods.retain(|p| live(p.ends.as_deref()));
        (!self.periods.is_empty()).then_some(self)
    }
}

fn same_hazard(a: &WeatherAlert, b: &WeatherAlert) -> bool {
    a.event.eq_ignore_ascii_case(&b.event) && a.sender == b.sender
}

fn push_distinct<T: PartialEq>(into: &mut Vec<T>, items: impl IntoIterator<Item = T>) {
    for item in items {
        if !into.contains(&item) {
            into.push(item);
        }
    }
}

/// One alert from a group of the same hazard, newest message first: the newest text,
/// every period and headline in time order, and the highest severity.
fn fold(group: Vec<WeatherAlert>) -> WeatherAlert {
    let mut by_start: Vec<&WeatherAlert> = group.iter().collect();
    by_start.sort_by_key(|a| a.starts());
    let mut out = group[0].clone();
    out.severity = group.iter().map(|a| a.severity).max().unwrap_or_default();

    let mut periods = Vec::new();
    push_distinct(&mut periods, by_start.iter().flat_map(|a| a.in_force()));
    periods.sort_by_key(|p| ts(p.onset.as_deref()));
    out.periods = periods;

    let mut headlines = Vec::new();
    push_distinct(&mut headlines, by_start.iter().flat_map(|a| a.headline.lines()).filter(|h| !h.is_empty()));
    out.headline = headlines.join("\n");
    let mut texts = Vec::new();
    push_distinct(&mut texts, group.iter().map(|a| a.description.trim()).filter(|d| !d.is_empty()));
    out.description = texts.join("\n\n");
    out.instruction = group.iter().find_map(|a| a.instruction.clone());
    out.expires = group.iter().filter_map(|a| a.expires.clone()).max_by_key(|e| ts(Some(e)));

    let mut ids = Vec::new();
    push_distinct(
        &mut ids,
        group.iter().flat_map(|a| if a.merged_ids.is_empty() { vec![a.id.clone()] } else { a.merged_ids.clone() }),
    );
    out.merged_ids = ids;
    let (mut refs, mut vtec) = (Vec::new(), Vec::new());
    push_distinct(&mut refs, group.iter().flat_map(|a| a.references.iter().cloned()));
    push_distinct(&mut vtec, group.iter().flat_map(|a| a.vtec.iter().cloned()));
    out.references = refs;
    out.vtec = vtec;
    out
}

/// Each alert once: drop the messages another in the list updates, then fold the alerts
/// for the same event from the same office into one. Most severe first, then soonest.
pub fn merge_alerts(alerts: Vec<WeatherAlert>) -> Vec<WeatherAlert> {
    let replaced: HashSet<String> = alerts.iter().flat_map(|a| a.references.iter().cloned()).collect();
    let mut current: Vec<WeatherAlert> = alerts.into_iter().filter(|a| !replaced.contains(&a.id)).collect();
    current.sort_by_key(|a| Reverse(ts(a.sent.as_deref().or(a.effective.as_deref()))));

    let mut groups: Vec<Vec<WeatherAlert>> = Vec::new();
    for a in current {
        match groups.iter_mut().find(|g| same_hazard(&g[0], &a)) {
            Some(g) => g.push(a),
            None => groups.push(vec![a]),
        }
    }
    let mut merged: Vec<WeatherAlert> = groups.into_iter().map(fold).collect();
    merged.sort_by(|a, b| b.severity.cmp(&a.severity).then_with(|| a.starts().cmp(&b.starts())));
    merged
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::AlertSeverity;
    use crate::noaa::NoaaAlertProvider;

    fn alert(id: &str, event: &str, sent: &str, onset: &str, ends: &str) -> WeatherAlert {
        WeatherAlert {
            id: id.into(),
            event: event.into(),
            headline: format!("{event} until {ends}"),
            description: "Cold.".into(),
            severity: AlertSeverity::Severe,
            sender: Some("NWS Birmingham AL".into()),
            sent: Some(sent.into()),
            periods: vec![AlertPeriod { onset: Some(onset.into()), ends: Some(ends.into()) }],
            ..Default::default()
        }
    }

    /// api.weather.gov for Calera, AL on 13 November 2026: one Freeze Warning product
    /// with a segment for each night.
    #[test]
    fn one_freeze_warning_for_two_nights() {
        let json: serde_json::Value =
            serde_json::from_str(include_str!("../testdata/nws-calera-freeze-2026-11-13.json")).unwrap();
        let alerts = NoaaAlertProvider::parse_alerts_json(&json).unwrap();
        assert_eq!(alerts.len(), 1);
        let a = &alerts[0];
        assert_eq!(a.event, "Freeze Warning");
        assert_eq!(a.periods.len(), 2);
        assert_eq!(a.periods[0].onset.as_deref(), Some("2026-11-14T00:00:00-06:00"));
        assert_eq!(a.periods[0].ends.as_deref(), Some("2026-11-14T08:00:00-06:00"));
        assert_eq!(a.periods[1].onset.as_deref(), Some("2026-11-15T00:00:00-06:00"));
        assert_eq!(a.periods[1].ends.as_deref(), Some("2026-11-15T08:00:00-06:00"));
        assert_eq!(a.merged_ids.len(), 2);
        assert!(a.vtec.contains(&"KBMX.FZ.W.0002".to_string()) && a.vtec.contains(&"KBMX.FZ.W.0003".to_string()));
        assert_eq!(a.description.matches("* WHAT...").count(), 1);
        assert_eq!(a.headline.lines().count(), 2);
        assert!(a.headline.lines().next().unwrap().contains("until November 14"));
    }

    #[test]
    fn an_update_replaces_what_it_references() {
        let old = alert("A.1", "Wind Advisory", "2026-11-13T03:00:00-06:00", "2026-11-13T06:00:00-06:00", "2026-11-13T18:00:00-06:00");
        let mut new = alert("A.2", "Wind Advisory", "2026-11-13T09:00:00-06:00", "2026-11-13T06:00:00-06:00", "2026-11-13T21:00:00-06:00");
        new.references = vec!["A.1".into()];
        let merged = merge_alerts(vec![old, new]);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].id, "A.2");
        assert_eq!(merged[0].periods.len(), 1);
        assert_eq!(merged[0].periods[0].ends.as_deref(), Some("2026-11-13T21:00:00-06:00"));
    }

    #[test]
    fn the_same_event_twice_keeps_the_newest_text() {
        let mut zone = alert("Z", "Flood Warning", "2026-11-13T03:00:00-06:00", "2026-11-13T06:00:00-06:00", "2026-11-14T06:00:00-06:00");
        zone.description = "Old text.".into();
        let mut county = alert("C", "Flood Warning", "2026-11-13T09:00:00-06:00", "2026-11-13T06:00:00-06:00", "2026-11-14T06:00:00-06:00");
        county.description = "New text.".into();
        let merged = merge_alerts(vec![zone, county]);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].id, "C");
        assert_eq!(merged[0].periods.len(), 1);
        assert_eq!(merged[0].description, "New text.\n\nOld text.");
        assert_eq!(merged[0].merged_ids, ["C", "Z"]);
    }

    #[test]
    fn different_events_stay_apart_most_severe_first() {
        let mut watch = alert("W", "Winter Storm Watch", "2026-11-13T09:00:00-06:00", "2026-11-14T00:00:00-06:00", "2026-11-14T12:00:00-06:00");
        watch.severity = AlertSeverity::Moderate;
        let warning = alert("F", "Freeze Warning", "2026-11-13T09:00:00-06:00", "2026-11-15T00:00:00-06:00", "2026-11-15T08:00:00-06:00");
        let other_office = WeatherAlert { id: "G".into(), sender: Some("NWS Atlanta GA".into()), ..warning.clone() };
        let merged = merge_alerts(vec![watch, warning, other_office]);
        assert_eq!(merged.iter().map(|a| a.id.as_str()).collect::<Vec<_>>(), ["F", "G", "W"]);
    }

    #[test]
    fn merging_twice_changes_nothing() {
        let json: serde_json::Value =
            serde_json::from_str(include_str!("../testdata/nws-calera-freeze-2026-11-13.json")).unwrap();
        let once = NoaaAlertProvider::parse_alerts_json(&json).unwrap();
        assert_eq!(merge_alerts(once.clone()), once);
    }

    #[test]
    fn past_periods_drop_and_keys_name_every_message() {
        let a = alert("F.1", "Freeze Warning", "2026-11-13T09:00:00-06:00", "2026-11-14T00:00:00-06:00", "2026-11-14T08:00:00-06:00");
        let b = alert("F.2", "Freeze Warning", "2026-11-13T09:00:00-06:00", "2026-11-15T00:00:00-06:00", "2026-11-15T08:00:00-06:00");
        let merged = merge_alerts(vec![a, b]).remove(0);
        let saturday_noon = ts(Some("2026-11-14T12:00:00-06:00")).unwrap();
        let later = merged.clone().advanced_to(saturday_noon).unwrap();
        assert_eq!(later.periods.len(), 1);
        assert_eq!(later.periods[0].onset.as_deref(), Some("2026-11-15T00:00:00-06:00"));
        assert!(merged.advanced_to(saturday_noon + 2 * 86_400).is_none());
        let keys = later.keys();
        assert!(keys.contains(&"F.1".to_string()) && keys.contains(&"F.2".to_string()));
    }
}
