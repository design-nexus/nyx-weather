//! Worker-thread weather fetching, radar tile loading, caching, and offline fallback orchestration.

use crate::paths;
use nyx_weather_core::{
    Cache, FIELD_SIZE, FieldGrid, ForecastSnapshot, Location, OpenMeteoConfig, OpenMeteoProvider, ProviderError, RadarClient,
    RadarManifest, TileView, WeatherProvider,
};
use std::time::{SystemTime, UNIX_EPOCH};

const GEOCODING_TTL_SECS: i64 = 24 * 3600;

#[derive(Debug, Clone)]
pub struct FetchOutcome {
    pub snapshot: Option<ForecastSnapshot>,
    pub stale_fallback: bool,
    pub error: Option<ProviderError>,
}

pub fn now_epoch() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// Format how long ago `fetched_at` occurred (e.g. `"just now"`, `"4m ago"`, `"2h ago"`).
pub fn human_age(fetched_at: i64) -> String {
    let diff = now_epoch().saturating_sub(fetched_at);
    if diff < 60 {
        "just now".into()
    } else if diff < 3600 {
        format!("{}m ago", diff / 60)
    } else if diff < 86400 {
        format!("{}h ago", diff / 3600)
    } else {
        format!("{}d ago", diff / 86400)
    }
}

thread_local! {
    /// This thread's connection to the cache, opened (and migrated) once. The GTK
    /// thread only reads through it; WAL keeps those reads from waiting on writers.
    static CACHE: std::cell::RefCell<Option<Cache>> = const { std::cell::RefCell::new(None) };
}

/// Run `f` with this thread's cache connection, opening it first if needed.
fn with_cache<T>(f: impl FnOnce(&mut Cache) -> T) -> Option<T> {
    CACHE.with(|c| {
        let mut c = c.borrow_mut();
        if c.is_none() {
            *c = Cache::open(&paths::cache_db()).ok();
        }
        c.as_mut().map(f)
    })
}

/// Read cached snapshot immediately without touching the network.
pub fn get_cached(location_id: &str) -> Option<ForecastSnapshot> {
    with_cache(|c| match c.get_forecast(location_id) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("nyx-weather: cached forecast for {location_id}: {e:#}");
            None
        }
    })
    .flatten()
}

/// Forget cached forecasts of locations no longer saved, on a worker.
pub fn prune_forecasts(keep: Vec<String>) {
    std::thread::spawn(move || {
        with_cache(|c| {
            let ids: Vec<&str> = keep.iter().map(String::as_str).collect();
            let _ = c.retain_forecasts(&ids);
        });
    });
}

/// Load a fresh cached forecast or fetch from Open-Meteo on a worker thread,
/// preserving and returning any cached snapshot if the network is unreachable.
pub fn load_or_fetch(location: &Location, config: &OpenMeteoConfig, max_age_secs: i64, force_refresh: bool) -> FetchOutcome {
    let now = now_epoch();
    let cached_snapshot = get_cached(&location.id);
    let is_stale = with_cache(|c| c.is_stale(&location.id, max_age_secs, now).ok()).flatten().unwrap_or(true);

    if !force_refresh
        && !is_stale
        && let Some(snap) = cached_snapshot
    {
        return FetchOutcome { snapshot: Some(snap), stale_fallback: false, error: None };
    }

    let provider = OpenMeteoProvider::new(config.clone());
    match provider.fetch_forecast(location) {
        Ok(fresh) => {
            with_cache(|c| c.put_forecast(&fresh));
            FetchOutcome { snapshot: Some(fresh), stale_fallback: false, error: None }
        }
        Err(err) => FetchOutcome { stale_fallback: cached_snapshot.is_some(), snapshot: cached_snapshot, error: Some(err) },
    }
}

