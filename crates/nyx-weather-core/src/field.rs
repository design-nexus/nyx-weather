//! Surface fields over the map: current wind, temperature and sea-level pressure at a
//! square grid of points (one Open-Meteo request), with bilinear sampling and contour
//! lines for drawing them.

use crate::radar::TileCoord;
use serde::{Deserialize, Serialize};

/// Points per side of the grid.
pub const FIELD_SIZE: usize = 7;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct FieldPoint {
    pub lat: f64,
    pub lon: f64,
    /// NaN where the service had no value.
    #[serde(deserialize_with = "crate::model::nan::one")]
    pub temperature_c: f64,
    #[serde(deserialize_with = "crate::model::nan::one")]
    pub pressure_hpa: f64,
    #[serde(deserialize_with = "crate::model::nan::one")]
    pub wind_speed_kmh: f64,
    #[serde(deserialize_with = "crate::model::nan::one")]
    pub wind_direction_deg: f64,
}

/// `size`×`size` points, row by row from north to south, each row west to east, spread
/// evenly over `bounds`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FieldGrid {
    pub size: usize,
    pub bounds: Bounds,
    pub points: Vec<FieldPoint>,
    pub fetched_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Bounds {
    pub north: f64,
    pub south: f64,
    pub west: f64,
    pub east: f64,
}

impl Bounds {
    /// The area the 3×3 tile map around `center` shows.
    pub fn of_tile_grid(center: TileCoord) -> Self {
        let (north, west) = tile_to_lat_lon(center.x - 1, center.y - 1, center.z);
        let (south, east) = tile_to_lat_lon(center.x + 2, center.y + 2, center.z);
        Self { north, south, west, east }
    }

    /// The coordinates of each grid point, in the order of [`FieldGrid::points`].
    pub fn points(&self, size: usize) -> Vec<(f64, f64)> {
        let step = |a: f64, b: f64, i: usize| a + (b - a) * i as f64 / (size - 1).max(1) as f64;
        (0..size)
            .flat_map(|r| (0..size).map(move |c| (r, c)))
            .map(|(r, c)| (step(self.north, self.south, r), step(self.west, self.east, c)))
            .collect()
    }
}

/// The north-west corner of tile (`x`, `y`) at zoom `z`, as (lat, lon).
pub fn tile_to_lat_lon(x: i32, y: i32, z: u8) -> (f64, f64) {
    let n = (1u32 << z) as f64;
    let lon = x as f64 / n * 360.0 - 180.0;
    let lat = (std::f64::consts::PI * (1.0 - 2.0 * y as f64 / n)).sinh().atan().to_degrees();
    (lat, lon)
}

impl FieldGrid {
    fn value(&self, row: usize, col: usize, f: impl Fn(&FieldPoint) -> f64) -> f64 {
        self.points.get(row * self.size + col).map(f).unwrap_or(f64::NAN)
    }

    /// Bilinear value at grid position (`gx`, `gy`) in 0..size-1 (column, row).
    pub fn sample(&self, gx: f64, gy: f64, f: impl Fn(&FieldPoint) -> f64 + Copy) -> f64 {
        let max = (self.size - 1) as f64;
        let (gx, gy) = (gx.clamp(0.0, max), gy.clamp(0.0, max));
        let (c0, r0) = (gx.floor() as usize, gy.floor() as usize);
        let (c1, r1) = ((c0 + 1).min(self.size - 1), (r0 + 1).min(self.size - 1));
        let (tx, ty) = (gx - c0 as f64, gy - r0 as f64);
        let top = self.value(r0, c0, f) * (1.0 - tx) + self.value(r0, c1, f) * tx;
        let bottom = self.value(r1, c0, f) * (1.0 - tx) + self.value(r1, c1, f) * tx;
        top * (1.0 - ty) + bottom * ty
    }

