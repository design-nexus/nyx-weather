//! SQLite forecast and geocoding cache with transactional updates and offline fallback.

use crate::model::{ForecastSnapshot, Location};
use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OptionalExtension, params};
use std::path::Path;
use std::time::Duration;

pub struct Cache {
    db: Connection,
    tiles_put: u32,
}

/// The newest schema; older caches are brought up to it once, on open.
const VERSION: i64 = 2;
/// Tiles kept per kind (basemap, everything else), so animating radar frames never
/// pushes the map out.
const TILES_KEPT: i64 = 1500;
const BASEMAP: &str = "%tile.openstreetmap.org%";

impl Cache {
    /// Open or initialize the SQLite weather cache at `path`.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }
        Self::from_connection(Connection::open(path)?)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    fn from_connection(db: Connection) -> Result<Self> {
        db.busy_timeout(Duration::from_secs(3))?;
        let version: i64 = db.pragma_query_value(None, "user_version", |r| r.get(0))?;
        ensure!(version <= VERSION, "This weather cache requires a newer version of Weather");
        if version < VERSION {
            db.execute_batch(
                "PRAGMA journal_mode=WAL;
                CREATE TABLE IF NOT EXISTS forecasts (
                    location_id TEXT PRIMARY KEY,
                    fetched_at INTEGER NOT NULL,
                    snapshot_json TEXT NOT NULL
                );
                CREATE TABLE IF NOT EXISTS geocoding_cache (
                    query TEXT PRIMARY KEY,
                    fetched_at INTEGER NOT NULL,
                    locations_json TEXT NOT NULL
                );
                CREATE TABLE IF NOT EXISTS tile_cache (
                    url TEXT PRIMARY KEY,
                    fetched_at INTEGER NOT NULL,
                    png_bytes BLOB NOT NULL
                );
                DELETE FROM tile_cache WHERE url LIKE '%cartocdn.com%' OR length(png_bytes) = 1370;",
            )?;
            db.pragma_update(None, "user_version", VERSION)?;
        }
        Ok(Self { db, tiles_put: 0 })
    }