/// Search locations using the SQLite geocoding cache first, then Open-Meteo Geocoding API.
pub fn search_locations(query: &str, config: &OpenMeteoConfig) -> Result<Vec<Location>, ProviderError> {
    let q = query.trim();
    if q.len() < 2 {
        return Ok(Vec::new());
    }
    let now = now_epoch();
    if let Some(cached) = with_cache(|c| c.get_geocoding(q, GEOCODING_TTL_SECS, now).ok().flatten()).flatten()
        && !cached.is_empty()
    {
        return Ok(cached);
    }

    let provider = OpenMeteoProvider::new(config.clone());
    let results = provider.search_locations(q, 8)?;
    with_cache(|c| c.put_geocoding(q, &results, now));
    Ok(results)
}

fn radar_client() -> &'static RadarClient {
    static CLIENT: std::sync::OnceLock<RadarClient> = std::sync::OnceLock::new();
    CLIENT.get_or_init(RadarClient::new)
}

fn manifest_cache() -> &'static std::sync::Mutex<Option<RadarManifest>> {
    static MANIFEST: std::sync::OnceLock<std::sync::Mutex<Option<RadarManifest>>> = std::sync::OnceLock::new();
    MANIFEST.get_or_init(|| std::sync::Mutex::new(None))
}

/// Return cached RainViewer manifest if fresh (< 5 minutes old), or fetch and cache it.
pub fn fetch_radar_manifest() -> Result<RadarManifest, ProviderError> {
    let now = now_epoch();
    if let Ok(guard) = manifest_cache().lock()
        && let Some(m) = guard.as_ref()
        && now.saturating_sub(m.fetched_at) < 300
    {
        return Ok(m.clone());
    }
    let m = radar_client().fetch_manifest()?;
    if let Ok(mut guard) = manifest_cache().lock() {
        *guard = Some(m.clone());
    }
    Ok(m)
}

/// Whatever of `urls` is already in the cache, without network I/O.
pub fn cached_tiles(urls: &[Option<String>]) -> Vec<Option<Vec<u8>>> {
    with_cache(|c| RadarClient::cached_tiles(urls, c)).unwrap_or_else(|| vec![None; urls.len()])
}

/// Fetch map tiles (from the cache first, the rest over HTTP), on a worker.
pub fn fetch_tiles(urls: &[Option<String>]) -> Vec<Option<Vec<u8>>> {
    with_cache(|c| radar_client().fetch_tiles(urls, Some(c))).unwrap_or_else(|| radar_client().fetch_tiles(urls, None))
}

/// Pre-warm the Radar page's opening view (zoom 6 at a typical size, the basemap one
/// zoom in on HiDPI) and its newest radar frame, so opening Radar is instant.
pub fn prewarm_radar(location: &Location, scale: i32) {
    let (lat, lon, us) = (location.latitude, location.longitude, location.is_us());
    std::thread::spawn(move || {
        let (w, h) = (1100.0, 640.0);
        let base_z = if scale >= 2 { 7 } else { 6 };
        let _ = fetch_tiles(&RadarClient::basemap_urls(&TileView::covering(lat, lon, 6, base_z, w, h)));
        let (host, frame) = if us {
            (String::new(), nyx_weather_core::iem_radar_frames(now_epoch()).pop())
        } else {
            match fetch_radar_manifest() {
                Ok(m) => (m.host.clone(), m.radar_frames.into_iter().rfind(|f| !f.is_nowcast)),
                Err(_) => return,
            }
        };
        if let Some(f) = frame {
            let view = TileView::covering(lat, lon, 6, f.tile_zoom(6), w, h);
            let _ = fetch_tiles(&RadarClient::overlay_urls(&view, &host, &f));
        }
    });
}

/// Wind, temperature and pressure over `view`: from the cache when under 30 minutes
/// old, otherwise from Open-Meteo.
pub fn fetch_field(view: &TileView, config: &OpenMeteoConfig) -> Result<FieldGrid, ProviderError> {
    let key = view.key();
    let now = now_epoch();
    if let Some(grid) = with_cache(|c| c.get_field(&key, 30 * 60, now).ok().flatten()).flatten() {
        return Ok(grid);
    }
    let grid = OpenMeteoProvider::new(config.clone()).fetch_field(view.bounds(), FIELD_SIZE)?;
    with_cache(|c| c.put_field(&key, &grid));
    Ok(grid)
}