    /// Wind at (`gx`, `gy`) as (east, north) components in km/h, interpolated as a vector
    /// so directions either side of north don't average to south.
    pub fn wind(&self, gx: f64, gy: f64) -> (f64, f64) {
        // Direction is where the wind comes from; the vector points where it goes.
        let u = |p: &FieldPoint| -p.wind_speed_kmh * p.wind_direction_deg.to_radians().sin();
        let v = |p: &FieldPoint| -p.wind_speed_kmh * p.wind_direction_deg.to_radians().cos();
        (self.sample(gx, gy, u), self.sample(gx, gy, v))
    }

    /// Finite (min, max) of a quantity over the grid.
    pub fn range(&self, f: impl Fn(&FieldPoint) -> f64) -> Option<(f64, f64)> {
        let vals = self.points.iter().map(f).filter(|v| v.is_finite());
        vals.fold(None, |acc, v| match acc {
            None => Some((v, v)),
            Some((lo, hi)) => Some((lo.min(v), hi.max(v))),
        })
    }

    /// Line segments where the quantity crosses `level`, in grid positions
    /// (column, row), by marching squares over each cell.
    pub fn contour(&self, level: f64, f: impl Fn(&FieldPoint) -> f64 + Copy) -> Vec<[(f64, f64); 2]> {
        let mut out = Vec::new();
        for r in 0..self.size.saturating_sub(1) {
            for c in 0..self.size.saturating_sub(1) {
                // Corners clockwise from the top left, with their positions.
                let corners = [(c, r), (c + 1, r), (c + 1, r + 1), (c, r + 1)];
                let vals: Vec<f64> = corners.iter().map(|&(cc, rr)| self.value(rr, cc, f)).collect();
                if vals.iter().any(|v| !v.is_finite()) {
                    continue;
                }
                let mut crossings = Vec::new();
                for i in 0..4 {
                    let (a, b) = (vals[i], vals[(i + 1) % 4]);
                    if (a < level) != (b < level) {
                        let t = (level - a) / (b - a);
                        let (pa, pb) = (corners[i], corners[(i + 1) % 4]);
                        crossings
                            .push((pa.0 as f64 + (pb.0 as f64 - pa.0 as f64) * t, pa.1 as f64 + (pb.1 as f64 - pa.1 as f64) * t));
                    }
                }
                // Two crossings make one segment; four (a saddle) make two.
                for pair in crossings.as_chunks::<2>().0 {
                    out.push([pair[0], pair[1]]);
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid(f: impl Fn(usize, usize) -> f64) -> FieldGrid {
        let bounds = Bounds { north: 1.0, south: 0.0, west: 0.0, east: 1.0 };
        let points = bounds
            .points(3)
            .into_iter()
            .enumerate()
            .map(|(i, (lat, lon))| FieldPoint {
                lat,
                lon,
                temperature_c: f(i / 3, i % 3),
                pressure_hpa: f64::NAN,
                wind_speed_kmh: 10.0,
                wind_direction_deg: if i % 2 == 0 { 350.0 } else { 10.0 },
            })
            .collect();
        FieldGrid { size: 3, bounds, points, fetched_at: 0 }
    }

    #[test]
    fn samples_contours_and_winds() {
        // Temperature rises west to east: 0, 1, 2 in every row.
        let g = grid(|_, c| c as f64);
        assert_eq!(g.sample(0.5, 1.0, |p| p.temperature_c), 0.5);
        assert_eq!(g.range(|p| p.temperature_c), Some((0.0, 2.0)));
        let segs = g.contour(0.5, |p| p.temperature_c);
        assert_eq!(segs.len(), 2);
        assert!(segs.iter().flatten().all(|&(x, _)| (x - 0.5).abs() < 1e-9));
        // Winds from 350° and 10° average to a wind from the north (blowing south).
        let (u, v) = g.wind(0.5, 0.0);
        assert!(u.abs() < 1e-9 && v < 0.0);
        assert!(g.range(|p| p.pressure_hpa).is_none());
    }

    #[test]
    fn bounds_cover_the_tile_grid() {
        let b = Bounds::of_tile_grid(crate::lat_lon_to_tile(40.0, -74.0, 6));
        assert!(b.north > 40.0 && b.south < 40.0 && b.west < -74.0 && b.east > -74.0);
        assert_eq!(b.points(7).len(), 49);
    }
}