    /// Store a normalized forecast snapshot inside a transaction.
    /// Existing offline data for other locations is untouched.
    pub fn put_forecast(&mut self, snapshot: &ForecastSnapshot) -> Result<()> {
        ensure!(!snapshot.location.id.trim().is_empty(), "Snapshot location ID must not be empty");
        let json = serde_json::to_string(snapshot).context("Could not serialize forecast snapshot")?;
        let tx = self.db.transaction()?;
        tx.execute(
            "INSERT INTO forecasts (location_id, fetched_at, snapshot_json)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(location_id) DO UPDATE SET
                 fetched_at = excluded.fetched_at,
                 snapshot_json = excluded.snapshot_json",
            params![snapshot.location.id, snapshot.fetched_at, json],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Read a cached forecast snapshot for `location_id` (even if stale, for offline fallback).
    pub fn get_forecast(&self, location_id: &str) -> Result<Option<ForecastSnapshot>> {
        let raw: Option<String> = self
            .db
            .query_row("SELECT snapshot_json FROM forecasts WHERE location_id = ?1", params![location_id], |row| row.get(0))
            .optional()?;
        match raw {
            Some(json) => {
                let mut snapshot: ForecastSnapshot = serde_json::from_str(&json).context("Invalid cached forecast JSON")?;
                snapshot.advance_to(crate::model::now_unix());
                Ok(Some(snapshot))
            }
            None => Ok(None),
        }
    }

    /// Forget cached forecasts for every location not in `keep`.
    pub fn retain_forecasts(&mut self, keep: &[&str]) -> Result<()> {
        let ids: Vec<String> = {
            let mut stmt = self.db.prepare("SELECT location_id FROM forecasts")?;
            stmt.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?
        };
        for id in ids.iter().filter(|id| !keep.contains(&id.as_str())) {
            self.db.execute("DELETE FROM forecasts WHERE location_id = ?1", params![id])?;
        }
        Ok(())
    }

    /// Return true if no cached forecast exists or its `fetched_at` is older than `max_age_secs`.
    pub fn is_stale(&self, location_id: &str, max_age_secs: i64, now_epoch: i64) -> Result<bool> {
        let fetched_at: Option<i64> = self
            .db
            .query_row("SELECT fetched_at FROM forecasts WHERE location_id = ?1", params![location_id], |row| row.get(0))
            .optional()?;
        Ok(match fetched_at {
            Some(ts) => now_epoch.saturating_sub(ts) >= max_age_secs,
            None => true,
        })
    }

    /// Delete a single location's cached forecast.
    pub fn remove_forecast(&mut self, location_id: &str) -> Result<()> {
        self.db.execute("DELETE FROM forecasts WHERE location_id = ?1", params![location_id])?;
        Ok(())
    }

    /// Cache geocoding search results for `query`.
    pub fn put_geocoding(&mut self, query: &str, locations: &[Location], now_epoch: i64) -> Result<()> {
        let key = query.trim().to_lowercase();
        if key.is_empty() {
            return Ok(());
        }
        let json = serde_json::to_string(locations)?;
        self.db.execute(
            "INSERT INTO geocoding_cache (query, fetched_at, locations_json)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(query) DO UPDATE SET
                 fetched_at = excluded.fetched_at,
                 locations_json = excluded.locations_json",
            params![key, now_epoch, json],
        )?;
        // Searches are cached for a day; anything a month old is only taking room.
        self.db.execute("DELETE FROM geocoding_cache WHERE fetched_at < ?1", params![now_epoch - 30 * 86_400])?;
        Ok(())
    }

    /// Retrieve cached geocoding results if younger than `max_age_secs`.
    pub fn get_geocoding(&self, query: &str, max_age_secs: i64, now_epoch: i64) -> Result<Option<Vec<Location>>> {
        let key = query.trim().to_lowercase();
        let row: Option<(i64, String)> = self
            .db
            .query_row("SELECT fetched_at, locations_json FROM geocoding_cache WHERE query = ?1", params![key], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .optional()?;
        match row {
            Some((ts, json)) if now_epoch.saturating_sub(ts) < max_age_secs => Ok(Some(serde_json::from_str(&json)?)),
            _ => Ok(None),
        }
    }

    /// Store a PNG tile in the SQLite cache. Every 50 tiles, the oldest of each kind
    /// past `TILES_KEPT` are dropped.
    pub fn put_tile(&mut self, url: &str, png_bytes: &[u8], now_epoch: i64) -> Result<()> {
        self.db.execute(
            "INSERT INTO tile_cache (url, fetched_at, png_bytes)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(url) DO UPDATE SET
                 fetched_at = excluded.fetched_at,
                 png_bytes = excluded.png_bytes",
            params![url, now_epoch, png_bytes],
        )?;
        self.tiles_put += 1;
        if self.tiles_put % 50 == 1 {
            for kind in ["url LIKE ?1", "url NOT LIKE ?1"] {
                let _ = self.db.execute(
                    &format!(
                        "DELETE FROM tile_cache WHERE url IN (
                            SELECT url FROM tile_cache WHERE {kind}
                            ORDER BY fetched_at DESC LIMIT -1 OFFSET ?2
                        )"
                    ),
                    params![BASEMAP, TILES_KEPT],
                );
            }
        }
        Ok(())
    }

    /// Store a surface field under `key` (it shares the tile table).
    pub fn put_field(&mut self, key: &str, grid: &crate::field::FieldGrid) -> Result<()> {
        let json = serde_json::to_vec(grid)?;
        self.put_tile(&format!("field:{key}"), &json, grid.fetched_at)
    }

    /// A surface field stored under `key`, if younger than `max_age_secs`.
    pub fn get_field(&self, key: &str, max_age_secs: i64, now_epoch: i64) -> Result<Option<crate::field::FieldGrid>> {
        Ok(self.get_tile(&format!("field:{key}"), max_age_secs, now_epoch)?.and_then(|b| serde_json::from_slice(&b).ok()))
    }

    /// Retrieve a cached PNG tile if younger than `max_age_secs`.
    pub fn get_tile(&self, url: &str, max_age_secs: i64, now_epoch: i64) -> Result<Option<Vec<u8>>> {
        let row: Option<(i64, Vec<u8>)> = self
            .db
            .query_row("SELECT fetched_at, png_bytes FROM tile_cache WHERE url = ?1", params![url], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .optional()?;
        match row {
            Some((ts, bytes)) if now_epoch.saturating_sub(ts) < max_age_secs => Ok(Some(bytes)),
            _ => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{CurrentWeather, ForecastSnapshot, Location, ProviderAttribution, WeatherCondition};

    fn sample_snapshot(loc: Location, fetched_at: i64, temp: f64) -> ForecastSnapshot {
        ForecastSnapshot {
            location: loc,
            fetched_at,
            utc_offset_seconds: 0,
            current: CurrentWeather {
                time: "2026-10-18T12:00".into(),
                temperature_c: temp,
                apparent_temperature_c: temp,
                condition: WeatherCondition::Clear,
                is_day: true,
                humidity_pct: 50,
                dew_point_c: Some(10.0),
                wind_speed_kmh: 12.0,
                wind_gusts_kmh: None,
                wind_direction_deg: 180,
                pressure_hpa: Some(1013.0),
                precipitation_mm: 0.0,
                cloud_cover_pct: 10,
                visibility_m: Some(10000.0),
                uv_index: Some(4.0),
            },
            hourly: vec![],
            daily: vec![],
            air_quality: None,
            alerts: vec![],
            alerts_unavailable: false,
            models: vec![],
            minutely_precip: vec![],
            attribution: ProviderAttribution {
                provider_id: "open-meteo".into(),
                provider_name: "Open-Meteo".into(),
                license_name: "CC BY 4.0".into(),
                attribution_text: "Weather data by Open-Meteo.com".into(),
                attribution_url: "https://open-meteo.com/".into(),
                commercial_use_allowed: false,
            },
        }
    }

    #[test]
    fn round_trips_missing_model_values() -> Result<()> {
        let mut cache = Cache::open_in_memory()?;
        let loc = Location::new("Austin", None, None, Some("US".into()), 30.0, -97.0, None, None);
        let mut snap = sample_snapshot(loc.clone(), 9_000, 20.0);
        snap.models.push(crate::model::ModelSeries {
            model_id: "icon_seamless".into(),
            model_name: "DWD ICON".into(),
            resolution: String::new(),
            daily_max_c: vec![20.0, f64::NAN],
            daily_min_c: vec![10.0, f64::NAN],
            daily_precip_mm: vec![],
            hourly_temp_c: vec![],
        });
        cache.put_forecast(&snap)?;
        let back = cache.get_forecast(&loc.id)?.unwrap();
        assert_eq!(back.models[0].daily_max_c[0], 20.0);
        assert!(back.models[0].daily_max_c[1].is_nan());
        Ok(())
    }

    #[test]
    fn stores_isolates_and_checks_staleness() -> Result<()> {
        let mut cache = Cache::open_in_memory()?;
        let austin = Location::new(
            "Austin",
            Some("Texas".into()),
            Some("United States".into()),
            Some("US".into()),
            30.2672,
            -97.7431,
            None,
            None,
        );
        let oslo = Location::new("Oslo", None, Some("Norway".into()), Some("NO".into()), 59.9139, 10.7522, None, None);

        assert!(cache.is_stale(&austin.id, 1800, 10_000)?);
        cache.put_forecast(&sample_snapshot(austin.clone(), 9_000, 25.0))?;
        cache.put_forecast(&sample_snapshot(oslo.clone(), 9_500, 8.0))?;

        assert!(!cache.is_stale(&austin.id, 1800, 10_000)?);
        assert!(cache.is_stale(&austin.id, 1800, 11_000)?);

        // Even when stale, offline fallback still returns the last known snapshot.
        let cached_austin = cache.get_forecast(&austin.id)?.unwrap();
        assert_eq!(cached_austin.current.temperature_c, 25.0);
        let cached_oslo = cache.get_forecast(&oslo.id)?.unwrap();
        assert_eq!(cached_oslo.current.temperature_c, 8.0);

        cache.put_geocoding("Aust", std::slice::from_ref(&austin), 10_000)?;
        assert_eq!(cache.get_geocoding("aust", 3600, 10_500)?.unwrap().len(), 1);
        assert!(cache.get_geocoding("aust", 3600, 20_000)?.is_none());

        Ok(())
    }
}
