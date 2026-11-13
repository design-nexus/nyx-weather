//! Provider-independent weather models, unit conversions, provider adapters,
//! natural query evaluation, and SQLite caching for Weather.

pub mod alerts;
pub mod cache;
pub mod field;
pub mod model;
pub mod noaa;
pub mod open_meteo;
pub mod provider;
pub mod query;
pub mod radar;
pub mod reflectivity;

pub use alerts::merge_alerts;
pub use cache::Cache;
pub use field::{Bounds, FIELD_SIZE, FieldGrid, FieldPoint};
pub use model::*;
pub use noaa::NoaaAlertProvider;
pub use open_meteo::{OpenMeteoConfig, OpenMeteoProvider};
pub use provider::{AlertProvider, ProviderError, WeatherProvider};
pub use query::{QueryIntent, QueryResponse, evaluate_query, parse_query};
pub use radar::{
    RAINVIEWER_MAX_ZOOM, RadarClient, RadarFrame, RadarLayerKind, RadarManifest, TileCoord, TileView,
    iem_radar_frames, iem_satellite_frame, lat_lon_to_tile, tile_xy,
};
pub use reflectivity::{Echo, RadarPalette};
