//! Weather radar and infrared satellite tile client over an OpenStreetMap basemap: the
//! NEXRAD mosaic from the Iowa Environmental Mesonet (IEM) for US places, RainViewer
//! elsewhere, and GOES-East infrared from IEM. A map is a [`TileView`]: the tiles that
//! cover it at their native size.

use crate::cache::Cache;
use crate::field::{Bounds, tile_to_lat_lon};
use crate::provider::ProviderError;
use crate::reflectivity::RadarPalette;
use reqwest::blocking::Client;
use reqwest::redirect;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::VecDeque;
use std::io::Read;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const RAINVIEWER_MAPS_URL: &str = "https://api.rainviewer.com/public/weather-maps.json";
const MAX_TILE_BYTES: usize = 512 * 1024;
const BASEMAP_TTL_SECS: i64 = 7 * 86_400;
/// RainViewer and timestamped IEM tile URLs name their frame's time, so a cached one
/// never goes stale.
const RADAR_TILE_TTL_SECS: i64 = 3 * 3600;
/// IEM's satellite tiles are always "latest" under the same URL.
const LIVE_TILE_TTL_SECS: i64 = 5 * 60;
/// RainViewer's free API allows 100 requests a minute per address; stay under it.
const RAINVIEWER_PER_MINUTE: usize = 90;
/// Tiles fetched at once, per batch.
const PARALLEL_FETCHES: usize = 12;
/// The most tiles a view spans each way.
const MAX_SPAN: i32 = 24;
/// IEM's mosaic: a frame every 5 minutes, for the last hour, starting this far back
/// (the mosaic takes a few minutes to appear).
const IEM_STEP_SECS: i64 = 300;
const IEM_FRAMES: i64 = 13;
const IEM_LAG_SECS: i64 = 600;

/// How long a cached overlay tile at `url` stays current.
fn overlay_ttl(url: &str) -> i64 {
    if url.contains("/cache/tile.py/") { LIVE_TILE_TTL_SECS } else { RADAR_TILE_TTL_SECS }
}

/// The highest zoom RainViewer serves; outside the US, radar stops here.
pub const RAINVIEWER_MAX_ZOOM: u8 = 7;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RadarLayerKind {
    #[default]
    PrecipitationRadar,
    InfraredSatellite,
    WindStreamlines,
    TemperatureContour,
}

