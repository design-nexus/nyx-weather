//! Desktop notifications for new severe-weather alerts.

use crate::prefs;
use gtk::gio;
use gtk::prelude::*;
use nyx_weather_core::{AlertSeverity, ForecastSnapshot, WeatherAlert};

/// Alert keys remembered, so an alert is notified once even after it drops out and returns.
const REMEMBERED: usize = 300;

/// The Severe or Extreme alerts not notified before: none of their message ids, the
/// messages they update or their VTEC events has been, so an update or another segment
/// of the same warning stays quiet.
fn fresh<'a>(alerts: &'a [WeatherAlert], notified: &[String]) -> Vec<&'a WeatherAlert> {
    alerts
        .iter()
        .filter(|a| a.severity >= AlertSeverity::Severe && !a.keys().iter().any(|k| notified.contains(k)))
        .collect()
}

/// Notify each Severe or Extreme alert in `snap` not notified before.
pub fn new_alerts(snap: &ForecastSnapshot) {
    let p = prefs::get();
    if !p.notify_alerts {
        return;
    }
    let fresh = fresh(&snap.alerts, &p.notified_alerts);
    if fresh.is_empty() {
        return;
    }
    if let Some(app) = gio::Application::default() {
        for a in &fresh {
            let n = gio::Notification::new(&format!("{} · {}", a.event, snap.location.name));
            n.set_body(a.headline.lines().next());
            n.set_priority(if a.severity == AlertSeverity::Extreme {
                gio::NotificationPriority::Urgent
            } else {
                gio::NotificationPriority::High
            });
            app.send_notification(Some(&a.id), &n);
        }
    }
    prefs::update(|p| {
        for key in fresh.iter().flat_map(|a| a.keys()) {
            if !p.notified_alerts.contains(&key) {
                p.notified_alerts.push(key);
            }
        }
        let extra = p.notified_alerts.len().saturating_sub(REMEMBERED);
        p.notified_alerts.drain(..extra);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use nyx_weather_core::merge_alerts;

    fn warning(id: &str, refs: &[&str], vtec: &str) -> WeatherAlert {
        WeatherAlert {
            id: id.into(),
            event: "Freeze Warning".into(),
            severity: AlertSeverity::Severe,
            references: refs.iter().map(|r| r.to_string()).collect(),
            vtec: vec![vtec.into()],
            ..Default::default()
        }
    }

    #[test]
    fn each_warning_notifies_once() {
        let tonight = warning("P.001.1", &["Q.001.2"], "KBMX.FZ.W.0002");
        let saturday = warning("P.001.2", &["Q.001.3"], "KBMX.FZ.W.0003");
        let alerts = merge_alerts(vec![tonight, saturday]);
        let first = fresh(&alerts, &[]);
        assert_eq!(first.len(), 1);

        let notified: Vec<String> = first.iter().flat_map(|a| a.keys()).collect();
        assert!(fresh(&alerts, &notified).is_empty());
        // The next update has new ids but names the one before, and keeps its VTEC event.
        let update = merge_alerts(vec![warning("R.001.1", &["P.001.1"], "KBMX.FZ.W.0002")]);
        assert!(fresh(&update, &notified).is_empty());
        // Ids notified before this change still count.
        assert!(fresh(&alerts, &["P.001.2".to_string()]).is_empty());
        // A new warning does notify.
        let next_week = merge_alerts(vec![warning("S.001.1", &[], "KBMX.FZ.W.0004")]);
        assert_eq!(fresh(&next_week, &notified).len(), 1);
    }
}