impl RadarLayerKind {
    pub fn id(self) -> &'static str {
        match self {
            Self::PrecipitationRadar => "radar",
            Self::InfraredSatellite => "satellite",
            Self::WindStreamlines => "wind",
            Self::TemperatureContour => "temp",
        }
    }

    pub fn from_id(id: &str) -> Self {
        match id {
            "satellite" => Self::InfraredSatellite,
            "wind" => Self::WindStreamlines,
            "temp" => Self::TemperatureContour,
            _ => Self::PrecipitationRadar,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::PrecipitationRadar => "Precipitation Radar",
            Self::InfraredSatellite => "Infrared Satellite",
            Self::WindStreamlines => "Wind",
            Self::TemperatureContour => "Temp & Pressure",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::PrecipitationRadar => "Radar reflectivity on the classic NWS scale (dBZ).",
            Self::InfraredSatellite => "Infrared cloud-top imagery.",
            Self::WindStreamlines => "Current 10 m wind over the map, from a 7×7 grid of Open-Meteo points.",
            Self::TemperatureContour => {
                "Current 2 m temperature and sea-level isobars (every 2 hPa), from a 7×7 grid of Open-Meteo points."
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RadarFrame {
    pub time: i64,
    /// RainViewer's tile path; `iem-n0q:YYYYMMDDHHMI` for an IEM mosaic frame;
    /// `iem:LAYER` for IEM's latest satellite imagery.
    pub path: String,
    pub is_nowcast: bool,
}

impl RadarFrame {
    /// The colours this frame's tiles are drawn in, for a radar frame.
    pub fn palette(&self) -> Option<RadarPalette> {
        if self.path.starts_with("iem-n0q:") {
            Some(RadarPalette::Iem)
        } else if self.path.starts_with("iem:") || self.path.contains("/satellite/") {
            None
        } else {
            Some(RadarPalette::RainViewer)
        }
    }

    /// The zoom to fetch this frame's tiles at for a map at `map_z`. RainViewer's tiles
    /// are 512 px, so one zoom out gives the map's own detail in a quarter of the
    /// requests.
    pub fn tile_zoom(&self, map_z: u8) -> u8 {
        match self.palette() {
            Some(RadarPalette::RainViewer) => map_z.saturating_sub(1).clamp(1, RAINVIEWER_MAX_ZOOM),
            _ => map_z,
        }
    }

    /// The URL of tile (`x`, `y`) at `z` (`x` already wrapped); `host` is RainViewer's.
    pub fn tile_url(&self, host: &str, z: u8, x: i32, y: i32) -> String {
        if let Some(stamp) = self.path.strip_prefix("iem-n0q:") {
            format!("https://mesonet.agron.iastate.edu/c/tile.py/1.0.0/ridge::USCOMP-N0Q-{stamp}/{z}/{x}/{y}.png")
        } else if let Some(layer) = self.path.strip_prefix("iem:") {
            format!("https://mesonet.agron.iastate.edu/cache/tile.py/1.0.0/{layer}/{z}/{x}/{y}.png")
        } else if self.path.contains("/satellite/") {
            format!("{}{}/512/{z}/{x}/{y}/0/0_0.png", host.trim_end_matches('/'), self.path)
        } else {
            format!("{}{}/512/{z}/{x}/{y}/2/1_1.png", host.trim_end_matches('/'), self.path)
        }
    }
}

/// IEM's NEXRAD mosaic frames for the last hour, oldest first, at `now` (Unix seconds).
pub fn iem_radar_frames(now: i64) -> Vec<RadarFrame> {
    let latest = (now - IEM_LAG_SECS).div_euclid(IEM_STEP_SECS) * IEM_STEP_SECS;
    (0..IEM_FRAMES)
        .rev()
        .map(|i| {
            let time = latest - i * IEM_STEP_SECS;
            let stamp = chrono::DateTime::from_timestamp(time, 0).map(|t| t.format("%Y%m%d%H%M").to_string()).unwrap_or_default();
            RadarFrame { time, path: format!("iem-n0q:{stamp}"), is_nowcast: false }
        })
        .collect()
}

/// IEM's latest GOES-East infrared imagery, as a single frame at `now`.
pub fn iem_satellite_frame(now: i64) -> RadarFrame {
    RadarFrame { time: now, path: "iem:goes-east-ir-4km-900913".to_string(), is_nowcast: false }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RadarManifest {
    pub host: String,
    pub radar_frames: Vec<RadarFrame>,
    pub satellite_frames: Vec<RadarFrame>,
    pub fetched_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TileCoord {
    pub z: u8,
    pub x: i32,
    pub y: i32,
    /// Fractional offset `(0.0..1.0)` inside the center tile `(x, y)`.
    pub frac_x: f64,
    pub frac_y: f64,
}

/// Where (`lat`, `lon`) is at zoom `z`, in tiles (fractional), Web Mercator.
pub fn tile_xy(lat: f64, lon: f64, z: u8) -> (f64, f64) {
    let n = (1u32 << z) as f64;
    let lat_rad = lat.clamp(-85.0511, 85.0511).to_radians();
    let xf = (lon + 180.0) / 360.0 * n;
    let yf = (1.0 - (lat_rad.tan() + 1.0 / lat_rad.cos()).ln() / std::f64::consts::PI) / 2.0 * n;
    (xf, yf)
}

/// Convert WGS84 `(lat, lon)` and zoom `z` (`1..=12`) into Web Mercator tile coordinates.
pub fn lat_lon_to_tile(lat: f64, lon: f64, z: u8) -> TileCoord {
    let z_clamped = z.clamp(1, 12);
    let (xf, yf) = tile_xy(lat, lon, z_clamped);
    TileCoord { z: z_clamped, x: xf.floor() as i32, y: yf.floor() as i32, frac_x: xf.fract(), frac_y: yf.fract() }
}

/// The tiles that cover a map: `cols × rows` tiles at zoom `z` from (`x0`, `y0`), each
/// `tile_px` px wide on the map. `x0` isn't wrapped, so a view can cross the antimeridian.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TileView {
    pub z: u8,
    pub x0: i32,
    pub y0: i32,
    pub cols: i32,
    pub rows: i32,
    pub tile_px: f64,
}

impl TileView {
    /// The tiles at `tile_z` that cover a `w × h` px map at zoom `map_z` centred on
    /// (`lat`, `lon`). A tile is 256 px at the map's zoom, half that one zoom in.
    pub fn covering(lat: f64, lon: f64, map_z: u8, tile_z: u8, w: f64, h: f64) -> Self {
        let tile_px = 256.0 * 2f64.powi(map_z as i32 - tile_z as i32);
        let (cx, cy) = tile_xy(lat, lon, tile_z);
        let (half_w, half_h) = (w.max(1.0) / tile_px / 2.0, h.max(1.0) / tile_px / 2.0);
        let n = 1i32 << tile_z;
        let x0 = (cx - half_w).floor() as i32;
        let x1 = (cx + half_w).ceil() as i32;
        let y0 = ((cy - half_h).floor() as i32).max(0);
        let y1 = ((cy + half_h).ceil() as i32).min(n);
        Self {
            z: tile_z,
            x0,
            y0,
            cols: (x1 - x0).clamp(1, MAX_SPAN.min(n.max(1) * 2)),
            rows: (y1 - y0).clamp(1, MAX_SPAN),
            tile_px,
        }
    }

    pub fn len(&self) -> usize {
        (self.cols * self.rows) as usize
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Each tile's (column, row) in the view, row-major.
    pub fn cells(&self) -> impl Iterator<Item = (i32, i32)> + use<> {
        let (cols, rows) = (self.cols, self.rows);
        (0..rows).flat_map(move |r| (0..cols).map(move |c| (c, r)))
    }

    /// The tile at (`col`, `row`) of the view, as (`x` wrapped, `y`), or `None` past the
    /// poles.
    pub fn tile(&self, col: i32, row: i32) -> Option<(i32, i32)> {
        let n = 1i32 << self.z;
        let y = self.y0 + row;
        (0..n).contains(&y).then(|| ((self.x0 + col).rem_euclid(n), y))
    }

    /// The area the view shows.
    pub fn bounds(&self) -> Bounds {
        let (north, west) = tile_to_lat_lon(self.x0, self.y0, self.z);
        let (south, east) = tile_to_lat_lon(self.x0 + self.cols, self.y0 + self.rows, self.z);
        Bounds { north, south, west, east }
    }

    /// A key naming the view, for caches.
    pub fn key(&self) -> String {
        format!("{}/{}/{}/{}x{}", self.z, self.x0, self.y0, self.cols, self.rows)
    }
}

/// A sliding window of RainViewer request times, to stay under its rate limit.
struct RateLimit {
    sent: Mutex<VecDeque<Instant>>,
    per_minute: usize,
}

impl RateLimit {
    /// Wait until another request fits in the last minute's budget.
    fn acquire(&self) {
        loop {
            let wait = {
                let Ok(mut sent) = self.sent.lock() else { return };
                let now = Instant::now();
                while sent.front().is_some_and(|t| now.duration_since(*t) >= Duration::from_secs(60)) {
                    sent.pop_front();
                }
                if sent.len() < self.per_minute {
                    sent.push_back(now);
                    return;
                }
                sent.front().map(|t| Duration::from_secs(60).saturating_sub(now.duration_since(*t)))
            };
            std::thread::sleep(wait.unwrap_or(Duration::from_secs(1)).min(Duration::from_secs(1)));
        }
    }
}

pub struct RadarClient {
    http: Client,
    rainviewer: RateLimit,
}

impl Default for RadarClient {
    fn default() -> Self {
        Self::new()
    }
}

impl RadarClient {
    pub fn new() -> Self {
        let http = Client::builder()
            .user_agent("nyx-weather/0.1 (https://github.com/design-nexus/nyx)")
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(12))
            .redirect(redirect::Policy::none())
            .build()
            .unwrap_or_else(|_| Client::new());
        Self { http, rainviewer: RateLimit { sent: Mutex::new(VecDeque::new()), per_minute: RAINVIEWER_PER_MINUTE } }
    }

    pub fn fetch_manifest(&self) -> Result<RadarManifest, ProviderError> {
        let resp = self.http.get(RAINVIEWER_MAPS_URL).send().map_err(|e| {
            if e.is_connect() || e.is_timeout() {
                ProviderError::Offline
            } else {
                ProviderError::Failed("Could not reach RainViewer radar service.".into())
            }
        })?;
        if !resp.status().is_success() {
            return Err(ProviderError::Failed(format!("RainViewer returned HTTP {}", resp.status().as_u16())));
        }
        let mut body = Vec::new();
        resp.take(512 * 1024).read_to_end(&mut body).map_err(|_| ProviderError::InvalidResponse)?;
        let val: Value = serde_json::from_slice(&body).map_err(|_| ProviderError::InvalidResponse)?;
        Self::parse_manifest_json(&val)
    }

    pub fn parse_manifest_json(val: &Value) -> Result<RadarManifest, ProviderError> {
        let host = val["host"].as_str().unwrap_or("https://tilecache.rainviewer.com").trim_end_matches('/').to_string();

        let mut radar_frames = Vec::new();
        if let Some(past) = val["radar"]["past"].as_array() {
            for item in past {
                if let (Some(time), Some(path)) = (item["time"].as_i64(), item["path"].as_str()) {
                    radar_frames.push(RadarFrame { time, path: path.to_string(), is_nowcast: false });
                }
            }
        }
        if let Some(nowcast) = val["radar"]["nowcast"].as_array() {
            for item in nowcast {
                if let (Some(time), Some(path)) = (item["time"].as_i64(), item["path"].as_str()) {
                    radar_frames.push(RadarFrame { time, path: path.to_string(), is_nowcast: true });
                }
            }
        }

        let mut satellite_frames = Vec::new();
        if let Some(infrared) = val["satellite"]["infrared"].as_array() {
            for item in infrared {
                if let (Some(time), Some(path)) = (item["time"].as_i64(), item["path"].as_str()) {
                    satellite_frames.push(RadarFrame { time, path: path.to_string(), is_nowcast: false });
                }
            }
        }

        let fetched_at = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);

        if satellite_frames.is_empty() {
            satellite_frames.push(RadarFrame {
                time: fetched_at,
                path: "iem:goes-east-ir-4km-900913".to_string(),
                is_nowcast: false,
            });
        }

        Ok(RadarManifest { host, radar_frames, satellite_frames, fetched_at })
    }

    fn is_invalid_rainviewer_tile(url: &str, bytes: &[u8]) -> bool {
        // RainViewer answers an unavailable tile with one fixed placeholder image.
        url.contains("rainviewer.com") && bytes.len() == 1370
    }

    fn ttl(url: &str) -> i64 {
        if url.contains("tile.openstreetmap.org") { BASEMAP_TTL_SECS } else { overlay_ttl(url) }
    }

    fn fetch_bytes_http(&self, url: &str) -> Option<Vec<u8>> {
        if url.contains("rainviewer.com") {
            self.rainviewer.acquire();
        }
        let resp = self.http.get(url).send().ok()?;
        if !resp.status().is_success() {
            return None;
        }
        let mut buf = Vec::new();
        resp.take((MAX_TILE_BYTES + 1) as u64).read_to_end(&mut buf).ok()?;
        if buf.is_empty() || buf.len() > MAX_TILE_BYTES || Self::is_invalid_rainviewer_tile(url, &buf) {
            return None;
        }
        Some(buf)
    }

    /// The OpenStreetMap tiles of `view`, row-major (`None` past the poles).
    pub fn basemap_urls(view: &TileView) -> Vec<Option<String>> {
        view.cells()
            .map(|(c, r)| view.tile(c, r).map(|(x, y)| format!("https://tile.openstreetmap.org/{}/{x}/{y}.png", view.z)))
            .collect()
    }

    /// The tiles of `frame` over `view`, row-major.
    pub fn overlay_urls(view: &TileView, host: &str, frame: &RadarFrame) -> Vec<Option<String>> {
        view.cells().map(|(c, r)| view.tile(c, r).map(|(x, y)| frame.tile_url(host, view.z, x, y))).collect()
    }

    /// Whatever of `urls` is already in the cache, without any network I/O.
    pub fn cached_tiles(urls: &[Option<String>], cache: &Cache) -> Vec<Option<Vec<u8>>> {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
        urls.iter()
            .map(|u| {
                let url = u.as_ref()?;
                cache.get_tile(url, Self::ttl(url), now).ok().flatten().filter(|b| !Self::is_invalid_rainviewer_tile(url, b))
            })
            .collect()
    }

    /// Fetch `urls`: cached tiles from SQLite, the rest over HTTP a few at a time
    /// (RainViewer's within its rate limit), written back to the cache.
    pub fn fetch_tiles(&self, urls: &[Option<String>], mut cache: Option<&mut Cache>) -> Vec<Option<Vec<u8>>> {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
        let mut results: Vec<Option<Vec<u8>>> = match cache.as_deref() {
            Some(c) => Self::cached_tiles(urls, c),
            None => vec![None; urls.len()],
        };
        let misses: Vec<(usize, &str)> = urls
            .iter()
            .enumerate()
            .filter(|(i, u)| u.is_some() && results[*i].is_none())
            .filter_map(|(i, u)| u.as_deref().map(|u| (i, u)))
            .collect();
        for chunk in misses.chunks(PARALLEL_FETCHES) {
            let fetched: Vec<(usize, &str, Option<Vec<u8>>)> = std::thread::scope(|s| {
                let handles: Vec<_> =
                    chunk.iter().map(|&(i, url)| s.spawn(move || (i, url, self.fetch_bytes_http(url)))).collect();
                handles.into_iter().filter_map(|h| h.join().ok()).collect()
            });
            for (i, url, bytes) in fetched {
                if let Some(bytes) = bytes {
                    if let Some(c) = cache.as_deref_mut() {
                        let _ = c.put_tile(url, &bytes, now);
                    }
                    results[i] = Some(bytes);
                }
            }
        }
        results
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn computes_slippy_tile_coordinates_and_parses_manifest() {
        let tc = lat_lon_to_tile(40.7128, -74.0060, 6);
        assert_eq!(tc.z, 6);
        assert_eq!(tc.x, 18);
        assert_eq!(tc.y, 24);
        assert!((0.0..1.0).contains(&tc.frac_x));
        assert!((0.0..1.0).contains(&tc.frac_y));

        let sample = serde_json::json!({
            "host": "https://tilecache.rainviewer.com",
            "radar": {
                "past": [{"time": 1729280000, "path": "/v2/radar/1729280000"}],
                "nowcast": [{"time": 1729280600, "path": "/v2/radar/1729280600"}]
            },
            "satellite": {
                "infrared": [{"time": 1729280000, "path": "/v2/satellite/1729280000"}]
            }
        });
        let manifest = RadarClient::parse_manifest_json(&sample).unwrap();
        assert_eq!(manifest.radar_frames.len(), 2);
        assert!(!manifest.radar_frames[0].is_nowcast);
        assert!(manifest.radar_frames[1].is_nowcast);
        assert_eq!(manifest.satellite_frames.len(), 1);
    }

    #[test]
    fn a_view_covers_the_map_at_native_size() {
        // 1280 × 720 at zoom 6 around Birmingham, AL: 256 px tiles, 5–7 across.
        let v = TileView::covering(33.5, -86.8, 6, 6, 1280.0, 720.0);
        assert_eq!(v.tile_px, 256.0);
        assert!((5..=7).contains(&v.cols) && (3..=5).contains(&v.rows), "{v:?}");
        let (cx, cy) = tile_xy(33.5, -86.8, 6);
        assert!(v.x0 as f64 <= cx - 2.5 && (v.x0 + v.cols) as f64 >= cx + 2.5);
        assert!(v.y0 as f64 <= cy - 1.4 && (v.y0 + v.rows) as f64 >= cy + 1.4);
        // One zoom in (HiDPI), tiles are half as wide and twice as many each way.
        let hi = TileView::covering(33.5, -86.8, 6, 7, 1280.0, 720.0);
        assert_eq!(hi.tile_px, 128.0);
        assert!(hi.cols >= 2 * v.cols - 2);
        let b = v.bounds();
        assert!(b.north > 33.5 && b.south < 33.5 && b.west < -86.8 && b.east > -86.8);
    }

    #[test]
    fn views_wrap_across_the_antimeridian() {
        let v = TileView::covering(0.0, 179.9, 4, 4, 1024.0, 256.0);
        let xs: Vec<i32> = (0..v.cols).filter_map(|c| v.tile(c, 0).map(|(x, _)| x)).collect();
        assert!(xs.contains(&15) && xs.contains(&0), "{xs:?}");
    }

    #[test]
    fn iem_frames_step_every_five_minutes_behind_now() {
        let now = 1_793_173_456;
        let frames = iem_radar_frames(now);
        assert_eq!(frames.len(), 13);
        assert!(frames.windows(2).all(|w| w[1].time - w[0].time == 300));
        let last = frames.last().unwrap();
        assert_eq!(last.time % 300, 0);
        assert!(now - last.time >= 600 && now - last.time < 900);
        let url = last.tile_url("", 6, 16, 25);
        assert!(url.starts_with("https://mesonet.agron.iastate.edu/c/tile.py/1.0.0/ridge::USCOMP-N0Q-"), "{url}");
        assert!(url.ends_with("/6/16/25.png"));
        assert_eq!(last.palette(), Some(RadarPalette::Iem));
        assert_eq!(last.tile_zoom(9), 9);
    }

    #[test]
    fn rainviewer_frames_use_512_px_tiles_one_zoom_out() {
        let f = RadarFrame { time: 0, path: "/v2/radar/abc".into(), is_nowcast: false };
        assert_eq!(f.palette(), Some(RadarPalette::RainViewer));
        assert_eq!(f.tile_zoom(6), 5);
        assert_eq!(f.tile_zoom(9), RAINVIEWER_MAX_ZOOM);
        assert_eq!(
            f.tile_url("https://tilecache.rainviewer.com/", 5, 8, 12),
            "https://tilecache.rainviewer.com/v2/radar/abc/512/5/8/12/2/1_1.png"
        );
        let rv = TileView::covering(33.5, -86.8, 6, f.tile_zoom(6), 1280.0, 720.0);
        assert_eq!(rv.tile_px, 512.0);
        assert!(rv.len() <= 12, "{rv:?}");
        assert_eq!(iem_satellite_frame(5).palette(), None);
    }
}
